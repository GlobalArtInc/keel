use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use keel_sdk::system::SecretCipher;

use super::entity::subscription;
use super::functions::subscriptions::ERROR_PEER;

pub const HEADER: &str = "x-keel-peer";
pub const NAME_PREFIX: &str = "peer-";

#[derive(Clone, Copy, Debug)]
pub struct PeerSubscription<'a> {
    pub link: Id,
    pub principal: Id,
    pub level: Level,
    pub url: &'a str,
    pub secret: &'a str,
    pub events: &'a [String],
    pub floor: u64,
}

pub fn name_of(link: Id) -> String {
    format!("{NAME_PREFIX}{}", link.to_string().to_lowercase())
}

pub async fn subscribe(
    db: &Db,
    cipher: &dyn SecretCipher,
    peer: PeerSubscription<'_>,
    now: DateTime<Utc>,
) -> Result<Id> {
    let id = Id::generate();
    let name = name_of(peer.link);
    subscription::ActiveModel {
        id: Set(id.to_string()),
        account_id: Set(Id::ROOT_ACCOUNT.to_string()),
        principal_id: Set(peer.principal.to_string()),
        name: Set(name.clone()),
        url: Set(peer.url.to_owned()),
        secret: Set(cipher.seal(peer.secret.as_bytes())?),
        events: Set(peer.events.to_vec()),
        enabled: Set(true),
        created_at: Set(now),
        created_by: Set(name),
        updated_at: Set(now),
        peer_id: Set(Some(peer.link.to_string())),
        level: Set(Some(peer.level.as_str().to_owned())),
        floor_seq: Set(Some(i64::try_from(peer.floor).unwrap_or(i64::MAX))),
        local_only: Set(true),
    }
    .insert(db)
    .await?;
    Ok(id)
}

pub async fn unsubscribe(db: &Db, link: Id) -> Result<u64> {
    let deleted = subscription::Entity::delete_many()
        .filter(subscription::Column::PeerId.eq(link.to_string()))
        .exec(db)
        .await?;
    Ok(deleted.rows_affected)
}

pub fn owned_by_link(row: &subscription::Model) -> Result<()> {
    match &row.peer_id {
        Some(link) => Err(KeelError::conflict(ERROR_PEER)
            .param("id", row.id.as_str())
            .param("link", link.as_str())),
        None => Ok(()),
    }
}

pub fn scope_level(row: &subscription::Model) -> Option<Level> {
    row.level.as_deref().and_then(|level| level.parse().ok())
}

pub fn below_floor(row: &subscription::Model, seq: u64) -> bool {
    row.floor_seq
        .is_some_and(|floor| i64::try_from(seq).is_ok_and(|seq| seq <= floor))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(peer: Option<&str>, level: Option<&str>) -> subscription::Model {
        let now = Utc::now();
        subscription::Model {
            id: "01J9S00000000000000000000S".into(),
            account_id: Id::ROOT_ACCOUNT.to_string(),
            principal_id: "01J9P00000000000000000000P".into(),
            name: "peer".into(),
            url: "https://bill.example/api/v1/x/peer/hook".into(),
            secret: "sealed".into(),
            events: vec!["dns.zone.*".into()],
            enabled: true,
            created_at: now,
            created_by: "peer".into(),
            updated_at: now,
            peer_id: peer.map(str::to_owned),
            level: level.map(str::to_owned),
            floor_seq: None,
            local_only: peer.is_some(),
        }
    }

    #[test]
    fn a_subscription_with_a_floor_skips_the_events_at_or_below_it() {
        let mut plain = row(None, None);
        assert!(!below_floor(&plain, 0));
        assert!(!below_floor(&plain, u64::MAX));
        plain.floor_seq = Some(8812);
        assert!(below_floor(&plain, 1));
        assert!(below_floor(&plain, 8812));
        assert!(!below_floor(&plain, 8813));
        assert!(!below_floor(&plain, u64::MAX));
    }

    #[test]
    fn a_peer_subscription_is_named_after_its_link_and_refuses_changes_outside_it() {
        let link: Id = "01J9K00000000000000000000K".parse().unwrap();
        assert_eq!(name_of(link), "peer-01j9k00000000000000000000k");
        owned_by_link(&row(None, None)).unwrap();
        let refused =
            owned_by_link(&row(Some("01J9K00000000000000000000K"), Some("admin"))).unwrap_err();
        assert_eq!(refused.key(), ERROR_PEER);
        assert_eq!(refused.code(), keel_sdk::types::ErrorCode::Conflict);
        assert_eq!(
            refused.params()["link"],
            json!("01J9K00000000000000000000K")
        );
        assert_eq!(scope_level(&row(None, Some("admin"))), Some(Level::Admin));
        assert_eq!(scope_level(&row(None, Some("nonsense"))), None);
        assert_eq!(scope_level(&row(None, None)), None);
    }
}
