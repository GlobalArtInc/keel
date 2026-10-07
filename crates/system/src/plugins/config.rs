use std::time::Duration;

use keel_sdk::schemars::JsonSchema;
use keel_sdk::serde::Deserialize;

pub(super) const SECTION: &str = "plugin";

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", default, deny_unknown_fields)]
#[schemars(crate = "keel_sdk::schemars")]
pub(super) struct PluginConfig {
    #[schemars(range(min = 10, max = 600_000))]
    timeout_ms: u64,
    #[schemars(range(min = 1, max = 100))]
    failures: u32,
    #[schemars(range(min = 10, max = 86_400_000))]
    quiet_ms: u64,
    #[schemars(range(min = 10, max = 86_400_000))]
    liveness_ms: u64,
    #[schemars(range(min = 10, max = 3_600_000))]
    backoff_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Timings {
    pub(super) timeout: Duration,
    pub(super) failures: u32,
    pub(super) quiet: Duration,
    pub(super) liveness: Duration,
    pub(super) backoff: Duration,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 10_000,
            failures: 5,
            quiet_ms: 60_000,
            liveness_ms: 30_000,
            backoff_ms: 1_000,
        }
    }
}

impl PluginConfig {
    pub(super) fn timings(&self) -> Timings {
        Timings {
            timeout: Duration::from_millis(self.timeout_ms),
            failures: self.failures.max(1),
            quiet: Duration::from_millis(self.quiet_ms),
            liveness: Duration::from_millis(self.liveness_ms),
            backoff: Duration::from_millis(self.backoff_ms),
        }
    }
}

impl Timings {
    pub(super) fn pause(&self, failures: u32) -> Duration {
        if failures == 0 {
            return Duration::ZERO;
        }
        let doubled = self
            .backoff
            .saturating_mul(1_u32 << failures.saturating_sub(1).min(20));
        doubled.min(self.quiet.max(self.backoff))
    }
}

#[cfg(test)]
mod tests {
    use keel_sdk::serde_json::{self, json};

    use super::*;

    #[test]
    fn the_default_section_waits_ten_seconds_for_a_frame_and_opens_after_five_failures() {
        let timings = PluginConfig::default().timings();
        assert_eq!(timings.timeout, Duration::from_secs(10));
        assert_eq!(timings.failures, 5);
        assert_eq!(timings.quiet, Duration::from_secs(60));
        assert_eq!(timings.liveness, Duration::from_secs(30));
        assert_eq!(timings.backoff, Duration::from_secs(1));
        let parsed: PluginConfig =
            serde_json::from_value(json!({ "timeout_ms": 250, "failures": 2 })).unwrap();
        assert_eq!(parsed.timings().timeout, Duration::from_millis(250));
        assert_eq!(parsed.timings().failures, 2);
        assert_eq!(parsed.timings().quiet, Duration::from_secs(60));
        assert!(serde_json::from_value::<PluginConfig>(json!({ "timeout": 1 })).is_err());
    }

    #[test]
    fn the_restart_pause_doubles_from_the_backoff_up_to_the_quiet_period() {
        let timings = PluginConfig::default().timings();
        let pauses: Vec<u64> = (0..9)
            .map(|failures| timings.pause(failures).as_secs())
            .collect();
        assert_eq!(pauses, [0, 1, 2, 4, 8, 16, 32, 60, 60]);
        assert_eq!(timings.pause(u32::MAX), Duration::from_secs(60));
    }
}
