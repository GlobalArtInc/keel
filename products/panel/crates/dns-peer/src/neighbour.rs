use std::time::Duration;

use keel::prelude::*;
use keel::sdk::system::PeerLink;
use keel::types::{CallError, ErrorCode, FieldError};
use keel_client::{Client, Credential, Error, OnBehalf};

pub const ORIGIN_NAME: &str = "panel";
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(4);
pub(crate) const CHECK_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
pub const UNREACHABLE: &str = "dns_peer.error.unreachable";
pub const ANSWER: &str = "dns_peer.error.answer";
pub const CLIENT: &str = "dns_peer.error.client";
pub const EXISTS: &str = "dns_peer.error.exists";
pub const TEMPLATE: &str = "dns_peer.error.template";
const PRINCIPAL_UPSERT: &str = "keel.principal.upsert";
const ZONE_CHECK: &str = "dns.zone.check";
const ZONE_CREATE: &str = "dns.zone.create";
const CUSTOMER_LEVEL: &str = "user";
const ZONE_NAME: &str = "name";
const SITE_DOMAIN: &str = "domain";
const SITE_ZONE: &str = "dnsZone";
const REASON_CAP: usize = 256;
const TIMED_OUT: &str = "the neighbour did not answer in time";
const RELAYED: [(&str, &str); 2] = [
    ("dns.zone.error.exists", EXISTS),
    ("dns.zone.error.template", TEMPLATE),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Customer {
    pub(crate) origin: String,
    pub(crate) external_id: String,
    pub(crate) login: String,
}

impl Customer {
    pub(crate) fn of(instance: Id, account: Id) -> Self {
        let external_id = account.to_string();
        Self {
            origin: format!("{ORIGIN_NAME}:{instance}"),
            login: format!(
                "{ORIGIN_NAME}-{}-{}",
                instance.to_string().to_ascii_lowercase(),
                external_id.to_ascii_lowercase()
            ),
            external_id,
        }
    }

    fn upsert(&self) -> Value {
        json!({
            "origin": self.origin,
            "externalId": self.external_id,
            "login": self.login,
            "level": CUSTOMER_LEVEL,
            "account": { "origin": self.origin, "externalId": self.external_id },
        })
    }

    fn on_behalf(&self) -> Result<OnBehalf> {
        OnBehalf::new(self.origin.as_str(), self.external_id.as_str()).map_err(invalid)
    }
}

pub(crate) struct Neighbour {
    client: Client,
}

impl Neighbour {
    pub(crate) fn of(link: &PeerLink, quick: bool) -> Result<Self> {
        let credential = Credential::peer(link.token.expose()).map_err(invalid)?;
        let builder = Client::builder(&link.url).credential(credential);
        let builder = if quick {
            builder
                .timeout(CHECK_TIMEOUT)
                .connect_timeout(CHECK_CONNECT_TIMEOUT)
                .attempts(1)
        } else {
            builder
        };
        Ok(Self {
            client: builder.build().map_err(invalid)?,
        })
    }

    pub(crate) async fn check(&self, customer: &Customer, name: &str) -> Result<()> {
        self.sync(customer).await?;
        self.client
            .call::<Value>(ZONE_CHECK, json!({ "name": name }))
            .on_behalf(customer.on_behalf()?)
            .await
            .map(drop)
            .map_err(|error| self.refused(error))
    }

    pub(crate) async fn create(&self, customer: &Customer, name: &str, key: &str) -> Result<Value> {
        self.sync(customer).await?;
        self.client
            .call::<Value>(ZONE_CREATE, json!({ "name": name }))
            .on_behalf(customer.on_behalf()?)
            .idempotency_key(key)
            .await
            .map(|answer| answer.data)
            .map_err(|error| self.refused(error))
    }

    pub(crate) fn timed_out(&self) -> KeelError {
        KeelError::peer(UNREACHABLE)
            .param("url", self.client.base())
            .param("reason", TIMED_OUT)
    }

    async fn sync(&self, customer: &Customer) -> Result<()> {
        self.client
            .call::<Value>(PRINCIPAL_UPSERT, customer.upsert())
            .await
            .map(drop)
            .map_err(|error| self.refused(error))
    }

    fn refused(&self, error: Error) -> KeelError {
        mapped(self.client.base(), error)
    }
}

pub(crate) fn mapped(base: &str, error: Error) -> KeelError {
    match error {
        Error::Refused(refusal) => {
            let CallError {
                key,
                params,
                fields,
                ..
            } = refusal.error;
            params
                .into_iter()
                .fold(
                    KeelError::with_key(ErrorCode::Peer, relayed(key)),
                    |error, (name, value)| error.param(name, value),
                )
                .fields(fields.into_iter().map(onto_the_site))
        }
        Error::Transport { reason, .. } => KeelError::peer(UNREACHABLE)
            .param("url", base)
            .param("reason", truncate(&reason)),
        Error::Answer { status, reason, .. } => KeelError::peer(ANSWER)
            .param("url", base)
            .param("status", status)
            .param("reason", truncate(&reason)),
        other => invalid(other),
    }
}

fn onto_the_site(field: FieldError) -> FieldError {
    let path = if field.path == ZONE_NAME {
        SITE_DOMAIN
    } else {
        SITE_ZONE
    };
    let local = field.key.as_deref().and_then(ours);
    FieldError {
        path: path.to_owned(),
        code: field.code.or_else(|| local.and(field.key.clone())),
        key: local.map(str::to_owned).or(field.key),
        params: field.params,
    }
}

fn ours(theirs: &str) -> Option<&'static str> {
    RELAYED
        .iter()
        .find(|(key, _)| *key == theirs)
        .map(|(_, ours)| *ours)
}

fn relayed(theirs: String) -> String {
    ours(&theirs).map_or(theirs, str::to_owned)
}

fn invalid(error: Error) -> KeelError {
    KeelError::internal(CLIENT).param("reason", truncate(&error.to_string()))
}

pub(crate) fn detail(error: &KeelError) -> String {
    let answer = error.to_call_error();
    answer
        .fields
        .iter()
        .find_map(|field| field.key.clone())
        .unwrap_or(answer.key)
}

fn truncate(text: &str) -> String {
    if text.len() <= REASON_CAP {
        return text.to_owned();
    }
    let mut end = REASON_CAP;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use keel::sdk::{Secret, serde_json};
    use keel_client::Refusal;

    use super::*;

    const INSTANCE: &str = "01J9A00000000000000000000A";
    const ACCOUNT: &str = "01J8A0000000000000000000A1";

    fn refusal(code: ErrorCode, key: &str, fields: Vec<FieldError>) -> Error {
        let mut error = CallError::new(code);
        error.key = key.to_owned();
        error.params.insert("max".to_owned(), json!(50));
        error.fields = fields;
        Error::Refused(Box::new(Refusal {
            url: "http://127.0.0.1:1/api/v1/call/dns.zone.check".to_owned(),
            status: code.status(),
            error,
        }))
    }

    fn field(path: &str, key: &str) -> FieldError {
        FieldError {
            path: path.to_owned(),
            code: Some(key.to_owned()),
            key: Some(key.to_owned()),
            params: [("name".to_owned(), json!("taken.example"))]
                .into_iter()
                .collect(),
        }
    }

    #[test]
    fn a_customer_is_named_by_this_installation_and_the_account_of_the_site() {
        let customer = Customer::of(INSTANCE.parse().unwrap(), ACCOUNT.parse().unwrap());
        assert_eq!(customer.origin, format!("panel:{INSTANCE}"));
        assert_eq!(customer.external_id, ACCOUNT);
        assert_eq!(
            customer.login,
            format!(
                "panel-{}-{}",
                INSTANCE.to_ascii_lowercase(),
                ACCOUNT.to_ascii_lowercase()
            )
        );
        assert!(customer.login.len() <= 64, "{}", customer.login);
        assert_eq!(
            customer.upsert(),
            json!({
                "origin": customer.origin,
                "externalId": ACCOUNT,
                "login": customer.login,
                "level": "user",
                "account": { "origin": customer.origin, "externalId": ACCOUNT },
            })
        );
        assert_eq!(
            customer.on_behalf().unwrap().header(),
            format!("origin=panel:{INSTANCE}; external_id={ACCOUNT}")
        );
    }

    #[test]
    fn a_refusal_of_the_neighbour_answers_peer_with_its_key_and_its_fields_on_the_site_form() {
        let refused = refusal(
            ErrorCode::Validation,
            "keel.error.validation",
            vec![
                field("name", "dns.zone.error.exists"),
                field("template", "dns.zone.error.template"),
            ],
        );
        let error = mapped("http://127.0.0.1:1", refused);
        let answer = error.to_call_error();
        assert_eq!(answer.code, ErrorCode::Peer);
        assert_eq!(answer.status(), 502);
        assert_eq!(answer.key, "keel.error.validation");
        assert_eq!(answer.params["max"], json!(50));
        assert_eq!(answer.fields.len(), 2);
        assert_eq!(answer.fields[0].path, "domain");
        assert_eq!(answer.fields[0].key.as_deref(), Some(EXISTS));
        assert_eq!(
            answer.fields[0].code.as_deref(),
            Some("dns.zone.error.exists")
        );
        assert_eq!(answer.fields[0].params["name"], json!("taken.example"));
        assert_eq!(answer.fields[1].path, "dnsZone");
        assert_eq!(answer.fields[1].key.as_deref(), Some(TEMPLATE));
        assert_eq!(detail(&error), EXISTS);
        let limited = mapped(
            "http://127.0.0.1:1",
            refusal(
                ErrorCode::LimitExceeded,
                "keel.error.limit_exceeded",
                Vec::new(),
            ),
        );
        let answer = limited.to_call_error();
        assert_eq!(answer.code, ErrorCode::Peer);
        assert_eq!(answer.key, "keel.error.limit_exceeded");
        assert!(answer.fields.is_empty());
        assert_eq!(detail(&limited), "keel.error.limit_exceeded");
    }

    #[test]
    fn only_the_dns_keys_this_installation_carries_texts_for_are_relayed_under_its_own() {
        let bare = FieldError {
            code: None,
            ..field("name", "dns.zone.error.exists")
        };
        let checked = FieldError {
            code: Some("check.domain".to_owned()),
            ..field("name", "keel.check.domain")
        };
        let error = mapped(
            "http://127.0.0.1:1",
            refusal(
                ErrorCode::Conflict,
                "dns.zone.error.template",
                vec![
                    bare,
                    checked,
                    field("account", "dns.zone.error.account"),
                    field("name", "probe.error.refused"),
                ],
            ),
        );
        let answer = error.to_call_error();
        assert_eq!(answer.code, ErrorCode::Peer);
        assert_eq!(answer.key, TEMPLATE);
        assert_eq!(answer.params["max"], json!(50));
        let relayed: Vec<(&str, Option<&str>, Option<&str>)> = answer
            .fields
            .iter()
            .map(|field| {
                (
                    field.path.as_str(),
                    field.key.as_deref(),
                    field.code.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            relayed,
            [
                ("domain", Some(EXISTS), Some("dns.zone.error.exists")),
                ("domain", Some("keel.check.domain"), Some("check.domain")),
                (
                    "dnsZone",
                    Some("dns.zone.error.account"),
                    Some("dns.zone.error.account")
                ),
                (
                    "domain",
                    Some("probe.error.refused"),
                    Some("probe.error.refused")
                ),
            ]
        );
        let plain = mapped(
            "http://127.0.0.1:1",
            refusal(
                ErrorCode::Internal,
                "dns.zone.error.not_started",
                Vec::new(),
            ),
        );
        assert_eq!(plain.key(), "dns.zone.error.not_started");
        let dictionaries = [
            include_str!("../i18n/en.json"),
            include_str!("../i18n/ru.json"),
        ]
        .map(|text| serde_json::from_str::<Value>(text).expect("a dictionary"));
        for (theirs, ours) in RELAYED {
            assert!(theirs.starts_with("dns."), "{theirs}");
            let pointer = format!("/{}", ours.replace('.', "/"));
            for messages in &dictionaries {
                assert!(
                    messages.pointer(&pointer).is_some_and(Value::is_string),
                    "{ours}"
                );
            }
        }
    }

    #[test]
    fn a_neighbour_out_of_reach_or_out_of_contract_answers_peer_with_its_base() {
        let long = "ж".repeat(REASON_CAP);
        let transport = mapped(
            "https://dns.example:8443",
            Error::Transport {
                url: "https://dns.example:8443/api/v1/call/dns.zone.check".to_owned(),
                attempts: 3,
                reason: long.clone(),
            },
        );
        assert_eq!(transport.code(), ErrorCode::Peer);
        assert_eq!(transport.key(), UNREACHABLE);
        assert_eq!(transport.params()["url"], "https://dns.example:8443");
        let reason = transport.params()["reason"].as_str().unwrap();
        assert!(reason.len() <= REASON_CAP && long.starts_with(reason));
        let answer = mapped(
            "https://dns.example:8443",
            Error::Answer {
                url: "https://dns.example:8443/api/v1/call/dns.zone.check".to_owned(),
                status: 200,
                reason: "the body is not an envelope".to_owned(),
            },
        );
        assert_eq!(answer.code(), ErrorCode::Peer);
        assert_eq!(answer.key(), ANSWER);
        assert_eq!(answer.params()["status"], 200);
        assert_eq!(answer.params()["url"], "https://dns.example:8443");
        let refused = mapped(
            "https://dns.example:8443",
            Error::Invalid("not a function name".to_owned()),
        );
        assert_eq!(refused.code(), ErrorCode::Internal);
        assert_eq!(refused.key(), CLIENT);
    }

    #[test]
    fn a_link_with_a_malformed_token_or_address_is_refused_before_anything_is_sent() {
        let link = |url: &str, token: &str| PeerLink {
            id: Id::generate(),
            instance: INSTANCE.parse().unwrap(),
            url: url.to_owned(),
            token: Secret::new(token),
        };
        let quick = Neighbour::of(&link("http://127.0.0.1:1", "kpt_01J9B.token"), true).unwrap();
        assert_eq!(quick.client.base(), "http://127.0.0.1:1");
        assert_eq!(quick.timed_out().key(), UNREACHABLE);
        assert_eq!(quick.timed_out().params()["reason"], TIMED_OUT);
        for (url, token) in [
            ("http://127.0.0.1:1", "kat_01J9B.token"),
            ("http://127.0.0.1:1", "kpt_with space"),
            ("ftp://dns.example", "kpt_01J9B.token"),
        ] {
            let error = Neighbour::of(&link(url, token), false)
                .err()
                .unwrap_or_else(|| panic!("{url} {token}"));
            assert_eq!(error.code(), ErrorCode::Internal, "{url} {token}");
            assert_eq!(error.key(), CLIENT, "{url} {token}");
        }
    }
}
