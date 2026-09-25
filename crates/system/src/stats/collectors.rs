use std::collections::BTreeSet;
use std::sync::OnceLock;

use keel_sdk::StatCollector;
use keel_sdk::prelude::*;

const ERROR_INTERVAL: &str = "stats.error.interval";
const ERROR_DUPLICATE: &str = "stats.error.duplicate";
const ERROR_NOT_STARTED: &str = "stats.error.not_started";
const SECONDS_PER_MINUTE: u64 = 60;

pub(super) struct Collector {
    pub(super) def: StatCollector,
    minutes: i64,
}

impl Collector {
    fn of(def: &StatCollector) -> Result<Self> {
        let interval = def.interval();
        let seconds = interval.as_secs();
        let minutes = seconds / SECONDS_PER_MINUTE;
        if interval.subsec_nanos() != 0
            || !seconds.is_multiple_of(SECONDS_PER_MINUTE)
            || minutes == 0
        {
            return Err(KeelError::internal(ERROR_INTERVAL)
                .param("key", def.key())
                .param("seconds", seconds));
        }
        let minutes = i64::try_from(minutes).map_err(|_| {
            KeelError::internal(ERROR_INTERVAL)
                .param("key", def.key())
                .param("seconds", seconds)
        })?;
        Ok(Self {
            def: def.clone(),
            minutes,
        })
    }

    pub(super) fn is_due(&self, minute: i64) -> bool {
        minute.rem_euclid(self.minutes) == 0
    }
}

struct Started {
    collectors: Vec<Collector>,
    settings: Arc<dyn SettingsSource>,
}

#[derive(Default)]
pub(super) struct Stats {
    started: OnceLock<Started>,
}

impl Stats {
    pub(super) fn start(&self, ctx: &Context) -> Result<()> {
        let mut seen = BTreeSet::new();
        let mut collectors = Vec::new();
        for def in ctx.contributions::<StatCollector>() {
            if !seen.insert(def.key().to_owned()) {
                return Err(KeelError::internal(ERROR_DUPLICATE).param("key", def.key()));
            }
            collectors.push(Collector::of(def)?);
        }
        collectors.sort_by(|left, right| left.def.key().cmp(right.def.key()));
        let started = Started {
            collectors,
            settings: ctx.require::<dyn SettingsSource>()?,
        };
        if self.started.set(started).is_err() {
            tracing::warn!("the stats module was started twice; the first start stands");
        }
        Ok(())
    }

    fn started(&self) -> Result<&Started> {
        self.started
            .get()
            .ok_or_else(|| KeelError::unavailable(ERROR_NOT_STARTED))
    }

    pub(super) fn collectors(&self) -> Result<&[Collector]> {
        Ok(&self.started()?.collectors)
    }

    pub(super) fn find(&self, key: &str) -> Result<Option<&Collector>> {
        Ok(self
            .collectors()?
            .iter()
            .find(|collector| collector.def.key() == key))
    }

    pub(super) fn settings(&self) -> Result<&dyn SettingsSource> {
        Ok(self.started()?.settings.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use keel_sdk::limit::Unit;

    use super::*;

    fn collector(seconds: u64) -> StatCollector {
        StatCollector::new(
            "probe.count",
            Unit::Count,
            Duration::from_secs(seconds),
            |_| async { Ok(1.0) },
        )
    }

    #[test]
    fn an_interval_is_a_whole_number_of_minutes_and_decides_the_minutes_sampled() {
        let every_five = Collector::of(&collector(300)).unwrap();
        assert_eq!(every_five.minutes, 5);
        assert!(every_five.is_due(0));
        assert!(every_five.is_due(10));
        assert!(!every_five.is_due(11));
        for seconds in [0, 30, 90] {
            let refused = Collector::of(&collector(seconds)).err().unwrap();
            assert_eq!(refused.key(), ERROR_INTERVAL);
            assert_eq!(refused.params()["seconds"], json!(seconds));
        }
        let fractional = StatCollector::new(
            "probe.count",
            Unit::Count,
            Duration::from_millis(60_500),
            |_| async { Ok(1.0) },
        );
        assert!(Collector::of(&fractional).is_err());
    }
}
