use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::response::Response;
use keel_sdk::KeelError;
use keel_sdk::system::{Authenticator, OnBehalf};

use super::Shared;
use super::reply;
use crate::dispatcher::{Dispatcher, LiveBus, OutboxFeed};
use crate::registry::Registry;

#[derive(Clone)]
pub(super) struct Resolvers {
    pub(super) authenticators: Arc<[Arc<dyn Authenticator>]>,
    pub(super) delegates: Arc<[Arc<dyn OnBehalf>]>,
}

impl Resolvers {
    fn of(registry: &Registry) -> Self {
        Self {
            authenticators: registry
                .contributions()
                .of::<Arc<dyn Authenticator>>()
                .cloned()
                .collect(),
            delegates: registry
                .contributions()
                .of::<Arc<dyn OnBehalf>>()
                .cloned()
                .collect(),
        }
    }
}

#[derive(Clone)]
pub(super) struct Live {
    pub(super) dispatcher: Arc<Dispatcher>,
    pub(super) registry: Arc<Registry>,
    pub(super) resolvers: Resolvers,
    pub(super) bus: Option<Arc<LiveBus>>,
    pub(super) outbox: Option<Arc<OutboxFeed>>,
}

impl Live {
    pub(super) fn new(dispatcher: Arc<Dispatcher>) -> Self {
        let registry = dispatcher.registry().clone();
        let resolvers = Resolvers::of(&registry);
        let bus = registry.services().get::<LiveBus>();
        let outbox = registry.services().get::<OutboxFeed>();
        Self {
            dispatcher,
            registry,
            resolvers,
            bus,
            outbox,
        }
    }
}

impl FromRequestParts<Arc<Shared>> for Live {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<Shared>,
    ) -> Result<Self, Self::Rejection> {
        match state.live.get() {
            Some(live) => Ok(live.clone()),
            None => Err(reply::error(
                KeelError::unavailable("keel.error.unavailable"),
                &reply::request_id_in(&parts.extensions),
            )),
        }
    }
}
