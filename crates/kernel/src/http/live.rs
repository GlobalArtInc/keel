use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::response::Response;
use keel_sdk::KeelError;
use keel_sdk::system::Authenticator;

use super::Shared;
use super::reply;
use crate::dispatcher::Dispatcher;
use crate::registry::Registry;

#[derive(Clone)]
pub(super) struct Live {
    pub(super) dispatcher: Arc<Dispatcher>,
    pub(super) registry: Arc<Registry>,
    pub(super) authenticator: Option<Arc<dyn Authenticator>>,
}

impl Live {
    pub(super) fn new(dispatcher: Arc<Dispatcher>) -> Self {
        let registry = dispatcher.registry().clone();
        let authenticator = registry.services().get::<dyn Authenticator>();
        Self {
            dispatcher,
            registry,
            authenticator,
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
