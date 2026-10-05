use axum::Extension;
use axum::extract::Query;
use axum::response::{IntoResponse, Json, Response};
use keel_sdk::KeelError;
use keel_types::{
    CHALLENGE_MAX_LEN, CHALLENGE_MIN_LEN, CHALLENGE_PARAM, InstanceInfo, ProvenInstance,
    is_challenge,
};

use super::live::Live;
use super::reply::{self, RequestId};
use crate::instance::InstanceKey;

pub const CHALLENGE_KEY: &str = "keel.error.challenge";

pub(super) async fn serve(
    live: Live,
    Extension(RequestId(request_id)): Extension<RequestId>,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Response {
    let services = live.registry.services();
    let Some(info) = services.get::<InstanceInfo>() else {
        return unavailable(&request_id);
    };
    let challenges: Vec<&str> = pairs
        .iter()
        .filter(|(name, _)| name == CHALLENGE_PARAM)
        .map(|(_, value)| value.as_str())
        .collect();
    match challenges.as_slice() {
        [] => Json(info.as_ref()).into_response(),
        [challenge] if is_challenge(challenge) => match services.get::<InstanceKey>() {
            Some(key) => Json(ProvenInstance {
                instance: info.as_ref().clone(),
                proof: key.prove(&info, challenge),
            })
            .into_response(),
            None => unavailable(&request_id),
        },
        _ => reply::error(
            KeelError::validation(CHALLENGE_KEY)
                .field(CHALLENGE_PARAM)
                .param("min", CHALLENGE_MIN_LEN)
                .param("max", CHALLENGE_MAX_LEN),
            &request_id,
        ),
    }
}

fn unavailable(request_id: &str) -> Response {
    reply::error(KeelError::unavailable("keel.error.unavailable"), request_id)
}
