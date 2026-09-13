use std::sync::Arc;

use axum::Extension;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Json, Response};
use keel_types::Principal;

use super::Shared;
use super::auth::Zone;
use super::live::Live;
use super::reply::{self, RequestId};
use crate::boot;

pub(super) async fn serve(
    State(state): State<Arc<Shared>>,
    live: Live,
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(principal): Extension<Principal>,
    Extension(Zone(tz)): Extension<Zone>,
    headers: HeaderMap,
) -> Response {
    let payload = match boot::payload(&live.registry, &state.db, &principal, tz).await {
        Ok(payload) => payload,
        Err(error) => return reply::error(error, &request_id),
    };
    if reply::matches_etag(&headers, &payload.meta) {
        return reply::not_modified(&payload.meta);
    }
    let version = payload.meta.clone();
    reply::cached(Json(payload).into_response(), &version)
}
