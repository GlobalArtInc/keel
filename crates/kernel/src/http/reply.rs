use axum::Json;
use axum::extract::Request;
use axum::http::header::{CACHE_CONTROL, ETAG, IF_NONE_MATCH};
use axum::http::{Extensions, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use keel_sdk::KeelError;
use keel_types::{Envelope, Meta, MetaPrincipal, Principal};
use serde::Serialize;

use crate::dispatcher::generate_request_id;

pub const REQUEST_ID: HeaderName = HeaderName::from_static("x-keel-request-id");
pub const META_VERSION: HeaderName = HeaderName::from_static("x-keel-meta-version");
pub const IDEMPOTENT_REPLAY: HeaderName = HeaderName::from_static("x-keel-idempotent-replay");
pub const IDEMPOTENCY_KEY: HeaderName = HeaderName::from_static("idempotency-key");

const CACHE_PRIVATE: &str = "private, no-store";
const REQUEST_ID_MAX_LEN: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RequestId(pub(super) String);

pub(super) async fn request_id(mut request: Request, next: Next) -> Response {
    let id = request
        .headers()
        .get(REQUEST_ID)
        .and_then(|value| value.to_str().ok())
        .filter(|value| acceptable_request_id(value))
        .map(str::to_owned)
        .unwrap_or_else(generate_request_id);
    request.extensions_mut().insert(RequestId(id.clone()));
    let mut response = next.run(request).await;
    if let Ok(value) = HeaderValue::from_str(&id) {
        response.headers_mut().insert(REQUEST_ID, value);
    }
    response
}

pub(super) fn request_id_in(extensions: &Extensions) -> String {
    extensions
        .get::<RequestId>()
        .map(|id| id.0.clone())
        .unwrap_or_else(generate_request_id)
}

pub(crate) fn acceptable_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= REQUEST_ID_MAX_LEN
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

pub(super) fn envelope<T: Serialize>(envelope: &Envelope<T>) -> Response {
    let status =
        StatusCode::from_u16(envelope.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(envelope)).into_response()
}

pub(super) fn ok<T: Serialize>(data: T, request_id: &str, principal: &Principal) -> Response {
    envelope(&Envelope::ok(data, meta(request_id, principal)))
}

pub(super) fn error(error: KeelError, request_id: &str) -> Response {
    let mut wire = error.to_call_error();
    wire.request_id = Some(request_id.to_owned());
    envelope(&Envelope::<()>::err(wire))
}

pub(super) fn meta(request_id: &str, principal: &Principal) -> Meta {
    Meta {
        request_id: request_id.to_owned(),
        principal: meta_principal(principal),
    }
}

fn meta_principal(principal: &Principal) -> Option<MetaPrincipal> {
    if principal.level == keel_types::Level::Guest {
        return None;
    }
    principal.meta().ok()
}

pub(super) fn etag_value(version: &str) -> HeaderValue {
    HeaderValue::from_str(&format!("\"{version}\""))
        .unwrap_or_else(|_| HeaderValue::from_static("\"invalid\""))
}

pub(super) fn matches_etag(headers: &HeaderMap, version: &str) -> bool {
    headers
        .get_all(IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .any(|candidate| {
            candidate == "*"
                || candidate
                    .strip_prefix("W/")
                    .unwrap_or(candidate)
                    .trim_matches('"')
                    == version
        })
}

pub(super) fn cached(mut response: Response, version: &str) -> Response {
    let headers = response.headers_mut();
    headers.insert(ETAG, etag_value(version));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static(CACHE_PRIVATE));
    response
}

pub(super) fn not_modified(version: &str) -> Response {
    cached(StatusCode::NOT_MODIFIED.into_response(), version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_request_id_is_kept_only_when_it_is_short_and_plain() {
        assert!(acceptable_request_id("req-01j9q3w6b1r8n5x2k4t7m0v9z3"));
        assert!(acceptable_request_id("trace_7.a"));
        assert!(!acceptable_request_id(""));
        assert!(!acceptable_request_id("has space"));
        assert!(!acceptable_request_id("new\nline"));
        assert!(!acceptable_request_id(&"x".repeat(REQUEST_ID_MAX_LEN + 1)));
    }

    #[test]
    fn if_none_match_accepts_quoted_weak_and_wildcard_forms() {
        let mut headers = HeaderMap::new();
        headers.insert(
            IF_NONE_MATCH,
            HeaderValue::from_static("\"m-1\", W/\"m-2\""),
        );
        assert!(matches_etag(&headers, "m-1"));
        assert!(matches_etag(&headers, "m-2"));
        assert!(!matches_etag(&headers, "m-3"));
        headers.insert(IF_NONE_MATCH, HeaderValue::from_static("*"));
        assert!(matches_etag(&headers, "anything"));
        assert!(!matches_etag(&HeaderMap::new(), "m-1"));
        assert_eq!(etag_value("m-1"), "\"m-1\"");
    }
}
