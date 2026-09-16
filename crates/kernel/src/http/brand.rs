use std::sync::Arc;

use axum::Extension;
use axum::body::{Body, Bytes};
use axum::extract::{FromRequestParts, Path, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::Response;
use keel_sdk::system::ManifestPatch;
use keel_sdk::{Asset, KeelError};

use super::Shared;
use super::live::Live;
use super::proxy;
use super::reply::{self, RequestId};
use crate::boot;

const KEEL_MARK: &[u8] = include_bytes!("../../assets/keel.svg");
const SVG: &str = "image/svg+xml";
const OCTET_STREAM: &str = "application/octet-stream";
const CACHE_NO_STORE: &str = "no-store";
const IMAGE_POLICY: &str = "default-src 'none'; style-src 'unsafe-inline'";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct HostName(pub(super) Option<String>);

impl FromRequestParts<Arc<Shared>> for HostName {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<Shared>,
    ) -> Result<Self, Self::Rejection> {
        let trusted = proxy::is_trusted(
            proxy::peer(&parts.extensions),
            &state.options.trusted_proxies,
        );
        Ok(Self(host_name(&parts.headers, &parts.uri, trusted)))
    }
}

pub(super) async fn effective(
    State(state): State<Arc<Shared>>,
    live: Live,
    Extension(RequestId(request_id)): Extension<RequestId>,
    HostName(host): HostName,
) -> Response {
    match boot::patch(&live.registry, &state.db, host.as_deref()).await {
        Ok(Some(ManifestPatch {
            logo: Some(logo), ..
        })) => return image(SVG, Bytes::from(logo)),
        Ok(_) => {}
        Err(error) => return reply::error(error, &request_id),
    }
    match live.registry.products() {
        [single] => answer(single.logo(), &request_id),
        _ => image(SVG, Bytes::from_static(KEEL_MARK)),
    }
}

pub(super) async fn product(
    live: Live,
    Extension(RequestId(request_id)): Extension<RequestId>,
    Path(id): Path<String>,
) -> Response {
    let logo = live
        .registry
        .manifests()
        .iter()
        .find(|manifest| manifest.id == id)
        .and_then(|manifest| manifest.logo());
    answer(logo, &request_id)
}

fn answer(asset: Option<&'static Asset>, request_id: &str) -> Response {
    match asset {
        Some(asset) => image(asset.content_type, Bytes::from_static(asset.bytes)),
        None => reply::error(KeelError::not_found("keel.error.not_found"), request_id),
    }
}

fn image(content_type: &str, body: Bytes) -> Response {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = StatusCode::OK;
    let head = response.headers_mut();
    head.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(content_type)
            .unwrap_or_else(|_| HeaderValue::from_static(OCTET_STREAM)),
    );
    head.insert(CACHE_CONTROL, HeaderValue::from_static(CACHE_NO_STORE));
    head.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    head.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(IMAGE_POLICY),
    );
    response
}

pub(super) fn host_name(headers: &HeaderMap, uri: &Uri, trusted_peer: bool) -> Option<String> {
    let authority = proxy::authority(headers, uri, trusted_peer)?.trim();
    let host = if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']')?;
        &authority[..end + 2]
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(host: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(axum::http::header::HOST, HeaderValue::from_static(host));
        headers
    }

    #[test]
    fn the_host_name_is_the_host_header_without_its_port_in_lower_case() {
        let uri = Uri::from_static("/api/v1/boot");
        assert_eq!(
            host_name(&headers("panel.example:1500"), &uri, false).as_deref(),
            Some("panel.example")
        );
        assert_eq!(
            host_name(&headers("Panel.Example"), &uri, false).as_deref(),
            Some("panel.example")
        );
        assert_eq!(
            host_name(&headers("127.0.0.1:8080"), &uri, false).as_deref(),
            Some("127.0.0.1")
        );
        assert_eq!(
            host_name(&headers("[::1]:8080"), &uri, false).as_deref(),
            Some("[::1]")
        );
        assert_eq!(host_name(&headers(""), &uri, false), None);
        assert_eq!(host_name(&headers(":1500"), &uri, false), None);
        assert_eq!(host_name(&HeaderMap::new(), &uri, false), None);
        let absolute = Uri::from_static("http://Keel.Test:9/api/v1/boot");
        assert_eq!(
            host_name(&HeaderMap::new(), &absolute, false).as_deref(),
            Some("keel.test")
        );
        let mut forwarded = headers("upstream.internal:1500");
        forwarded.insert(
            super::proxy::X_FORWARDED_HOST,
            HeaderValue::from_static("Panel.Example:8443"),
        );
        assert_eq!(
            host_name(&forwarded, &uri, true).as_deref(),
            Some("panel.example")
        );
        assert_eq!(
            host_name(&forwarded, &uri, false).as_deref(),
            Some("upstream.internal")
        );
    }

    #[test]
    fn the_kernel_mark_is_an_svg_document_without_a_script_element() {
        let mark = std::str::from_utf8(KEEL_MARK).expect("UTF-8");
        assert!(mark.trim_start().starts_with("<svg"), "{mark}");
        assert!(!mark.contains("<script"), "{mark}");
        assert!(
            mark.contains("xmlns=\"http://www.w3.org/2000/svg\""),
            "{mark}"
        );
    }
}
