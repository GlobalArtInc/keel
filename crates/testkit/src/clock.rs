use std::sync::{Mutex, PoisonError};

use chrono::{DateTime, Duration, DurationRound, Utc};
use keel_kernel::Clock;

#[derive(Debug)]
pub struct FixedClock {
    at: Mutex<DateTime<Utc>>,
}

impl FixedClock {
    pub fn at(at: DateTime<Utc>) -> Self {
        Self { at: Mutex::new(at) }
    }

    pub fn frozen_now() -> Self {
        let now = Utc::now();
        Self::at(now.duration_trunc(Duration::seconds(1)).unwrap_or(now))
    }

    pub fn now(&self) -> DateTime<Utc> {
        *self.at.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn set(&self, at: DateTime<Utc>) {
        *self.at.lock().unwrap_or_else(PoisonError::into_inner) = at;
    }

    pub fn advance(&self, by: Duration) {
        let mut at = self.at.lock().unwrap_or_else(PoisonError::into_inner);
        *at += by;
    }
}

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        FixedClock::now(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fixed_clock_stands_still_until_it_is_moved() {
        let start = "2026-09-12T10:00:00Z"
            .parse::<DateTime<Utc>>()
            .expect("a valid instant");
        let clock = FixedClock::at(start);
        assert_eq!(clock.now(), start);
        assert_eq!(clock.now(), start);
        clock.advance(Duration::minutes(5));
        assert_eq!(clock.now(), start + Duration::minutes(5));
        clock.set(start);
        assert_eq!(clock.now(), start);
    }

    #[test]
    fn the_frozen_clock_starts_on_a_whole_second_near_wall_time() {
        let clock = FixedClock::frozen_now();
        let now = clock.now();
        assert_eq!(now.timestamp_subsec_nanos(), 0);
        assert!((Utc::now() - now) < Duration::seconds(2));
    }
}
