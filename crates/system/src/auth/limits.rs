use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};

use keel_sdk::async_trait::async_trait;
use keel_sdk::chrono::{DateTime, Duration, Utc};
use keel_sdk::serde_json::Value;
use keel_sdk::system::CallExt;
use keel_sdk::types::Level;
use keel_sdk::{Call, Clock, Guard, KeelError, Result};

use super::policy::{SETTING_RATE_GUEST_PER_MINUTE, lockout_policy};

pub const GUEST_PER_MINUTE: u32 = 30;
pub const LOCKOUT_THRESHOLD: u32 = 5;
pub const LOCKOUT_MINUTES: i64 = 15;

#[derive(Default)]
pub struct RateLimit {
    buckets: Mutex<HashMap<IpAddr, Bucket>>,
}

struct Bucket {
    tokens: u32,
    refill_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LockoutPolicy {
    pub threshold: u32,
    pub window: Duration,
}

impl RateLimit {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.buckets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn exhausted(&self, ip: IpAddr, now: DateTime<Utc>) -> bool {
        self.buckets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&ip.to_canonical())
            .is_some_and(|bucket| bucket.refill_at > now && bucket.tokens == 0)
    }

    pub fn admit(&self, ip: IpAddr, now: DateTime<Utc>, limit: u32) -> bool {
        let mut buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        buckets.retain(|_, bucket| bucket.refill_at > now);
        let bucket = buckets.entry(ip.to_canonical()).or_insert(Bucket {
            tokens: limit,
            refill_at: now + Duration::minutes(1),
        });
        if bucket.tokens == 0 {
            return false;
        }
        bucket.tokens -= 1;
        true
    }
}

#[async_trait]
impl Guard for RateLimit {
    async fn check(&self, _: &str, call: &Call<Value>) -> Result<()> {
        if call.principal().level != Level::Guest {
            return Ok(());
        }
        let Some(ip) = call.client().and_then(|client| client.ip) else {
            return Ok(());
        };
        let now = call.require::<dyn Clock>()?.now();
        let limit = call.setting::<u32>(SETTING_RATE_GUEST_PER_MINUTE).await?;
        if self.admit(ip, now, limit) {
            Ok(())
        } else {
            Err(KeelError::rate_limited("auth.error.rate_limited"))
        }
    }
}

#[derive(Default)]
pub struct Lockout {
    pairs: Mutex<HashMap<Pair, Failures>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Pair {
    login: String,
    ip: Option<IpAddr>,
}

struct Failures {
    count: u32,
    until: DateTime<Utc>,
}

impl LockoutPolicy {
    pub fn new(threshold: u32, minutes: i64) -> Self {
        Self {
            threshold,
            window: Duration::minutes(minutes),
        }
    }
}

impl Default for LockoutPolicy {
    fn default() -> Self {
        Self::new(LOCKOUT_THRESHOLD, LOCKOUT_MINUTES)
    }
}

impl Lockout {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.pairs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn locked(
        &self,
        login: &str,
        ip: Option<IpAddr>,
        now: DateTime<Utc>,
        policy: LockoutPolicy,
    ) -> bool {
        let mut pairs = self.pairs.lock().unwrap_or_else(PoisonError::into_inner);
        pairs.retain(|_, failures| failures.until > now);
        pairs
            .get(&pair(login, ip))
            .is_some_and(|failures| failures.count >= policy.threshold)
    }

    pub fn failed(
        &self,
        login: &str,
        ip: Option<IpAddr>,
        now: DateTime<Utc>,
        policy: LockoutPolicy,
    ) {
        let mut pairs = self.pairs.lock().unwrap_or_else(PoisonError::into_inner);
        pairs.retain(|_, failures| failures.until > now);
        let failures = pairs.entry(pair(login, ip)).or_insert(Failures {
            count: 0,
            until: now + policy.window,
        });
        failures.count += 1;
        if failures.count >= policy.threshold {
            failures.until = now + policy.window;
        }
    }

    pub async fn check<P>(
        &self,
        call: &Call<P>,
        login: &str,
        ip: Option<IpAddr>,
        now: DateTime<Utc>,
    ) -> Result<LockoutPolicy> {
        let policy = lockout_policy(call).await?;
        if self.locked(login, ip, now, policy) {
            return Err(locked());
        }
        Ok(policy)
    }

    pub fn succeeded(&self, login: &str, ip: Option<IpAddr>) {
        self.pairs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&pair(login, ip));
    }

    pub fn unlock(&self, login: &str) -> usize {
        let mut pairs = self.pairs.lock().unwrap_or_else(PoisonError::into_inner);
        let before = pairs.len();
        pairs.retain(|pair, _| pair.login != login);
        before - pairs.len()
    }
}

fn pair(login: &str, ip: Option<IpAddr>) -> Pair {
    Pair {
        login: login.to_owned(),
        ip: ip.map(|ip| ip.to_canonical()),
    }
}

pub fn locked() -> KeelError {
    KeelError::forbidden("auth.error.locked")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        text.parse().expect("an instant")
    }

    fn ip(text: &str) -> IpAddr {
        text.parse().expect("an address")
    }

    #[test]
    fn a_bucket_holds_the_limit_per_minute_and_leaves_once_idle() {
        let limit = RateLimit::new();
        let start = at("2026-09-13T10:00:00Z");
        let local = ip("127.0.0.1");
        let other = ip("10.1.1.1");
        assert!(limit.is_empty());
        for _ in 0..3 {
            assert!(limit.admit(local, start, 3));
        }
        assert!(!limit.admit(local, start + Duration::seconds(59), 3));
        assert!(limit.admit(other, start + Duration::seconds(59), 3));
        assert!(limit.admit(ip("::ffff:10.1.1.1"), start + Duration::seconds(59), 3));
        assert_eq!(limit.len(), 2);
        assert!(limit.admit(local, start + Duration::minutes(1), 3));
        assert_eq!(limit.len(), 2);
        assert!(limit.admit(local, start + Duration::minutes(2), 3));
        assert_eq!(limit.len(), 1);
    }

    #[test]
    fn a_bucket_reads_exhausted_once_spent_without_spending_and_until_it_refills() {
        let limit = RateLimit::new();
        let start = at("2026-09-13T10:00:00Z");
        let local = ip("127.0.0.1");
        assert!(!limit.exhausted(local, start));
        assert!(limit.is_empty());
        assert!(limit.admit(local, start, 2));
        assert!(!limit.exhausted(local, start));
        assert!(limit.admit(local, start, 2));
        assert!(limit.exhausted(local, start));
        assert!(limit.exhausted(ip("::ffff:127.0.0.1"), start + Duration::seconds(59)));
        assert!(!limit.exhausted(ip("10.1.1.1"), start));
        assert!(!limit.exhausted(local, start + Duration::minutes(1)));
        assert!(!limit.admit(local, start + Duration::seconds(30), 2));
        assert!(limit.exhausted(local, start + Duration::seconds(30)));
    }

    #[test]
    fn five_failures_lock_a_pair_for_the_window_and_the_pair_alone() {
        let lockout = Lockout::new();
        let policy = LockoutPolicy::default();
        assert_eq!(policy, LockoutPolicy::new(5, 15));
        let start = at("2026-09-13T10:00:00Z");
        let attacker = Some(ip("10.1.1.1"));
        let victim = Some(ip("127.0.0.1"));
        for _ in 0..4 {
            lockout.failed("a1", attacker, start, policy);
            assert!(!lockout.locked("a1", attacker, start, policy));
        }
        lockout.failed("a1", attacker, start + Duration::minutes(10), policy);
        assert!(lockout.locked("a1", attacker, start + Duration::minutes(10), policy));
        assert!(lockout.locked(
            "a1",
            Some(ip("::ffff:10.1.1.1")),
            start + Duration::minutes(24),
            policy
        ));
        assert!(!lockout.locked("a1", victim, start + Duration::minutes(10), policy));
        assert!(!lockout.locked("a2", attacker, start + Duration::minutes(10), policy));
        assert!(!lockout.locked("a1", attacker, start + Duration::minutes(25), policy));
        assert_eq!(lockout.len(), 0);
        let stricter = LockoutPolicy::new(3, 15);
        lockout.failed("a1", attacker, start, stricter);
        lockout.failed("a1", attacker, start, stricter);
        assert!(!lockout.locked("a1", attacker, start, stricter));
        lockout.failed("a1", attacker, start, stricter);
        assert!(lockout.locked("a1", attacker, start, stricter));
        assert!(!lockout.locked("a1", attacker, start, policy));
    }

    #[test]
    fn a_success_clears_the_pair_and_unlock_clears_every_pair_of_the_login() {
        let lockout = Lockout::new();
        let policy = LockoutPolicy::new(2, 15);
        let now = at("2026-09-13T10:00:00Z");
        lockout.failed("a1", Some(ip("10.1.1.1")), now, policy);
        lockout.failed("a1", Some(ip("10.1.1.2")), now, policy);
        lockout.failed("a1", None, now, policy);
        lockout.failed("nobody", Some(ip("10.1.1.1")), now, policy);
        assert_eq!(lockout.len(), 4);
        lockout.succeeded("a1", Some(ip("10.1.1.2")));
        assert_eq!(lockout.len(), 3);
        assert_eq!(lockout.unlock("a1"), 2);
        assert_eq!(lockout.len(), 1);
        assert_eq!(lockout.unlock("a1"), 0);
        assert!(!lockout.is_empty());
        assert_eq!(locked().key(), "auth.error.locked");
    }
}
