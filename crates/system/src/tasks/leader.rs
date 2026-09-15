use keel_sdk::prelude::*;
use keel_sdk::system::{AdvisoryLock, LockGuard};

pub const LOCK_KEY: i64 = i64::from_be_bytes(*b"keeltask");

pub async fn hold(lock: &dyn AdvisoryLock) -> Result<LockGuard> {
    lock.take(LOCK_KEY).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_worker_key_is_the_eight_ascii_bytes_of_keeltask() {
        assert_eq!(LOCK_KEY.to_be_bytes(), *b"keeltask");
        assert_ne!(LOCK_KEY, i64::from_be_bytes(*b"keelcron"));
    }
}
