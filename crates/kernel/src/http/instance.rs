use axum::Extension;
use axum::response::{IntoResponse, Json, Response};
use keel_sdk::KeelError;
use keel_types::InstanceInfo;

use super::live::Live;
use super::reply::{self, RequestId};

pub(super) async fn serve(
    live: Live,
    Extension(RequestId(request_id)): Extension<RequestId>,
) -> Response {
    match live.registry.services().get::<InstanceInfo>() {
        Some(info) => Json(info.as_ref()).into_response(),
        None => reply::error(
            KeelError::unavailable("keel.error.unavailable"),
            &request_id,
        ),
    }
}
