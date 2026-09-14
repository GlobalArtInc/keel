use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::envelope::{MetaActor, MetaPrincipal};
use crate::id::Id;
use crate::level::Level;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Principal {
    pub id: Id,
    pub login: String,
    pub level: Level,
    pub account_id: Id,
    pub role: String,
    pub actor: Option<Box<Principal>>,
    pub scope: Scope,
    pub via: Via,
    pub external_id: Option<String>,
    pub origin: Option<String>,
    pub lang: String,
    #[serde(skip)]
    pub grants: BTreeMap<String, bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Full,
    Functions(Vec<String>),
    ReadOnly,
}

const MASK_SUFFIX: &str = ".*";

impl Scope {
    pub fn admits(&self, function: &str) -> bool {
        match self {
            Scope::Full | Scope::ReadOnly => true,
            Scope::Functions(masks) => masks.iter().any(|mask| mask_matches(mask, function)),
        }
    }

    pub fn admits_write(&self) -> bool {
        !matches!(self, Scope::ReadOnly)
    }

    pub fn contains(&self, other: &Scope) -> bool {
        match (self, other) {
            (Scope::Full, _) => true,
            (Scope::Functions(_) | Scope::ReadOnly, Scope::Full) => false,
            (Scope::Functions(outer), Scope::Functions(inner)) => inner
                .iter()
                .all(|mask| outer.iter().any(|wider| mask_covers(wider, mask))),
            (Scope::Functions(_), Scope::ReadOnly) => false,
            (Scope::ReadOnly, Scope::ReadOnly) => true,
            (Scope::ReadOnly, Scope::Functions(_)) => false,
        }
    }
}

pub fn is_mask(text: &str) -> bool {
    let name = text.strip_suffix(MASK_SUFFIX).unwrap_or(text);
    !name.is_empty()
        && name.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        })
}

fn mask_matches(mask: &str, function: &str) -> bool {
    match mask.strip_suffix(MASK_SUFFIX) {
        Some(prefix) => function
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('.') && rest.len() > 1),
        None => mask == function,
    }
}

fn mask_covers(wider: &str, mask: &str) -> bool {
    match (
        wider.strip_suffix(MASK_SUFFIX),
        mask.strip_suffix(MASK_SUFFIX),
    ) {
        (None, None) => wider == mask,
        (None, Some(_)) => false,
        (Some(_), None) => mask_matches(wider, mask),
        (Some(outer), Some(inner)) => outer == inner || mask_matches(wider, inner),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Via {
    Http,
    Socket,
    Plugin(String),
    Peer(Id),
    Internal,
    Su,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ActorVia {
    Su,
    Socket,
    Peer,
}

#[derive(Debug, thiserror::Error)]
#[error("an actor cannot arrive via {0:?}: only su, socket and peer delegate")]
pub struct NotDelegated(pub Via);

impl TryFrom<&Via> for ActorVia {
    type Error = NotDelegated;

    fn try_from(via: &Via) -> Result<Self, Self::Error> {
        match via {
            Via::Su => Ok(ActorVia::Su),
            Via::Socket => Ok(ActorVia::Socket),
            Via::Peer(_) => Ok(ActorVia::Peer),
            Via::Http | Via::Plugin(_) | Via::Internal => Err(NotDelegated(via.clone())),
        }
    }
}

impl Principal {
    pub fn meta(&self) -> Result<MetaPrincipal, NotDelegated> {
        let actor = self
            .actor
            .as_deref()
            .map(|actor| {
                Ok(MetaActor {
                    id: actor.id,
                    login: actor.login.clone(),
                    level: actor.level,
                    via: ActorVia::try_from(&actor.via)?,
                })
            })
            .transpose()?;
        Ok(MetaPrincipal {
            id: self.id,
            login: self.login.clone(),
            level: self.level,
            account: self.account_id,
            actor,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: Id,
    pub kind: String,
    pub parent_id: Option<Id>,
    pub path: String,
    pub state: AccountState,
    pub external_id: Option<String>,
    pub origin: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum AccountState {
    Active,
    Suspended,
    Deleting,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn alice() -> Principal {
        Principal {
            id: "01J9Q3W6B1R8N5X2K4T7M0V9A1".parse().unwrap(),
            login: "alice".into(),
            level: Level::User,
            account_id: "01J8A0000000000000000000A1".parse().unwrap(),
            role: "owner".into(),
            actor: Some(Box::new(Principal {
                id: "01J8A0000000000000000000B2".parse().unwrap(),
                login: "root".into(),
                level: Level::Admin,
                account_id: "01J8A0000000000000000000A0".parse().unwrap(),
                role: "owner".into(),
                actor: None,
                scope: Scope::Full,
                via: Via::Su,
                external_id: None,
                origin: None,
                lang: "en".into(),
                grants: BTreeMap::new(),
            })),
            scope: Scope::Functions(vec!["dns.*".into()]),
            via: Via::Http,
            external_id: Some("client-42".into()),
            origin: Some("bill:01J9A".into()),
            lang: "ru".into(),
            grants: BTreeMap::new(),
        }
    }

    #[test]
    fn principal_round_trips_with_camel_case_names() {
        let wire = serde_json::to_value(alice()).unwrap();
        assert_eq!(wire["accountId"], json!("01J8A0000000000000000000A1"));
        assert_eq!(wire["externalId"], json!("client-42"));
        assert_eq!(wire["scope"], json!({ "functions": ["dns.*"] }));
        assert_eq!(wire["via"], json!("http"));
        assert_eq!(wire["actor"]["via"], json!("su"));
        assert_eq!(wire["actor"]["actor"], json!(null));
        let back: Principal = serde_json::from_value(wire).unwrap();
        assert_eq!(back, alice());
    }

    #[test]
    fn principal_projects_to_the_envelope_meta_shape() {
        let meta = serde_json::to_value(alice().meta().unwrap()).unwrap();
        assert_eq!(
            meta,
            json!({
                "id": "01J9Q3W6B1R8N5X2K4T7M0V9A1", "login": "alice", "level": "user",
                "account": "01J8A0000000000000000000A1",
                "actor": { "id": "01J8A0000000000000000000B2", "login": "root", "level": "admin", "via": "su" }
            })
        );
    }

    #[test]
    fn via_with_a_payload_serialises_as_a_tagged_object() {
        let peer: Id = "01J9A0000000000000000000A1".parse().unwrap();
        assert_eq!(
            serde_json::to_value(Via::Peer(peer)).unwrap(),
            json!({ "peer": "01J9A0000000000000000000A1" })
        );
        assert_eq!(
            serde_json::to_value(Via::Plugin("acme-sync".into())).unwrap(),
            json!({ "plugin": "acme-sync" })
        );
    }

    #[test]
    fn actor_via_is_the_delegation_mechanism_of_the_actor() {
        let peer: Id = "01J9A0000000000000000000A1".parse().unwrap();
        assert_eq!(ActorVia::try_from(&Via::Su).unwrap(), ActorVia::Su);
        assert_eq!(ActorVia::try_from(&Via::Socket).unwrap(), ActorVia::Socket);
        assert_eq!(
            ActorVia::try_from(&Via::Peer(peer)).unwrap(),
            ActorVia::Peer
        );
        assert_eq!(
            serde_json::to_value(ActorVia::Socket).unwrap(),
            json!("socket")
        );
        for via in [Via::Http, Via::Plugin("acme-sync".into()), Via::Internal] {
            assert!(ActorVia::try_from(&via).is_err());
        }
    }

    #[test]
    fn actor_via_on_the_wire_accepts_only_su_socket_and_peer() {
        for value in ["su", "socket", "peer"] {
            assert!(serde_json::from_value::<ActorVia>(json!(value)).is_ok());
        }
        for value in ["http", "plugin", "internal", "telnet"] {
            assert!(serde_json::from_value::<ActorVia>(json!(value)).is_err());
            let actor = json!({
                "id": "01J8A0000000000000000000B2", "login": "root", "level": "admin", "via": value
            });
            assert!(serde_json::from_value::<MetaActor>(actor).is_err());
        }
    }

    #[test]
    fn meta_refuses_an_actor_that_arrived_without_delegation() {
        let mut principal = alice();
        let actor = principal.actor.as_deref_mut().unwrap();
        actor.via = Via::Http;
        let error = principal.meta().unwrap_err();
        assert_eq!(error.0, Via::Http);
    }

    #[test]
    fn a_mask_is_an_exact_name_or_a_prefix_ending_in_dot_star() {
        assert!(mask_matches("dns.zone.edit", "dns.zone.edit"));
        assert!(!mask_matches("dns.zone.edit", "dns.zone.list"));
        assert!(!mask_matches("dns.zone", "dns.zone.edit"));
        assert!(mask_matches("svc.*", "svc.order"));
        assert!(mask_matches("svc.*", "svc.order.list"));
        assert!(!mask_matches("svc.*", "svc"));
        assert!(!mask_matches("svc.*", "svc."));
        assert!(!mask_matches("svc.*", "svcs.order"));
        assert!(!mask_matches("*", "svc.order"));
        for mask in ["svc.*", "svc.order", "keel.account_2.list", "a"] {
            assert!(is_mask(mask), "{mask}");
        }
        for mask in [
            "", "*", ".*", "svc.", "svc..x", "Svc.x", "svc.*.x", "svc x", "svc.**",
        ] {
            assert!(!is_mask(mask), "{mask}");
        }
    }

    #[test]
    fn a_functions_scope_admits_its_masks_and_read_only_refuses_writes() {
        let scoped = Scope::Functions(vec!["keel.account.list".into(), "svc.*".into()]);
        assert!(scoped.admits("keel.account.list"));
        assert!(scoped.admits("svc.order.list"));
        assert!(!scoped.admits("keel.principal.list"));
        assert!(scoped.admits_write());
        assert!(Scope::Full.admits("anything.at.all"));
        assert!(Scope::Full.admits_write());
        assert!(Scope::ReadOnly.admits("anything.at.all"));
        assert!(!Scope::ReadOnly.admits_write());
        assert!(!Scope::Functions(Vec::new()).admits("keel.account.list"));
    }

    #[test]
    fn a_scope_contains_another_only_when_every_mask_is_covered() {
        let full = Scope::Full;
        let keel = Scope::Functions(vec!["keel.*".into()]);
        let list = Scope::Functions(vec!["keel.account.list".into()]);
        let accounts = Scope::Functions(vec!["keel.account.*".into()]);
        let auth = Scope::Functions(vec!["auth.*".into()]);
        assert!(full.contains(&full));
        assert!(full.contains(&keel));
        assert!(full.contains(&Scope::ReadOnly));
        assert!(!keel.contains(&full));
        assert!(keel.contains(&keel));
        assert!(keel.contains(&list));
        assert!(keel.contains(&accounts));
        assert!(!accounts.contains(&keel));
        assert!(!keel.contains(&auth));
        assert!(list.contains(&list));
        assert!(!list.contains(&accounts));
        assert!(!keel.contains(&Scope::ReadOnly));
        assert!(Scope::ReadOnly.contains(&Scope::ReadOnly));
        assert!(!Scope::ReadOnly.contains(&list));
        assert!(!Scope::ReadOnly.contains(&full));
        assert!(mask_covers("keel.*", "keel.*"));
        assert!(mask_covers("keel.*", "keel.account.*"));
        assert!(!mask_covers("keel.account.*", "keel.*"));
        assert!(!mask_covers("keel.account.list", "keel.account.*"));
        assert!(!mask_covers("keel.account.list", "keel.account.get"));
    }

    #[test]
    fn unknown_scope_via_and_account_state_fail_to_deserialise() {
        assert!(serde_json::from_value::<Scope>(json!("everything")).is_err());
        assert!(serde_json::from_value::<Via>(json!("telnet")).is_err());
        assert!(serde_json::from_value::<AccountState>(json!("closed")).is_err());
    }

    #[test]
    fn account_round_trips() {
        let account = Account {
            id: "01J8A0000000000000000000A1".parse().unwrap(),
            kind: "reseller".into(),
            parent_id: Some("01J8A0000000000000000000A0".parse().unwrap()),
            path: "/01J8A0000000000000000000A1/".into(),
            state: AccountState::Active,
            external_id: None,
            origin: None,
        };
        let wire = serde_json::to_value(&account).unwrap();
        assert_eq!(wire["parentId"], json!("01J8A0000000000000000000A0"));
        assert_eq!(wire["state"], json!("active"));
        assert_eq!(serde_json::from_value::<Account>(wire).unwrap(), account);
    }
}
