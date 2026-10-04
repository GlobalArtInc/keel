use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::{Expr, OnConflict};
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set};
use keel_sdk::system::{AccountSource, CallerOptions};
use keel_sdk::types::{AccountState, ErrorCode, Event, EventSubject, Scope};

use super::authenticator::stored_principal;
use super::delivery::Seams;
use super::entity::{identity, product};
use super::on_behalf::{ERROR_ORIGIN, names_an_instance, names_peer};

pub const MASKS: [&str; 2] = ["keel.account.*", "keel.principal.*"];
pub const UPSERTS: [&str; 2] = ["keel.account.upsert", "keel.principal.upsert"];
const KERNEL: &str = "keel";
const ACCOUNT: &str = "keel.account";
const PRINCIPAL: &str = "keel.principal";
const SUSPENDED: &str = "keel.account.suspended";
const RESUMED: &str = "keel.account.resumed";
const DELETED: &str = "keel.account.deleted";
const RENAMED: &str = "keel.principal.renamed";
const SUSPEND: &str = "keel.account.suspend";
const RESUME: &str = "keel.account.resume";
const DELETE: &str = "keel.account.delete";
const UPDATE: &str = "keel.principal.update";
const ORIGIN: &str = "origin";

pub fn subscribed(subscribe: &[String], identity: bool) -> Vec<String> {
    let mut masks = subscribe.to_vec();
    if identity {
        for mask in MASKS {
            if !masks.iter().any(|held| held == mask) {
                masks.push(mask.to_owned());
            }
        }
    }
    masks
}

pub(super) async fn bound(call: Call<Value>) -> Result<()> {
    let principal = call.principal();
    let origin = call
        .params()
        .get(ORIGIN)
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut ids = vec![principal.id.to_string()];
    ids.extend(principal.actor.iter().map(|actor| actor.id.to_string()));
    let link = product::Entity::find()
        .filter(product::Column::PrincipalId.is_in(ids))
        .one(call.db())
        .await?;
    let allowed = match link {
        Some(link) => names_peer(origin, &link.instance_id),
        None => principal.level >= Level::Admin || !names_an_instance(origin),
    };
    if allowed {
        Ok(())
    } else {
        Err(KeelError::forbidden(ERROR_ORIGIN).param(ORIGIN, origin))
    }
}

pub(super) async fn reached(
    accounts: &dyn AccountSource,
    instance: &str,
    principal: &Principal,
    db: &Db,
) -> Result<bool> {
    if !principal
        .origin
        .as_deref()
        .is_some_and(|origin| names_peer(origin, instance))
    {
        return Ok(false);
    }
    Ok(accounts
        .by_id(principal.account_id, db)
        .await?
        .and_then(|account| account.origin)
        .is_some_and(|origin| names_peer(&origin, instance)))
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Change {
    Suspend,
    Resume,
    Delete,
    Rename(String),
}

fn change_of(event: &Event) -> Option<(Change, &EventSubject)> {
    if event.module != KERNEL {
        return None;
    }
    let subject = event.subject.as_ref()?;
    let change = match (event.kind.as_str(), subject.kind.as_str()) {
        (SUSPENDED, ACCOUNT) => Change::Suspend,
        (RESUMED, ACCOUNT) => Change::Resume,
        (DELETED, ACCOUNT) => Change::Delete,
        (RENAMED, PRINCIPAL) => Change::Rename(event.data.get("login")?.as_str()?.to_owned()),
        _ => return None,
    };
    Some((change, subject))
}

#[derive(Debug, PartialEq)]
enum Target {
    Unknown,
    Ambiguous(usize),
    Settled,
    Call(&'static str, Value),
}

fn single<T>(held: Vec<T>) -> Result<T, Target> {
    let count = held.len();
    <[T; 1]>::try_from(held)
        .map(|[one]| one)
        .map_err(|_| match count {
            0 => Target::Unknown,
            many => Target::Ambiguous(many),
        })
}

async fn account_target(seams: &Seams, instance: &str, id: Id, change: &Change) -> Result<Target> {
    let held: Vec<_> = seams
        .accounts
        .of_external_id(&id.to_string(), &seams.db)
        .await?
        .into_iter()
        .filter(|account| {
            account
                .origin
                .as_deref()
                .is_some_and(|origin| names_peer(origin, instance))
        })
        .collect();
    let account = match single(held) {
        Ok(account) => account,
        Err(target) => return Ok(target),
    };
    let pick = json!({ "id": account.id });
    Ok(match change {
        Change::Suspend => Target::Call(SUSPEND, pick),
        Change::Resume => Target::Call(RESUME, pick),
        Change::Delete if account.state == AccountState::Deleting => Target::Settled,
        Change::Delete => Target::Call(DELETE, pick),
        Change::Rename(_) => Target::Unknown,
    })
}

async fn principal_target(seams: &Seams, instance: &str, id: Id, login: &str) -> Result<Target> {
    let mut held = Vec::new();
    for principal in seams
        .principals
        .of_external_id(&id.to_string(), &seams.db)
        .await?
    {
        if reached(seams.accounts.as_ref(), instance, &principal, &seams.db).await? {
            held.push(principal);
        }
    }
    Ok(match single(held) {
        Ok(principal) if principal.login == login => Target::Settled,
        Ok(principal) => Target::Call(UPDATE, json!({ "id": principal.id, "login": login })),
        Err(target) => target,
    })
}

fn transient(error: &KeelError) -> bool {
    matches!(
        error.code(),
        ErrorCode::Internal | ErrorCode::Unavailable | ErrorCode::RateLimited
    )
}

async fn last_applied(db: &Db, link: &str, subject: &EventSubject) -> Result<Option<i64>> {
    Ok(identity::Entity::find_by_id((
        link.to_owned(),
        subject.kind.clone(),
        subject.id.to_string(),
    ))
    .one(db)
    .await?
    .map(|row| row.seq))
}

async fn record(
    db: &Db,
    link: &str,
    subject: &EventSubject,
    seq: i64,
    now: DateTime<Utc>,
) -> Result<()> {
    identity::Entity::insert(identity::ActiveModel {
        peer_id: Set(link.to_owned()),
        subject_kind: Set(subject.kind.clone()),
        subject_id: Set(subject.id.to_string()),
        seq: Set(seq),
        applied_at: Set(now),
    })
    .on_conflict(
        OnConflict::columns([
            identity::Column::PeerId,
            identity::Column::SubjectKind,
            identity::Column::SubjectId,
        ])
        .update_columns([identity::Column::Seq, identity::Column::AppliedAt])
        .action_and_where(Expr::col((identity::Entity, identity::Column::Seq)).lt(seq))
        .to_owned(),
    )
    .exec_without_returning(db)
    .await?;
    Ok(())
}

pub(super) async fn forget(db: &Db, link: &str) -> Result<u64> {
    let forgotten = identity::Entity::delete_many()
        .filter(identity::Column::PeerId.eq(link))
        .exec(db)
        .await?;
    Ok(forgotten.rows_affected)
}

async fn act(seams: &Seams, link: &product::Model, func: &str, params: Value) -> Result<()> {
    let id: Id = link
        .principal_id
        .as_deref()
        .and_then(|id| id.parse().ok())
        .ok_or_else(|| stored_principal(link))?;
    let mut principal = seams
        .principals
        .by_id(id, &seams.db)
        .await?
        .ok_or_else(|| stored_principal(link))?;
    principal.level = Level::Admin;
    principal.scope = Scope::Full;
    seams
        .caller
        .call(principal, func, params, CallerOptions::default())
        .await?;
    Ok(())
}

pub(super) async fn apply(seams: &Seams, link: &product::Model, event: &Event) -> Result<()> {
    let Some((change, subject)) = change_of(event) else {
        return Ok(());
    };
    let seq = i64::try_from(event.seq).unwrap_or(i64::MAX);
    if last_applied(&seams.db, &link.id, subject)
        .await?
        .is_some_and(|applied| applied >= seq)
    {
        tracing::debug!(peer = link.id, event = %event.id, seq, "an identity event older than the last one applied to its record is skipped");
        return Ok(());
    }
    let target = match &change {
        Change::Rename(login) => {
            principal_target(seams, &link.instance_id, subject.id, login).await?
        }
        _ => account_target(seams, &link.instance_id, subject.id, &change).await?,
    };
    match target {
        Target::Unknown => {
            tracing::debug!(peer = link.id, event = %event.id, subject = %subject.id, "no record here carries the subject of an identity event; it is skipped");
            return Ok(());
        }
        Target::Ambiguous(count) => {
            tracing::error!(peer = link.id, event = %event.id, subject = %subject.id, count, "several records here carry the subject of an identity event under origins of its peer; it is skipped");
            return Ok(());
        }
        Target::Settled => {}
        Target::Call(func, params) => match act(seams, link, func, params).await {
            Ok(()) => {}
            Err(error) if transient(&error) => return Err(error),
            Err(error) => {
                tracing::warn!(peer = link.id, event = %event.id, function = func, error = %error, "an identity event was refused here; it is skipped");
            }
        },
    }
    record(&seams.db, &link.id, subject, seq, seams.clock.now()).await
}

#[cfg(test)]
mod tests {
    use keel_sdk::serde_json;

    use super::*;

    fn event(kind: &str, module: &str, subject: Option<&str>, data: Value) -> Event {
        serde_json::from_value(json!({
            "id": Id::generate(),
            "seq": 7,
            "type": kind,
            "at": "2026-10-04T12:00:00Z",
            "instanceId": Id::generate(),
            "module": module,
            "subject": subject.map(|kind| json!({ "kind": kind, "id": Id::generate() })),
            "data": data
        }))
        .expect("an event")
    }

    #[test]
    fn the_identity_role_adds_the_account_and_principal_masks_once() {
        let asked = vec!["dns.zone.*".to_owned(), "keel.account.*".to_owned()];
        assert_eq!(subscribed(&asked, false), asked);
        assert_eq!(
            subscribed(&asked, true),
            ["dns.zone.*", "keel.account.*", "keel.principal.*"]
        );
        assert_eq!(subscribed(&[], true), MASKS);
        assert!(subscribed(&[], false).is_empty());
    }

    #[test]
    fn the_kernels_account_state_events_and_the_principal_rename_are_the_changes_applied() {
        let changes: Vec<Option<Change>> = [
            event(SUSPENDED, KERNEL, Some(ACCOUNT), json!({})),
            event(RESUMED, KERNEL, Some(ACCOUNT), json!({})),
            event(DELETED, KERNEL, Some(ACCOUNT), json!({})),
            event(
                RENAMED,
                KERNEL,
                Some(PRINCIPAL),
                json!({ "login": "alice" }),
            ),
        ]
        .iter()
        .map(|event| change_of(event).map(|(change, _)| change))
        .collect();
        assert_eq!(
            changes,
            [
                Some(Change::Suspend),
                Some(Change::Resume),
                Some(Change::Delete),
                Some(Change::Rename("alice".into())),
            ]
        );
        for ignored in [
            event("keel.account.created", KERNEL, Some(ACCOUNT), json!({})),
            event("keel.principal.created", KERNEL, Some(PRINCIPAL), json!({})),
            event("keel.principal.deleted", KERNEL, Some(PRINCIPAL), json!({})),
            event(SUSPENDED, "probe", Some(ACCOUNT), json!({})),
            event(SUSPENDED, KERNEL, Some(PRINCIPAL), json!({})),
            event(SUSPENDED, KERNEL, None, json!({})),
            event(RENAMED, KERNEL, Some(PRINCIPAL), json!({})),
            event(RENAMED, KERNEL, Some(PRINCIPAL), json!({ "login": 7 })),
            event(RENAMED, KERNEL, Some(ACCOUNT), json!({ "login": "alice" })),
        ] {
            assert!(
                change_of(&ignored).is_none(),
                "{} {:?}",
                ignored.kind,
                ignored.subject
            );
        }
    }

    #[test]
    fn only_one_record_is_a_target_and_none_or_several_say_so() {
        assert_eq!(single(vec![7]), Ok(7));
        assert_eq!(single(Vec::<u8>::new()), Err(Target::Unknown));
        assert_eq!(single(vec![1, 2]), Err(Target::Ambiguous(2)));
    }

    #[test]
    fn a_refusal_is_final_and_a_fault_is_tried_again() {
        for final_code in [
            KeelError::validation("x"),
            KeelError::conflict("x"),
            KeelError::not_found("x"),
            KeelError::forbidden("x"),
        ] {
            assert!(!transient(&final_code), "{final_code}");
        }
        for fault in [
            KeelError::internal("x"),
            KeelError::unavailable("x"),
            KeelError::rate_limited("x"),
        ] {
            assert!(transient(&fault), "{fault}");
        }
    }
}
