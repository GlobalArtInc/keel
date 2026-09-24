use std::path::PathBuf;
use std::time::Duration;

use keel_sdk::schemars::JsonSchema;
use keel_sdk::serde::Deserialize;

pub(crate) const SECTION: &str = "host";
const SOCKET: &str = "/run/keel/agent.sock";
const TIMEOUT_SECONDS: u64 = 60;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", default, deny_unknown_fields)]
#[schemars(crate = "keel_sdk::schemars")]
pub(crate) struct HostConfig {
    #[schemars(length(min = 2, max = 100), pattern(r"^/"))]
    socket: String,
    #[schemars(range(min = 1, max = 3600))]
    timeout_seconds: u64,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            socket: SOCKET.to_owned(),
            timeout_seconds: TIMEOUT_SECONDS,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub(crate) socket: PathBuf,
    pub(crate) timeout: Duration,
}

impl HostConfig {
    pub(crate) fn target(&self) -> Target {
        Target {
            socket: PathBuf::from(&self.socket),
            timeout: Duration::from_secs(self.timeout_seconds),
        }
    }
}

#[cfg(test)]
mod tests {
    use keel_sdk::ConfigSection;
    use keel_sdk::serde_json::{self, json};

    use super::*;

    #[test]
    fn the_default_section_is_the_agent_socket_of_the_layout_and_a_minute() {
        let target = HostConfig::default().target();
        assert_eq!(target.socket, PathBuf::from("/run/keel/agent.sock"));
        assert_eq!(target.timeout, Duration::from_secs(60));
        let parsed: HostConfig =
            serde_json::from_value(json!({ "socket": "/tmp/a.sock" })).unwrap();
        assert_eq!(parsed.target().socket, PathBuf::from("/tmp/a.sock"));
        assert_eq!(parsed.target().timeout, Duration::from_secs(60));
        assert!(serde_json::from_value::<HostConfig>(json!({ "retries": 3 })).is_err());
    }

    #[test]
    fn the_schema_asks_for_an_absolute_socket_and_a_timeout_between_a_second_and_an_hour() {
        let section = ConfigSection::new::<HostConfig>(SECTION);
        let schema = section.schema().as_value();
        let socket = &schema["properties"]["socket"];
        assert_eq!(socket["pattern"], "^/");
        assert_eq!(socket["maxLength"], 100);
        let timeout = &schema["properties"]["timeout_seconds"];
        assert_eq!(timeout["minimum"], 1);
        assert_eq!(timeout["maximum"], 3600);
    }
}
