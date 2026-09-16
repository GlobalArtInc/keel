use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use keel_sdk::{Download, KeelError};
use keel_types::{Kind, Level};

use super::Shared;
use super::auth::Caller;
use super::call::{body, from_query, options, respond};
use super::live::Live;
use super::reply;
use crate::dispatcher::RAW_PREFIX;
use crate::registry::Function;

const NO_STORE: &str = "no-store";
const OCTET_STREAM: &str = "application/octet-stream";

pub(super) async fn get(
    State(state): State<Arc<Shared>>,
    live: Live,
    Path((module, name)): Path<(String, String)>,
    caller: Caller,
    Query(pairs): Query<Vec<(String, String)>>,
    headers: HeaderMap,
) -> Response {
    let params = from_query(pairs);
    serve(&state, &live, &module, &name, caller, &headers, params).await
}

pub(super) async fn post(
    State(state): State<Arc<Shared>>,
    live: Live,
    Path((module, name)): Path<(String, String)>,
    caller: Caller,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    let params = body(request, state.options.body_limit).await;
    serve(&state, &live, &module, &name, caller, &headers, params).await
}

async fn serve(
    state: &Shared,
    live: &Live,
    module: &str,
    name: &str,
    caller: Caller,
    headers: &HeaderMap,
    params: Result<serde_json::Value, KeelError>,
) -> Response {
    let Caller {
        request_id,
        principal,
        client,
        zone,
    } = caller;
    let func = format!("{module}.{name}");
    let Some(function) = live.registry.function(&func).filter(is_download) else {
        return reply::error(
            KeelError::not_found("keel.error.route_not_found")
                .param("path", format!("{RAW_PREFIX}/{module}/{name}")),
            &request_id,
        );
    };
    if principal.level == Level::Guest {
        return reply::error(
            KeelError::unauthenticated("keel.error.unauthenticated"),
            &request_id,
        );
    }
    let params = match params {
        Ok(params) => params,
        Err(error) => return reply::error(error, &request_id),
    };
    let options = options(headers, &request_id, client, zone);
    let mut reply = live
        .dispatcher
        .call(principal.clone(), function.name(), params, options)
        .await;
    match reply.download.take() {
        Some(download) => stream(download),
        None => respond(state, live, &principal, reply).await,
    }
}

fn is_download(function: &&Function) -> bool {
    function.def().kind() == Kind::Download
}

fn stream(download: Download) -> Response {
    let Download {
        filename,
        content_type,
        body,
    } = download;
    let mut response = Body::from_stream(body).into_response();
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(&content_type)
            .unwrap_or_else(|_| HeaderValue::from_static(OCTET_STREAM)),
    );
    if let Ok(value) = HeaderValue::from_str(&disposition(&filename)) {
        headers.insert(CONTENT_DISPOSITION, value);
    }
    headers.insert(CACHE_CONTROL, HeaderValue::from_static(NO_STORE));
    response
}

fn disposition(filename: &str) -> String {
    let plain = filename.is_ascii()
        && !filename
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte == b'"' || byte == b'\\');
    if plain {
        return format!("attachment; filename=\"{filename}\"");
    }
    let fallback: String = filename
        .chars()
        .map(|c| {
            if c.is_ascii() && !c.is_ascii_control() && c != '"' && c != '\\' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!(
        "attachment; filename=\"{fallback}\"; filename*=UTF-8''{}",
        percent_encode(filename)
    )
}

fn percent_encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ascii_name_is_quoted_and_a_name_outside_ascii_gets_a_filename_star() {
        assert_eq!(
            disposition("note.list-2026-09-16.csv"),
            "attachment; filename=\"note.list-2026-09-16.csv\""
        );
        assert_eq!(
            disposition("заметки.csv"),
            "attachment; filename=\"_______.csv\"; filename*=UTF-8''%D0%B7%D0%B0%D0%BC%D0%B5%D1%82%D0%BA%D0%B8.csv"
        );
        assert_eq!(
            disposition("say \"hi\".txt"),
            "attachment; filename=\"say _hi_.txt\"; filename*=UTF-8''say%20%22hi%22.txt"
        );
        assert_eq!(percent_encode("a b+c~d/e"), "a%20b+c~d%2Fe");
    }
}
