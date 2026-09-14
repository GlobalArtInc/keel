use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;
use std::panic::RefUnwindSafe;
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::http::header::{
    ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_SECURITY_POLICY, CONTENT_TYPE,
    REFERRER_POLICY, VARY, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::Response;
use rust_embed::Embed;

const INDEX: &str = "index.html";
const ASSET_EXTENSIONS: &[&str] = &[
    "js", "mjs", "css", "map", "woff2", "woff", "svg", "png", "ico", "json", "txt", "br",
];
const CONTENT_SECURITY_POLICY_VALUE: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'";
const CACHE_IMMUTABLE: &str = "public, max-age=31536000, immutable";
const CACHE_NO_STORE: &str = "no-store";

const RESERVED_PREFIXES: &[&str] = &["/api/", "/healthz", "/readyz", "/brand/"];
const BROTLI: &str = "br";
const BROTLI_SUFFIX: &str = ".br";
const HASH_MIN_LEN: usize = 6;

trait Source: Send + Sync + RefUnwindSafe + 'static {
    fn file(&self, path: &str) -> Option<Bytes>;
}

struct Embedded<E>(PhantomData<fn() -> E>);

impl<E: Embed + 'static> Source for Embedded<E> {
    fn file(&self, path: &str) -> Option<Bytes> {
        E::get(path).map(|file| match file.data {
            Cow::Borrowed(bytes) => Bytes::from_static(bytes),
            Cow::Owned(bytes) => Bytes::from(bytes),
        })
    }
}

struct Files(BTreeMap<String, Bytes>);

impl Source for Files {
    fn file(&self, path: &str) -> Option<Bytes> {
        self.0.get(path).cloned()
    }
}

#[derive(Clone)]
pub struct Frontend {
    source: Arc<dyn Source>,
}

impl Frontend {
    pub fn embedded<E: Embed + 'static>() -> Self {
        Self {
            source: Arc::new(Embedded::<E>(PhantomData)),
        }
    }

    pub fn from_files<N, B>(files: impl IntoIterator<Item = (N, B)>) -> Self
    where
        N: Into<String>,
        B: Into<Bytes>,
    {
        Self {
            source: Arc::new(Files(
                files
                    .into_iter()
                    .map(|(name, body)| (name.into(), body.into()))
                    .collect(),
            )),
        }
    }

    fn file(&self, path: &str) -> Option<Bytes> {
        self.source.file(path)
    }

    pub(super) fn answer(
        &self,
        method: &Method,
        path: &str,
        headers: &HeaderMap,
    ) -> Option<Response> {
        if *method != Method::GET && *method != Method::HEAD {
            return None;
        }
        if reserved(path) {
            return None;
        }
        let name = path.trim_start_matches('/');
        if !name.is_empty()
            && !name.ends_with(BROTLI_SUFFIX)
            && let Some(body) = self.file(name)
        {
            return Some(self.asset(name, body, headers));
        }
        if names_asset(path) {
            return None;
        }
        let index = self.file(INDEX)?;
        Some(self.asset(INDEX, index, headers))
    }

    fn asset(&self, name: &str, body: Bytes, headers: &HeaderMap) -> Response {
        let mut response = Response::new(Body::empty());
        let head = response.headers_mut();
        head.insert(CONTENT_TYPE, HeaderValue::from_static(content_type(name)));
        head.insert(
            CACHE_CONTROL,
            HeaderValue::from_static(if hashed(name) {
                CACHE_IMMUTABLE
            } else {
                CACHE_NO_STORE
            }),
        );
        head.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
        if is_html(name) {
            head.insert(
                CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(CONTENT_SECURITY_POLICY_VALUE),
            );
            head.insert(REFERRER_POLICY, HeaderValue::from_static("same-origin"));
        }
        let mut body = body;
        if let Some(twin) = self.file(&format!("{name}{BROTLI_SUFFIX}")) {
            head.insert(VARY, HeaderValue::from_static("Accept-Encoding"));
            if admits_brotli(headers) {
                head.insert(CONTENT_ENCODING, HeaderValue::from_static(BROTLI));
                body = twin;
            }
        }
        *response.status_mut() = StatusCode::OK;
        *response.body_mut() = Body::from(body);
        response
    }
}

impl fmt::Debug for Frontend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Frontend").finish_non_exhaustive()
    }
}

fn reserved(path: &str) -> bool {
    RESERVED_PREFIXES
        .iter()
        .any(|prefix| path.starts_with(prefix) || path == prefix.trim_end_matches('/'))
}

fn names_asset(path: &str) -> bool {
    let last = path.rsplit('/').next().unwrap_or(path);
    let Some((_, extension)) = last.rsplit_once('.') else {
        return false;
    };
    ASSET_EXTENSIONS.contains(&extension)
}

fn hashed(name: &str) -> bool {
    let file = name.rsplit('/').next().unwrap_or(name);
    let stem = file.split('.').next().unwrap_or(file);
    let Some((_, hash)) = stem.rsplit_once('-') else {
        return false;
    };
    hash.len() >= HASH_MIN_LEN
        && hash.bytes().all(|byte| byte.is_ascii_alphanumeric())
        && hash
            .bytes()
            .any(|byte| byte.is_ascii_digit() || byte.is_ascii_uppercase())
}

fn content_type(name: &str) -> &'static str {
    match name.rsplit_once('.').map(|(_, extension)| extension) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn is_html(name: &str) -> bool {
    name.ends_with(".html")
}

fn admits_brotli(headers: &HeaderMap) -> bool {
    headers
        .get_all(ACCEPT_ENCODING)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|entry| {
            let mut parts = entry.split(';');
            let coding = parts.next()?.trim();
            let weight = parts
                .find_map(|part| part.trim().strip_prefix("q="))
                .and_then(|weight| weight.trim().parse::<f32>().ok());
            Some((coding, weight))
        })
        .any(|(coding, weight)| coding.eq_ignore_ascii_case(BROTLI) && weight != Some(0.0))
}

#[cfg(test)]
mod tests {
    use std::panic::UnwindSafe;

    use super::*;

    #[test]
    fn a_frontend_is_shared_between_threads_and_across_a_panic_boundary() {
        fn assert_bounds<T: Send + Sync + UnwindSafe + RefUnwindSafe>() {}
        assert_bounds::<Frontend>();
    }

    #[test]
    fn a_path_names_an_asset_by_the_extension_of_its_last_segment_and_falls_back_otherwise() {
        for path in [
            "/assets/x.woff2",
            "/media/ibm-plex-sans-latin-400-normal-JC3YNCCZ.woff",
            "/main-abc123.js",
            "/main-abc123.js.br",
            "/styles-TYTUI7DH.css",
            "/missing.png",
            "/favicon.ico",
            "/3rdpartylicenses.txt",
            "/prerendered-routes.json",
            "/chunk-ZPMDAVXX.js.map",
            "/brand/logo.svg",
        ] {
            assert!(names_asset(path), "{path} names an asset");
        }
        for path in [
            "/",
            "/f/keel.account.list",
            "/f/note.list",
            "/f/note.edit/01J8A0000000000000000000A2",
            "/login",
            "/settings/auth.password.min_length",
            "/some.page.html",
            "/index",
        ] {
            assert!(!names_asset(path), "{path} falls back to the index");
        }
    }

    #[test]
    fn the_reserved_prefixes_are_the_api_the_probes_and_the_brand_routes() {
        for path in [
            "/api/",
            "/api/v1/nothing",
            "/api/v2/anything",
            "/healthz",
            "/readyz",
            "/brand/",
            "/brand/logo.svg",
        ] {
            assert!(reserved(path), "{path} is reserved");
        }
        for path in ["/", "/apix", "/f/api.list", "/branding", "/health"] {
            assert!(!reserved(path), "{path} is the frontend's");
        }
    }

    #[test]
    fn a_hashed_name_ends_its_stem_with_the_build_hash_and_the_rest_is_unhashed() {
        for name in [
            "main-abc123.js",
            "main-2WTDPBBK.js",
            "chunk-ZPMDAVXX.js",
            "chunk-ZPMDAVXX.js.map",
            "styles-TYTUI7DH.css",
            "media/ibm-plex-sans-latin-ext-500-normal-SG2XNXKB.woff2",
            "media/ibm-plex-mono-cyrillic-400-normal-SUZ3KICP.woff",
        ] {
            assert!(hashed(name), "{name} is hashed");
        }
        for name in [
            "index.html",
            "theme-boot.js",
            "3rdpartylicenses.txt",
            "prerendered-routes.json",
            "favicon.ico",
            "media/logo.svg",
        ] {
            assert!(!hashed(name), "{name} is unhashed");
        }
    }

    #[test]
    fn accept_encoding_admits_brotli_by_its_token_unless_weighted_zero() {
        let admits = |value: &'static str| {
            let mut headers = HeaderMap::new();
            headers.insert(ACCEPT_ENCODING, HeaderValue::from_static(value));
            admits_brotli(&headers)
        };
        assert!(admits("br"));
        assert!(admits("gzip, deflate, br"));
        assert!(admits("gzip;q=1.0, br;q=0.8"));
        assert!(admits("BR"));
        assert!(!admits("gzip, deflate"));
        assert!(!admits("br;q=0"));
        assert!(!admits("brotli"));
        assert!(!admits_brotli(&HeaderMap::new()));
    }

    #[test]
    fn the_content_type_follows_the_extension() {
        assert_eq!(content_type("index.html"), "text/html; charset=utf-8");
        assert_eq!(
            content_type("main-abc123.js"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(content_type("styles-x.css"), "text/css; charset=utf-8");
        assert_eq!(content_type("media/a.woff2"), "font/woff2");
        assert_eq!(content_type("prerendered-routes.json"), "application/json");
        assert_eq!(content_type("logo.svg"), "image/svg+xml");
        assert_eq!(content_type("unknown.bin"), "application/octet-stream");
    }
}
