use std::sync::Arc;

use keel_sdk::prelude::*;
use keel_sdk::system::{Caller, OutboxSubscriber, PrincipalSource, SecretCipher};
use tokio::sync::Notify;

use super::targets::Resolver;

pub const PRIVATE_TARGETS_KEY: &str = "webhook.private_targets";

pub struct Seams {
    pub db: Db,
    pub clock: Arc<dyn Clock>,
    pub cipher: Arc<dyn SecretCipher>,
    pub caller: Arc<dyn Caller>,
    pub principals: Arc<dyn PrincipalSource>,
    pub subscriber: Arc<dyn OutboxSubscriber>,
    pub settings: Arc<dyn SettingsSource>,
    pub resolver: Arc<dyn Resolver>,
    pub instance: Option<Id>,
}

pub struct Started {
    pub seams: Seams,
    wake: Notify,
}

impl Started {
    pub fn new(seams: Seams) -> Arc<Self> {
        Arc::new(Self {
            seams,
            wake: Notify::new(),
        })
    }

    pub fn db(&self) -> &Db {
        &self.seams.db
    }

    pub fn wake(&self) {
        self.wake.notify_one();
    }

    pub async fn woken(&self) {
        self.wake.notified().await;
    }

    pub async fn private_targets(&self) -> Result<bool> {
        private_targets(self.seams.settings.as_ref(), &self.seams.db).await
    }

    pub fn emitted_here(&self, instance: Id) -> bool {
        self.seams.instance.is_none_or(|local| local == instance)
    }
}

pub async fn private_targets(settings: &dyn SettingsSource, db: &Db) -> Result<bool> {
    let value = settings
        .resolve(PRIVATE_TARGETS_KEY, Id::ROOT_ACCOUNT, db)
        .await?;
    Ok(value.and_then(|value| value.as_bool()).unwrap_or(false))
}
