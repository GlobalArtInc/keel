use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::{Path, Query, Request, State};
use axum::http::header::ALLOW;
use axum::http::{HeaderMap, HeaderValue};
use axum::response::Response;
use keel_sdk::KeelError;
use keel_sdk::system::Client;
use keel_types::{Kind, Principal};
use serde_json::{Map, Value};

use super::Shared;
use super::auth::{Caller, Zone};
use super::cookie;
use super::live::Live;
use super::reply::{self, IDEMPOTENCY_KEY, IDEMPOTENT_REPLAY, META_VERSION};
use crate::dispatcher::{CallOptions, Reply, download_route_error, upload_route_error};

pub(super) const QUERY_PARAMS: &str = "q";

pub(super) async fn post(
    State(state): State<Arc<Shared>>,
    live: Live,
    Path(func): Path<String>,
    caller: Caller,
    request: Request,
) -> Response {
    let Caller {
        request_id,
        principal,
        client,
        zone,
    } = caller;
    if let Some(error) = raw_only(&live, &func) {
        return reply::error(error, &request_id);
    }
    let options = options(request.headers(), &request_id, client, zone);
    let params = match body(request, state.options.body_limit).await {
        Ok(params) => params,
        Err(error) => return reply::error(error, &request_id),
    };
    let reply = live
        .dispatcher
        .call(principal.clone(), &func, params, options)
        .await;
    respond(&state, &live, &principal, reply).await
}

pub(super) async fn get(
    State(state): State<Arc<Shared>>,
    live: Live,
    Path(func): Path<String>,
    caller: Caller,
    Query(pairs): Query<Vec<(String, String)>>,
    headers: HeaderMap,
) -> Response {
    let Caller {
        request_id,
        principal,
        client,
        zone,
    } = caller;
    if let Some(error) = raw_only(&live, &func) {
        return reply::error(error, &request_id);
    }
    if let Some(function) = live.registry.function(&func)
        && !matches!(function.def().kind(), Kind::List | Kind::Get)
    {
        let mut response = reply::error(
            KeelError::validation("keel.error.method_not_allowed")
                .param("function", func)
                .param("allowed", "POST"),
            &request_id,
        );
        response
            .headers_mut()
            .insert(ALLOW, HeaderValue::from_static("POST"));
        return response;
    }
    let params = match from_query(pairs) {
        Ok(params) => params,
        Err(error) => return reply::error(error, &request_id),
    };
    let options = options(&headers, &request_id, client, zone);
    let reply = live
        .dispatcher
        .call(principal.clone(), &func, params, options)
        .await;
    respond(&state, &live, &principal, reply).await
}

fn raw_only(live: &Live, func: &str) -> Option<KeelError> {
    let function = live.registry.function(func)?;
    match function.def().kind() {
        Kind::Download => Some(download_route_error(function)),
        Kind::Upload => Some(upload_route_error(function)),
        _ => None,
    }
}

pub(super) fn options(
    headers: &HeaderMap,
    request_id: &str,
    client: Client,
    zone: Zone,
) -> CallOptions {
    CallOptions {
        request_id: Some(request_id.to_owned()),
        idempotency_key: headers
            .get(IDEMPOTENCY_KEY)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(str::to_owned),
        client: Some(client),
        tz: zone.0,
        task: None,
    }
}

pub(super) async fn body(request: Request, limit: usize) -> Result<Value, KeelError> {
    let bytes = to_bytes(request.into_body(), limit)
        .await
        .map_err(|error| {
            KeelError::validation("keel.error.body_too_large")
                .param("limit", limit)
                .source(error)
        })?;
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| KeelError::validation("keel.error.body_not_json").source(error))
}

pub(super) fn from_query(pairs: Vec<(String, String)>) -> Result<Value, KeelError> {
    if let Some((_, json)) = pairs.iter().find(|(key, _)| key == QUERY_PARAMS) {
        return match serde_json::from_str::<Value>(json) {
            Ok(Value::Object(params)) => Ok(Value::Object(params)),
            Ok(_) => Err(KeelError::validation("keel.error.query_not_object")
                .param("parameter", QUERY_PARAMS)),
            Err(error) => Err(KeelError::validation("keel.error.query_not_json")
                .param("parameter", QUERY_PARAMS)
                .source(error)),
        };
    }
    let mut params = Map::new();
    for (key, value) in pairs {
        let value = match serde_json::from_str::<Value>(&value) {
            Ok(parsed) => parsed,
            Err(_) => Value::String(value),
        };
        params.insert(key, value);
    }
    Ok(Value::Object(params))
}

pub(super) async fn respond(
    state: &Shared,
    live: &Live,
    principal: &Principal,
    reply: Reply,
) -> Response {
    let mut response = reply::envelope(&reply.envelope);
    match state.meta_version(&live.registry, principal).await {
        Ok(version) => {
            if let Ok(value) = HeaderValue::from_str(&version) {
                response.headers_mut().insert(META_VERSION, value);
            }
        }
        Err(error) => tracing::warn!(error = %error, "the metadata version is not available"),
    }
    if reply.replay {
        response
            .headers_mut()
            .insert(IDEMPOTENT_REPLAY, HeaderValue::from_static("true"));
    }
    if let Some(change) = &reply.session {
        cookie::apply(&mut response, change);
    }
    response
}

#[cfg(test)]
mod tests {
    use keel_types::ErrorCode;
    use serde_json::json;

    use super::*;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn bare_query_pairs_become_json_values_where_they_parse_and_strings_otherwise() {
        let params = from_query(pairs(&[
            ("page", "2"),
            ("pageSize", "25"),
            ("search", "www"),
            ("id", "01J8A0000000000000000000A1"),
            ("sort", r#"{"key":"name","dir":"asc"}"#),
            ("flag", "true"),
            ("none", "null"),
            ("quoted", "\"42\""),
            ("page", "3"),
        ]))
        .unwrap();
        assert_eq!(
            params,
            json!({
                "page": 3, "pageSize": 25, "search": "www", "id": "01J8A0000000000000000000A1",
                "sort": { "key": "name", "dir": "asc" }, "flag": true, "none": null, "quoted": "42"
            })
        );
        assert_eq!(from_query(Vec::new()).unwrap(), json!({}));
    }

    #[test]
    fn the_q_parameter_carries_the_whole_object_and_must_be_one() {
        let params = from_query(pairs(&[
            ("q", r#"{"search":"42","page":1}"#),
            ("search", "ignored"),
        ]))
        .unwrap();
        assert_eq!(params, json!({ "search": "42", "page": 1 }));
        let error = from_query(pairs(&[("q", "[1]")])).unwrap_err();
        assert_eq!(error.code(), ErrorCode::Validation);
        assert_eq!(error.key(), "keel.error.query_not_object");
        let error = from_query(pairs(&[("q", "{not json")])).unwrap_err();
        assert_eq!(error.key(), "keel.error.query_not_json");
    }
}
