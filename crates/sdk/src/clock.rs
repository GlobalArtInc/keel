use chrono::{DateTime, Utc};

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Noon;

    impl Clock for Noon {
        fn now(&self) -> DateTime<Utc> {
            "2026-09-12T12:00:00Z".parse().expect("a valid instant")
        }
    }

    #[test]
    fn a_clock_is_an_object_safe_service() {
        let clock: std::sync::Arc<dyn Clock> = std::sync::Arc::new(Noon);
        assert_eq!(clock.now().to_rfc3339(), "2026-09-12T12:00:00+00:00");
    }
}
