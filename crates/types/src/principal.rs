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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Full,
    Functions(Vec<String>),
    ReadOnly,
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
            })),
            scope: Scope::Functions(vec!["dns.*".into()]),
            via: Via::Http,
            external_id: Some("client-42".into()),
            origin: Some("bill:01J9A".into()),
            lang: "ru".into(),
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
