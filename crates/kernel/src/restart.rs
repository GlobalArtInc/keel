use std::fmt;
use std::sync::{Mutex, PoisonError};

use chrono::{DateTime, Utc};
use keel_types::Id;
use tokio::sync::Notify;

pub const EXIT_STATUS: u8 = 75;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Requested {
    pub reason: String,
    pub requested_by: String,
    pub task: Option<Id>,
    pub at: DateTime<Utc>,
}

pub struct Handle {
    booted_at: DateTime<Utc>,
    pending: Mutex<Option<Requested>>,
    arrived: Notify,
}

impl Handle {
    pub fn new(booted_at: DateTime<Utc>) -> Self {
        Self {
            booted_at,
            pending: Mutex::new(None),
            arrived: Notify::new(),
        }
    }

    pub fn booted_at(&self) -> DateTime<Utc> {
        self.booted_at
    }

    pub fn request(&self, requested: Requested) -> Result<(), Requested> {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(first) = pending.as_ref() {
            return Err(first.clone());
        }
        *pending = Some(requested);
        drop(pending);
        self.arrived.notify_waiters();
        Ok(())
    }

    pub fn pending(&self) -> Option<Requested> {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub async fn requested(&self) -> Requested {
        loop {
            let arrived = self.arrived.notified();
            if let Some(requested) = self.pending() {
                return requested;
            }
            arrived.await;
        }
    }
}

impl fmt::Debug for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Handle")
            .field("booted_at", &self.booted_at)
            .field("pending", &self.pending())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;

    fn requested(reason: &str) -> Requested {
        Requested {
            reason: reason.to_owned(),
            requested_by: "alice".to_owned(),
            task: None,
            at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn the_first_request_stands_and_wakes_the_waiter() {
        let handle = Arc::new(Handle::new(Utc::now()));
        assert!(handle.pending().is_none());
        let waiter = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.requested().await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        handle.request(requested("product enable dns")).unwrap();
        let second = handle
            .request(requested("product disable dns"))
            .unwrap_err();
        assert_eq!(second.reason, "product enable dns");
        let woken = tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("the waiter wakes")
            .expect("the waiter finishes");
        assert_eq!(woken.reason, "product enable dns");
        assert_eq!(
            handle.pending().map(|pending| pending.requested_by),
            Some("alice".to_owned())
        );
        assert!(format!("{handle:?}").contains("product enable dns"));
    }

    #[tokio::test]
    async fn a_request_made_before_the_wait_is_seen_at_once() {
        let handle = Handle::new(Utc::now());
        handle.request(requested("restart")).unwrap();
        let seen = tokio::time::timeout(Duration::from_millis(100), handle.requested())
            .await
            .expect("a pending request resolves at once");
        assert_eq!(seen.reason, "restart");
        assert_eq!(EXIT_STATUS, 75);
    }
}
