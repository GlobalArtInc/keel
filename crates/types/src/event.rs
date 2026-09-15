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
    pub module: String,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NotificationFrame {
    pub id: Id,
    pub key: String,
    #[serde(default)]
    pub params: Value,
    pub level: NotificationLevel,
    #[serde(serialize_with = "millis")]
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum NotificationLevel {
    Info,
    Warning,
    Error,
}

impl NotificationLevel {
    pub const ALL: [NotificationLevel; 3] = [
        NotificationLevel::Info,
        NotificationLevel::Warning,
        NotificationLevel::Error,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            NotificationLevel::Info => "info",
            NotificationLevel::Warning => "warning",
            NotificationLevel::Error => "error",
        }
    }
}

impl std::fmt::Display for NotificationLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for NotificationLevel {
    type Err = UnknownNotificationLevel;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        NotificationLevel::ALL
            .into_iter()
            .find(|level| level.as_str() == text)
            .ok_or_else(|| UnknownNotificationLevel(text.to_owned()))
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown notification level: {0}")]
pub struct UnknownNotificationLevel(String);

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn event_from_the_design_round_trips() {
        let wire = json!({
            "id": "01J9EV5K00000000000000000A", "seq": 8812, "type": "dns.zone.created", "at": "2026-09-12T10:00:00.000Z",
            "instanceId": "01J9B0000000000000000000A1", "module": "dns",
            "actor": { "kind": "peer", "id": "01J9A0000000000000000000A1" },
            "subject": { "kind": "dns.zone", "id": "01J9Q3W6B1R8N5X2K4T7M0V9Z3", "accountId": "01J8A0000000000000000000A1" },
            "data": { "name": "a.org" }
        });
        let event: Event = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(event.kind, "dns.zone.created");
        assert_eq!(event.module, "dns");
        assert_eq!(event.actor.as_ref().unwrap().kind, EventActorKind::Peer);
        assert_eq!(event.at.to_rfc3339(), "2026-09-12T10:00:00+00:00");
        assert_eq!(serde_json::to_value(&event).unwrap(), wire);
    }

    #[test]
    fn whole_seconds_are_accepted_on_input_and_written_with_milliseconds() {
        let event: Event = serde_json::from_value(json!({
            "id": "01J9EV5K00000000000000000A", "seq": 1, "type": "x.y", "at": "2026-09-12T10:00:00Z",
            "instanceId": "01J9B0000000000000000000A1", "module": "dns", "data": null
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

    #[test]
    fn a_notification_frame_carries_its_level_and_writes_its_time_with_milliseconds() {
        let frame: NotificationFrame = serde_json::from_value(json!({
            "id": "01J9EV5K00000000000000000A", "key": "notify.problem.raised",
            "params": { "key": "note.overdue" }, "level": "warning", "createdAt": "2026-09-12T10:00:00Z"
        }))
        .unwrap();
        assert_eq!(frame.level, NotificationLevel::Warning);
        assert_eq!(
            serde_json::to_value(&frame).unwrap(),
            json!({
                "id": "01J9EV5K00000000000000000A", "key": "notify.problem.raised",
                "params": { "key": "note.overdue" }, "level": "warning", "createdAt": "2026-09-12T10:00:00.000Z"
            })
        );
        for level in NotificationLevel::ALL {
            assert_eq!(level.as_str().parse::<NotificationLevel>().unwrap(), level);
            assert_eq!(level.to_string(), level.as_str());
            assert_eq!(serde_json::to_value(level).unwrap(), json!(level.as_str()));
        }
        assert_eq!(
            "loud".parse::<NotificationLevel>().unwrap_err().to_string(),
            "unknown notification level: loud"
        );
    }
}
