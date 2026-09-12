use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::id::Id;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub id: Id,
    pub seq: u64,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(serialize_with = "millis")]
    pub at: DateTime<Utc>,
    pub instance_id: Id,
    pub product: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<EventActor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<EventSubject>,
    #[serde(default)]
    pub data: Value,
}

fn millis<S: serde::Serializer>(at: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EventActor {
    pub kind: EventActorKind,
    pub id: Id,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum EventActorKind {
    Principal,
    Peer,
    System,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EventSubject {
    pub kind: String,
    pub id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<Id>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn event_from_the_design_round_trips() {
        let wire = json!({
            "id": "01J9EV5K00000000000000000A", "seq": 8812, "type": "dns.zone.created", "at": "2026-09-12T10:00:00.000Z",
            "instanceId": "01J9B0000000000000000000A1", "product": "dns",
            "actor": { "kind": "peer", "id": "01J9A0000000000000000000A1" },
            "subject": { "kind": "dns.zone", "id": "01J9Q3W6B1R8N5X2K4T7M0V9Z3", "accountId": "01J8A0000000000000000000A1" },
            "data": { "name": "a.org" }
        });
        let event: Event = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(event.kind, "dns.zone.created");
        assert_eq!(event.actor.as_ref().unwrap().kind, EventActorKind::Peer);
        assert_eq!(event.at.to_rfc3339(), "2026-09-12T10:00:00+00:00");
        assert_eq!(serde_json::to_value(&event).unwrap(), wire);
    }

    #[test]
    fn whole_seconds_are_accepted_on_input_and_written_with_milliseconds() {
        let event: Event = serde_json::from_value(json!({
            "id": "01J9EV5K00000000000000000A", "seq": 1, "type": "x.y", "at": "2026-09-12T10:00:00Z",
            "instanceId": "01J9B0000000000000000000A1", "product": "dns", "data": null
        })).unwrap();
        assert_eq!(
            serde_json::to_value(&event).unwrap()["at"],
            json!("2026-09-12T10:00:00.000Z")
        );
    }

    #[test]
    fn unknown_actor_kind_fails_to_deserialise() {
        assert!(serde_json::from_value::<EventActorKind>(json!("robot")).is_err());
    }
}
