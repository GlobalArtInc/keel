use std::sync::Arc;

use axum::body::Body;
use axum::extract::multipart::{Field, Multipart, MultipartError};
use axum::extract::{FromRequest, Path, Query, Request, State};
use axum::http::header::{ALLOW, CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures::StreamExt;
use keel_sdk::multipart::{PARAMS_LIMIT, PARAMS_PART};
use keel_sdk::{Download, KeelError};
use keel_types::{ErrorCode, FieldError, FileAttrs, Kind, Level};
use serde_json::{Map, Value};

use super::Shared;
use super::auth::{self, Caller};
use super::call::{body, from_query, options, respond};
use super::events::KEEP_ALIVE_FLOOR;
use super::lines::{self, Tail};
use super::live::Live;
use super::reply;
use crate::dispatcher::{Plan, RAW_PREFIX, file_encoding, file_too_large, file_type};
use crate::registry::Function;

const NO_STORE: &str = "no-store";
const OCTET_STREAM: &str = "application/octet-stream";
const MULTIPART: &str = "multipart/form-data";

pub const BODY_NOT_MULTIPART_KEY: &str = "keel.error.body_not_multipart";
pub const PART_UNKNOWN_KEY: &str = "keel.error.part_unknown";
pub const PART_REPEATED_KEY: &str = "keel.error.part_repeated";

pub(super) async fn get(
    State(state): State<Arc<Shared>>,
    live: Live,
    Path((module, name)): Path<(String, String)>,
    caller: Caller,
    Query(pairs): Query<Vec<(String, String)>>,
    headers: HeaderMap,
) -> Response {
    let Some(function) = resolve(&live, &module, &name) else {
        return not_found(&module, &name, &caller.request_id);
    };
    if let Some(refused) = refuse_guest(&caller) {
        return refused;
    }
    if function.def().kind() == Kind::Upload {
        return method_not_allowed(function, "POST", &caller.request_id);
    }
    let params = from_query(pairs);
    serve(&state, &live, function, caller, &headers, params).await
}

pub(super) async fn post(
    State(state): State<Arc<Shared>>,
    live: Live,
    Path((module, name)): Path<(String, String)>,
    caller: Caller,
    request: Request,
) -> Response {
    let Some(function) = resolve(&live, &module, &name) else {
        return not_found(&module, &name, &caller.request_id);
    };
    if let Some(refused) = refuse_guest(&caller) {
        return refused;
    }
    if function.def().kind() == Kind::Stream {
        return method_not_allowed(function, "GET", &caller.request_id);
    }
    let headers = request.headers().clone();
    let params = match function.def().kind() {
        Kind::Upload => multipart(request, function).await,
        _ => body(request, state.options.body_limit).await,
    };
    serve(&state, &live, function, caller, &headers, params).await
}

fn resolve<'a>(live: &'a Live, module: &str, name: &str) -> Option<&'a Function> {
    let dotted = module.replace('_', ".");
    [module, dotted.as_str()]
        .into_iter()
        .find_map(|prefix| live.registry.function(&format!("{prefix}.{name}")))
        .filter(|function| function.module() == module)
        .filter(is_raw)
}

fn method_not_allowed(function: &Function, allowed: &'static str, request_id: &str) -> Response {
    let mut response = reply::error(
        KeelError::validation("keel.error.method_not_allowed")
            .param("function", function.name())
            .param("allowed", allowed),
        request_id,
    );
    response
        .headers_mut()
        .insert(ALLOW, HeaderValue::from_static(allowed));
    response
}

fn refuse_guest(caller: &Caller) -> Option<Response> {
    (caller.principal.level == Level::Guest).then(|| {
        reply::error(
            KeelError::unauthenticated("keel.error.unauthenticated"),
            &caller.request_id,
        )
    })
}

fn not_found(module: &str, name: &str, request_id: &str) -> Response {
    reply::error(
        KeelError::not_found("keel.error.route_not_found")
            .param("path", format!("{RAW_PREFIX}/{module}/{name}")),
        request_id,
    )
}

async fn serve(
    state: &Shared,
    live: &Live,
    function: &Function,
    caller: Caller,
    headers: &HeaderMap,
    params: Result<Value, KeelError>,
) -> Response {
    let Caller {
        request_id,
        principal,
        client,
        zone,
    } = caller;
    let params = match params {
        Ok(params) => params,
        Err(error) => return reply::error(error, &request_id),
    };
    let options = options(headers, &request_id, client.clone(), zone);
    let mut reply = live
        .dispatcher
        .call(principal.clone(), function.name(), params, options)
        .await;
    if let Some(download) = reply.download.take() {
        return stream(download);
    }
    let credential = auth::credential(headers);
    match (reply.lines.take(), credential) {
        (Some(lines), Some(credential)) => {
            let tail = Tail {
                function: function.name().to_owned(),
                request_id,
                principal,
                credential,
                on_behalf: auth::on_behalf(headers),
                client,
                resolvers: live.resolvers.clone(),
                keep_alive: state.options.sse_keep_alive.max(KEEP_ALIVE_FLOOR),
                db: state.db.clone(),
            };
            lines::serve(&state.tails, tail, lines)
        }
        (Some(_), None) => reply::error(
            KeelError::unauthenticated("keel.error.unauthenticated"),
            &request_id,
        ),
        (None, _) => respond(state, live, &principal, reply).await,
    }
}

fn is_raw(function: &&Function) -> bool {
    matches!(
        function.def().kind(),
        Kind::Download | Kind::Upload | Kind::Stream
    )
}

fn multipart_limit(files: &[(String, FileAttrs)]) -> u64 {
    files
        .iter()
        .map(|(_, file)| file.max_bytes)
        .fold(PARAMS_LIMIT as u64, u64::saturating_add)
}

async fn multipart(request: Request, function: &Function) -> Result<Value, KeelError> {
    let plan = Plan::of_params(function.descriptor());
    let files = plan.file_fields();
    let limit = multipart_limit(&files);
    let is_multipart = request
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .eq_ignore_ascii_case(MULTIPART)
        });
    if !is_multipart {
        return Err(
            KeelError::validation(BODY_NOT_MULTIPART_KEY).param("function", function.name())
        );
    }
    let (parts, body) = request.into_parts();
    let bytes = collect(body, limit).await?;
    let request = Request::from_parts(parts, Body::from(bytes));
    let mut multipart = Multipart::from_request(request, &())
        .await
        .map_err(|error| KeelError::validation(BODY_NOT_MULTIPART_KEY).source(error))?;
    let mut params = Map::new();
    let mut seen_parts: Vec<String> = Vec::new();
    while let Some(field) = multipart.next_field().await.map_err(malformed)? {
        let name = field.name().unwrap_or_default().to_owned();
        if seen_parts.contains(&name) {
            return Err(KeelError::validation(PART_REPEATED_KEY).param("part", name));
        }
        seen_parts.push(name.clone());
        if name == PARAMS_PART {
            let bytes = read_part(field, PARAMS_LIMIT as u64)
                .await?
                .ok_or_else(|| {
                    KeelError::validation("keel.error.body_too_large").param("limit", PARAMS_LIMIT)
                })?;
            match serde_json::from_slice::<Value>(&bytes) {
                Ok(Value::Object(object)) => params.extend(object),
                Ok(_) | Err(_) => {
                    return Err(KeelError::validation("keel.error.body_not_json")
                        .param("part", PARAMS_PART));
                }
            }
            continue;
        }
        let Some((_, attrs)) = files.iter().find(|(file, _)| *file == name) else {
            return Err(KeelError::validation(PART_UNKNOWN_KEY).param("part", name));
        };
        let content_type = field.content_type().map(str::to_owned);
        if !content_type
            .as_deref()
            .is_some_and(|content_type| attrs.admits(content_type))
        {
            return Err(refused(file_type(&name, attrs, content_type.as_deref())));
        }
        let Some(bytes) = read_part(field, attrs.max_bytes).await? else {
            return Err(refused(file_too_large(&name, attrs, None)));
        };
        let value = if plan.is_base64(&name) {
            STANDARD.encode(&bytes)
        } else {
            String::from_utf8(bytes).map_err(|_| refused(file_encoding(&name)))?
        };
        params.insert(name, Value::String(value));
    }
    Ok(Value::Object(params))
}

async fn collect(body: Body, limit: u64) -> Result<Vec<u8>, KeelError> {
    let mut stream = body.into_data_stream();
    let mut bytes = Vec::new();
    let mut over = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|error| KeelError::validation(BODY_NOT_MULTIPART_KEY).source(error))?;
        if over > 0 || (bytes.len() as u64).saturating_add(chunk.len() as u64) > limit {
            over = over.saturating_add(chunk.len() as u64);
            if over > limit {
                break;
            }
            continue;
        }
        bytes.extend_from_slice(&chunk);
    }
    if over > 0 {
        return Err(KeelError::validation("keel.error.body_too_large").param("limit", limit));
    }
    Ok(bytes)
}

async fn read_part(mut field: Field<'_>, cap: u64) -> Result<Option<Vec<u8>>, KeelError> {
    let mut bytes = Vec::new();
    while let Some(chunk) = field.chunk().await.map_err(malformed)? {
        if (bytes.len() as u64).saturating_add(chunk.len() as u64) > cap {
            return Ok(None);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(Some(bytes))
}

fn refused(field: FieldError) -> KeelError {
    KeelError::new(ErrorCode::Validation).fields([field])
}

fn malformed(error: MultipartError) -> KeelError {
    KeelError::validation(BODY_NOT_MULTIPART_KEY).source(error)
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

    #[test]
    fn the_multipart_limit_is_the_sum_of_the_file_caps_plus_the_params_allowance() {
        assert_eq!(multipart_limit(&[]), PARAMS_LIMIT as u64);
        assert_eq!(
            multipart_limit(&[
                ("logo".to_owned(), FileAttrs::new(65536, ["image/svg+xml"])),
                ("cover".to_owned(), FileAttrs::new(1024, ["image/png"])),
            ]),
            PARAMS_LIMIT as u64 + 65536 + 1024
        );
        assert_eq!(
            multipart_limit(&[("blob".to_owned(), FileAttrs::new(u64::MAX, ["a/b"]))]),
            u64::MAX
        );
    }
}
