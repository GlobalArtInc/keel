use std::sync::Arc;

use keel_sdk::MailTemplate;
use keel_sdk::prelude::*;
use keel_sdk::system::{Caller, OutboxSubscriber, PrincipalSource, SecretCipher};
use tokio::sync::Notify;

use super::templates::Templates;

pub struct Seams {
    pub db: Db,
    pub clock: Arc<dyn Clock>,
    pub cipher: Arc<dyn SecretCipher>,
    pub caller: Arc<dyn Caller>,
    pub principals: Arc<dyn PrincipalSource>,
    pub subscriber: Arc<dyn OutboxSubscriber>,
    pub instance: Option<Id>,
}

pub struct Started {
    pub db: Db,
    pub clock: Arc<dyn Clock>,
    pub cipher: Arc<dyn SecretCipher>,
    pub caller: Arc<dyn Caller>,
    pub principals: Arc<dyn PrincipalSource>,
    pub subscriber: Arc<dyn OutboxSubscriber>,
    pub instance: Option<Id>,
    pub templates: Templates,
    wake: Notify,
}

impl Started {
    pub fn new<'a>(
        seams: Seams,
        contributed: impl IntoIterator<Item = &'a MailTemplate>,
    ) -> Result<Arc<Self>> {
        let templates = Templates::compile(contributed)?;
        Ok(Arc::new(Self {
            db: seams.db,
            clock: seams.clock,
            cipher: seams.cipher,
            caller: seams.caller,
            principals: seams.principals,
            subscriber: seams.subscriber,
            instance: seams.instance,
            templates,
            wake: Notify::new(),
        }))
    }

    pub fn wake(&self) {
        self.wake.notify_one();
    }

    pub async fn woken(&self) {
        self.wake.notified().await;
    }
}
