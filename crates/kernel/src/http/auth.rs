use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::HeaderMap;
use axum::http::header::{ACCEPT_LANGUAGE, AUTHORIZATION, COOKIE, HOST, ORIGIN};
use axum::middleware::Next;
use axum::response::Response;
use keel_sdk::runtime::Db;
use keel_sdk::system::Credential;
use keel_sdk::{KeelError, Secret};
use keel_types::{Id, Level, Principal, Scope, Via};

use super::Shared;
use super::live::Live;
use super::reply;
use crate::loader::REQUIRED_LANGUAGE;
use crate::registry::Registry;

pub const SESSION_COOKIE: &str = "__Host-keel_session";
pub const GUEST_LOGIN: &str = "guest";

const SEC_FETCH_SITE: &str = "sec-fetch-site";
const CROSS_SITE: &str = "cross-site";
const BEARER: &str = "bearer ";

pub fn guest(lang: &str) -> Principal {
    Principal {
        id: Id::ROOT_ACCOUNT,
        login: GUEST_LOGIN.to_owned(),
        level: Level::Guest,
        account_id: Id::ROOT_ACCOUNT,
        role: String::new(),
        actor: None,
        scope: Scope::Full,
        via: Via::Http,
        external_id: None,
        origin: None,
        lang: lang.to_owned(),
    }
}

pub(super) async fn authenticate(
    State(state): State<Arc<Shared>>,
    live: Live,
    mut request: Request,
    next: Next,
) -> Response {
    let request_id = reply::request_id_in(request.extensions());
    let headers = request.headers();
    let credential = credential(headers);
    let authority = request
        .uri()
        .authority()
        .map(|authority| authority.as_str());
    if matches!(credential, Some(Credential::Session(_))) && cross_site(headers, authority) {
        return reply::error(KeelError::forbidden("keel.error.cross_site"), &request_id);
    }
    let principal = match (credential, &live.authenticator) {
        (Some(credential), Some(authenticator)) => {
            let db = Db::from_connection(state.db.clone());
            match authenticator.authenticate(&credential, &db).await {
                Ok(Some(principal)) => principal,
                Ok(None) => {
                    return reply::error(
                        KeelError::unauthenticated("keel.error.unauthenticated"),
                        &request_id,
                    );
                }
                Err(error) => return reply::error(error, &request_id),
            }
        }
        _ => guest(&accept_language(headers, &live.registry)),
    };
    request.extensions_mut().insert(principal);
    next.run(request).await
}

pub(super) fn credential(headers: &HeaderMap) -> Option<Credential> {
    if let Some(token) = bearer(headers) {
        return Some(Credential::Bearer(Secret::new(token)));
    }
    session_cookie(headers).map(|value| Credential::Session(Secret::new(value)))
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let scheme = value.get(..BEARER.len())?;
    if !scheme.eq_ignore_ascii_case(BEARER) {
        return None;
    }
    let token = value[BEARER.len()..].trim();
    (!token.is_empty()).then_some(token)
}

fn session_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == SESSION_COOKIE)
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
}

pub(super) fn cross_site(headers: &HeaderMap, authority: Option<&str>) -> bool {
    if headers
        .get(SEC_FETCH_SITE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|site| site.trim().eq_ignore_ascii_case(CROSS_SITE))
    {
        return true;
    }
    match headers.get(ORIGIN).and_then(|value| value.to_str().ok()) {
        Some(origin) => !origin_matches(
            origin,
            headers
                .get(HOST)
                .and_then(|value| value.to_str().ok())
                .or(authority),
        ),
        None => false,
    }
}

fn origin_matches(origin: &str, host: Option<&str>) -> bool {
    let Some(host) = host else {
        return false;
    };
    let authority = origin
        .split_once("://")
        .map_or(origin, |(_, authority)| authority)
        .trim_end_matches('/');
    authority.eq_ignore_ascii_case(host.trim())
}

pub(super) fn accept_language(headers: &HeaderMap, registry: &Registry) -> String {
    headers
        .get(ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok())
        .into_iter()
        .flat_map(|value| value.split(','))
        .filter_map(|range| range.split(';').next())
        .map(|tag| tag.trim().to_ascii_lowercase())
        .filter_map(|tag| tag.split('-').next().map(str::to_owned))
        .find(|lang| registry.i18n(lang).is_some())
        .unwrap_or_else(|| REQUIRED_LANGUAGE.to_owned())
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.append(*name, HeaderValue::from_static(value));
        }
        headers
    }

    #[test]
    fn a_bearer_token_wins_over_the_session_cookie() {
        let both = headers(&[
            ("authorization", "Bearer kat_1"),
            ("cookie", "__Host-keel_session=ks_1"),
        ]);
        match credential(&both) {
            Some(Credential::Bearer(token)) => assert_eq!(token.expose(), "kat_1"),
            other => panic!("expected a bearer credential, got {other:?}"),
        }
        let cookie = headers(&[("cookie", "theme=dark; __Host-keel_session=ks_2 ; x=1")]);
        match credential(&cookie) {
            Some(Credential::Session(token)) => assert_eq!(token.expose(), "ks_2"),
            other => panic!("expected a session credential, got {other:?}"),
        }
        assert!(credential(&headers(&[("cookie", "theme=dark")])).is_none());
        assert!(credential(&headers(&[("authorization", "Basic abc")])).is_none());
        assert!(credential(&headers(&[("authorization", "Bearer ")])).is_none());
        assert!(credential(&headers(&[("authorization", "bearer lower")])).is_some());
    }

    #[test]
    fn a_request_is_cross_site_when_the_browser_says_so_or_the_origin_is_foreign() {
        let refused = |pairs: &[(&'static str, &'static str)]| cross_site(&headers(pairs), None);
        assert!(refused(&[("origin", "https://evil.example")]));
        assert!(refused(&[("sec-fetch-site", "cross-site")]));
        assert!(!refused(&[("sec-fetch-site", "same-origin")]));
        assert!(refused(&[
            ("host", "panel.example:8443"),
            ("origin", "https://evil.example")
        ]));
        assert!(!refused(&[
            ("host", "panel.example:8443"),
            ("origin", "https://PANEL.example:8443")
        ]));
        assert!(refused(&[("host", "panel.example"), ("origin", "null")]));
        assert!(refused(&[("origin", "https://panel.example")]));
        assert!(!refused(&[("host", "panel.example")]));
        assert!(!refused(&[]));
        assert!(!cross_site(
            &headers(&[("origin", "https://panel.example")]),
            Some("panel.example")
        ));
    }

    #[test]
    fn the_guest_principal_has_no_identity_and_the_guest_level() {
        let guest = guest("ru");
        assert_eq!(guest.level, Level::Guest);
        assert_eq!(guest.login, GUEST_LOGIN);
        assert_eq!(guest.lang, "ru");
        assert_eq!(guest.id, Id::ROOT_ACCOUNT);
        assert!(guest.actor.is_none());
    }
}
