use chrono::{DateTime, Utc};

pub use keel_sdk::Clock;

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_clock_follows_wall_time() {
        let before = Utc::now();
        let read = SystemClock.now();
        let after = Utc::now();
        assert!(before <= read && read <= after);
    }
}
