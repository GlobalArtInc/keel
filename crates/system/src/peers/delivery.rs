use std::sync::{Arc, OnceLock};
use std::time::Duration;

use keel_sdk::chrono::Datelike;
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::EntityTrait;
use keel_sdk::serde_json;
use keel_sdk::system::{
    AccountSource, Caller, Endpoint, EndpointRequest, EventRelay, OutboxSubscriber, PeerReach,
    PrincipalSource, SecretCipher, peer_sees,
};
use keel_sdk::types::{Event, Scope, Via};
use tokio::sync::Notify;

use super::entity::product;
use super::functions::by_instance;
use super::inbox;
use super::pairing::ERROR_MASK;
use crate::webhooks::api::{
    EVENT_ID_HEADER, PEER_HEADER, SIGNATURE_HEADER, admits, any_matches, is_mask, verify,
};

pub const HOOK: &str = "hook";
pub const FEED: &str = "feed";
pub const FEED_LIMIT: u32 = 100;
pub const FEED_LIMIT_MAX: u32 = 500;
pub const FEED_BATCH: u32 = 500;
pub const FEED_SCAN: usize = 5_000;
pub const MAX_TYPES: usize = 64;
pub const YEARS: std::ops::RangeInclusive<i32> = 0..=9999;
pub const ERROR_SIGNATURE: &str = "peer.error.signature";
pub const ERROR_EVENT: &str = "peer.error.event";
pub const ERROR_FEED: &str = "peer.error.feed";
pub const ERROR_QUERY: &str = "peer.error.query";
const UNAUTHENTICATED: &str = "keel.error.unauthenticated";
const UNAVAILABLE: &str = "keel.error.unavailable";
pub(super) const RETRY_PAUSE: Duration = Duration::from_secs(5);

pub(super) struct Seams {
    pub(super) db: Db,
    pub(super) clock: Arc<dyn Clock>,
    pub(super) cipher: Arc<dyn SecretCipher>,
    pub(super) subscriber: Arc<dyn OutboxSubscriber>,
    pub(super) relay: Arc<dyn EventRelay>,
    pub(super) caller: Arc<dyn Caller>,
    pub(super) principals: Arc<dyn PrincipalSource>,
    pub(super) accounts: Arc<dyn AccountSource>,
    pub(super) reach: Arc<dyn PeerReach>,
    pub(super) instance: Option<Id>,
}

#[derive(Default)]
pub struct Delivery {
    seams: OnceLock<Seams>,
    wake: Notify,
}

impl Delivery {
    pub(super) fn attach(&self, seams: Seams) {
        if self.seams.set(seams).is_err() {
            tracing::warn!("the peer delivery was started twice; the first seams stand");
        }
    }

    pub(super) fn seams(&self) -> Result<&Seams> {
        self.seams
            .get()
            .ok_or_else(|| KeelError::internal("peer.error.not_started"))
    }

    pub(super) fn wake(&self) {
        self.wake.notify_one();
    }

    pub(super) async fn woken(&self) {
        self.wake.notified().await;
    }
}

pub(super) fn endpoints(delivery: &Arc<Delivery>) -> [Endpoint; 2] {
    let hooked = delivery.clone();
    let fed = delivery.clone();
    [
        Endpoint::post(HOOK, move |request| {
            let delivery = hooked.clone();
            async move { hook(&delivery, request).await }
        }),
        Endpoint::get(FEED, move |request| {
            let delivery = fed.clone();
            async move { feed(&delivery, request).await }
        }),
    ]
}

fn unsigned() -> KeelError {
    KeelError::unauthenticated(ERROR_SIGNATURE)
}

fn not_an_event(reason: &str) -> KeelError {
    KeelError::validation(ERROR_EVENT).param("reason", reason)
}

pub(super) fn representable(event: &Event) -> bool {
    YEARS.contains(&event.at.year())
}

async fn hook(delivery: &Delivery, request: EndpointRequest) -> Result<Value> {
    let seams = delivery.seams()?;
    let (Some(peer), Some(signature)) = (
        request.header(PEER_HEADER).map(str::trim),
        request.header(SIGNATURE_HEADER),
    ) else {
        return Err(unsigned());
    };
    let Some(row) = by_instance(&seams.db, peer).await? else {
        return Err(unsigned());
    };
    let Some(sealed) = row.webhook_secret_enc.as_deref() else {
        return Err(unsigned());
    };
    let secret = seams.cipher.open(sealed)?;
    if let Err(rejection) = verify(
        &secret,
        signature,
        &request.body,
        seams.clock.now().timestamp(),
    ) {
        tracing::warn!(peer = %row.id, instance = row.instance_id, %rejection, "a delivery was refused");
        return Err(unsigned());
    }
    let event: Event = serde_json::from_slice(&request.body).map_err(|_| not_an_event("body"))?;
    let id = event.id.to_string();
    if request.header(EVENT_ID_HEADER).map(str::trim) != Some(id.as_str()) {
        return Err(not_an_event("id"));
    }
    if event.instance_id.to_string() != row.instance_id {
        return Err(not_an_event("instance"));
    }
    if !representable(&event) {
        return Err(not_an_event("at"));
    }
    let inserted = inbox::receive(&seams.db, &row.id, &event, seams.clock.now()).await?;
    if inserted {
        delivery.wake();
    }
    Ok(json!({ "data": { "eventId": id, "duplicate": !inserted } }))
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct FeedQuery {
    pub(super) after: u64,
    pub(super) types: Vec<String>,
    pub(super) limit: u32,
}

fn malformed(name: &str) -> KeelError {
    KeelError::validation(ERROR_QUERY)
        .field(name)
        .param("name", name)
}

fn single<'a>(request: &'a EndpointRequest, name: &str) -> Result<Option<&'a str>> {
    let mut values = request.param(name);
    let first = values.next();
    if values.next().is_some() {
        return Err(malformed(name));
    }
    Ok(first)
}

pub(super) fn query(request: &EndpointRequest) -> Result<FeedQuery> {
    let after = match single(request, "after")? {
        Some(text) => text.trim().parse().map_err(|_| malformed("after"))?,
        None => 0,
    };
    let limit = match single(request, "limit")? {
        Some(text) => text
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|limit| (1..=FEED_LIMIT_MAX).contains(limit))
            .ok_or_else(|| malformed("limit").param("max", FEED_LIMIT_MAX))?,
        None => FEED_LIMIT,
    };
    let types: Vec<String> = request
        .param("types")
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|mask| !mask.is_empty())
        .map(str::to_owned)
        .collect();
    if types.len() > MAX_TYPES {
        return Err(malformed("types").param("max", MAX_TYPES));
    }
    for (index, mask) in types.iter().enumerate() {
        if !is_mask(mask) {
            return Err(KeelError::validation(ERROR_MASK)
                .field(format!("types[{index}]"))
                .param("mask", mask.as_str()));
        }
    }
    Ok(FeedQuery {
        after,
        types,
        limit,
    })
}

async fn feed(delivery: &Delivery, request: EndpointRequest) -> Result<Value> {
    let seams = delivery.seams()?;
    let principal = &request.principal;
    if principal.level == Level::Guest {
        return Err(KeelError::unauthenticated(UNAUTHENTICATED));
    }
    let (Via::Peer(link), Scope::Peer(_), None) =
        (&principal.via, &principal.scope, &principal.actor)
    else {
        return Err(KeelError::forbidden(ERROR_FEED));
    };
    let Some(row) = product::Entity::find_by_id(link.to_string())
        .one(&seams.db)
        .await?
    else {
        return Err(KeelError::unauthenticated(UNAUTHENTICATED));
    };
    let wanted = query(&request)?;
    let local = seams
        .instance
        .ok_or_else(|| KeelError::unavailable(UNAVAILABLE))?;
    let scope = AccountScope::of(principal);
    let floor = u64::try_from(row.feed_seq).unwrap_or_default();
    let mut next = wanted.after.max(floor);
    let mut data: Vec<Event> = Vec::new();
    let mut scanned = 0;
    'scan: while scanned < FEED_SCAN {
        let batch = seams.subscriber.after(next, FEED_BATCH).await?;
        if batch.is_empty() {
            break;
        }
        scanned += batch.len();
        for event in batch {
            next = event.seq;
            let served = event.instance_id == local
                && any_matches(row.events.iter().map(String::as_str), &event.kind)
                && (wanted.types.is_empty()
                    || any_matches(wanted.types.iter().map(String::as_str), &event.kind))
                && admits(&scope, &event, Some(local), &seams.db).await?
                && peer_sees(
                    Some(seams.reach.as_ref()),
                    seams.accounts.as_ref(),
                    principal,
                    &event,
                    &seams.db,
                )
                .await?;
            if served {
                data.push(event);
                if data.len() >= wanted.limit as usize {
                    break 'scan;
                }
            }
        }
    }
    Ok(json!({ "data": data, "next": next }))
}

#[cfg(test)]
mod tests {
    use keel_sdk::system::Client;
    use keel_sdk::types::ErrorCode;

    use super::*;

    fn request(query: &[(&str, &str)]) -> EndpointRequest {
        EndpointRequest {
            principal: keel_sdk::types::Principal {
                id: Id::generate(),
                login: "peer".into(),
                level: Level::Admin,
                account_id: Id::ROOT_ACCOUNT,
                role: "peer".into(),
                actor: None,
                scope: Scope::Full,
                via: Via::Http,
                external_id: None,
                origin: None,
                lang: "en".into(),
                grants: Default::default(),
            },
            client: Client::default(),
            request_id: "req".into(),
            headers: Vec::new(),
            query: query
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
            body: Vec::new(),
        }
    }

    fn refused(query: &[(&str, &str)], key: &str, path: &str) {
        let error = super::query(&request(query)).unwrap_err().to_call_error();
        assert_eq!(error.code, ErrorCode::Validation, "{query:?}");
        assert_eq!(error.fields[0].key.as_deref(), Some(key), "{query:?}");
        assert_eq!(error.fields[0].path, path, "{query:?}");
    }

    #[test]
    fn an_event_time_is_representable_from_the_year_zero_to_the_year_9999() {
        let at = |text: &str| -> Event {
            serde_json::from_value(json!({
                "id": Id::generate(),
                "seq": 1,
                "type": "probe.note.saved",
                "at": text,
                "instanceId": Id::generate(),
                "module": "probe",
                "data": {}
            }))
            .unwrap_or_else(|error| panic!("{text}: {error}"))
        };
        for accepted in [
            "0000-01-01T00:00:00Z",
            "2026-10-04T12:00:00Z",
            "9999-12-31T23:59:59.999Z",
        ] {
            assert!(representable(&at(accepted)), "{accepted}");
        }
        for refused in ["-5000-01-01T00:00:00Z", "+10000-01-01T00:00:00Z"] {
            assert!(!representable(&at(refused)), "{refused}");
        }
    }

    #[test]
    fn the_feed_query_defaults_after_and_limit_and_reads_types_as_masks_split_on_commas() {
        assert_eq!(
            query(&request(&[])).unwrap(),
            FeedQuery {
                after: 0,
                types: Vec::new(),
                limit: FEED_LIMIT,
            }
        );
        assert_eq!(
            query(&request(&[
                ("after", "8812"),
                ("types", "dns.zone.*, svc.*"),
                ("types", "probe.note.saved"),
                ("limit", "500"),
            ]))
            .unwrap(),
            FeedQuery {
                after: 8812,
                types: vec![
                    "dns.zone.*".into(),
                    "svc.*".into(),
                    "probe.note.saved".into()
                ],
                limit: 500,
            }
        );
        refused(&[("after", "-1")], ERROR_QUERY, "after");
        refused(&[("after", "x")], ERROR_QUERY, "after");
        refused(&[("after", "1"), ("after", "2")], ERROR_QUERY, "after");
        refused(&[("limit", "0")], ERROR_QUERY, "limit");
        refused(&[("limit", "501")], ERROR_QUERY, "limit");
        refused(&[("types", "dns.zone.*,dns")], ERROR_MASK, "types[1]");
        let many = vec!["probe.a.*"; MAX_TYPES + 1].join(",");
        refused(&[("types", many.as_str())], ERROR_QUERY, "types");
    }
}
