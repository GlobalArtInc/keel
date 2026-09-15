use std::fmt;
use std::sync::{Arc, OnceLock};

use keel_sdk::prelude::*;
use keel_sdk::system::{Caller, OutboxSubscriber, PrincipalSource, SecretCipher};
use tokio::sync::Notify;

use super::templates::{MailTemplate, Templates};

#[derive(Default)]
pub struct Mailer {
    wake: Notify,
    started: OnceLock<Arc<Started>>,
}

pub struct Seams {
    pub db: Db,
    pub clock: Arc<dyn Clock>,
    pub cipher: Arc<dyn SecretCipher>,
    pub caller: Arc<dyn Caller>,
    pub principals: Arc<dyn PrincipalSource>,
    pub subscriber: Arc<dyn OutboxSubscriber>,
}

pub struct Started {
    pub db: Db,
    pub clock: Arc<dyn Clock>,
    pub cipher: Arc<dyn SecretCipher>,
    pub caller: Arc<dyn Caller>,
    pub principals: Arc<dyn PrincipalSource>,
    pub subscriber: Arc<dyn OutboxSubscriber>,
    pub templates: Templates,
}

impl Mailer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn wake(&self) {
        self.wake.notify_one();
    }

    pub async fn woken(&self) {
        self.wake.notified().await;
    }

    pub fn start<'a>(
        &self,
        seams: Seams,
        contributed: impl IntoIterator<Item = &'a MailTemplate>,
    ) -> Result<Arc<Started>> {
        let templates = Templates::compile(contributed)?;
        let started = Arc::new(Started {
            db: seams.db,
            clock: seams.clock,
            cipher: seams.cipher,
            caller: seams.caller,
            principals: seams.principals,
            subscriber: seams.subscriber,
            templates,
        });
        if self.started.set(started.clone()).is_err() {
            tracing::warn!("the notify mailer was started twice; the first start stays");
        }
        Ok(started)
    }
}

impl fmt::Debug for Mailer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mailer")
            .field("started", &self.started.get().is_some())
            .finish()
    }
}

impl fmt::Debug for Started {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Started")
            .field("templates", &self.templates)
            .finish()
    }
}
