use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, FromRequestParts, Request, State};
use axum::http::header::{ACCEPT_LANGUAGE, AUTHORIZATION, COOKIE, HOST, ORIGIN, USER_AGENT};
use axum::http::request::Parts;
use axum::http::{Extensions, HeaderMap};
use axum::middleware::Next;
use axum::response::Response;
use chrono_tz::Tz;
use keel_sdk::runtime::Db;
use keel_sdk::system::{
    Authenticated, Authenticator, Client, Credential, PreferenceSource, Preferences, SessionChange,
};
use keel_sdk::{KeelError, Secret};
use keel_types::{ErrorCode, Id, Level, Principal, Scope, Via};

use super::Shared;
use super::cookie;
use super::live::Live;
use super::reply;
use crate::loader::REQUIRED_LANGUAGE;
use crate::registry::Registry;

pub const SESSION_COOKIE: &str = "__Host-keel_session";
pub const GUEST_LOGIN: &str = "guest";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Zone(pub(super) Option<Tz>);

#[derive(Clone, Debug)]
pub(super) struct Caller {
    pub(super) request_id: String,
    pub(super) principal: Principal,
    pub(super) client: Client,
    pub(super) zone: Zone,
}

impl<S: Send + Sync> FromRequestParts<S> for Caller {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let request_id = reply::request_id_in(&parts.extensions);
        let (Some(principal), Some(client)) = (
            parts.extensions.get::<Principal>().cloned(),
            parts.extensions.get::<Client>().cloned(),
        ) else {
            return Err(reply::error(
                KeelError::internal("keel.error.unauthenticated_route"),
                &request_id,
            ));
        };
        Ok(Self {
            request_id,
            principal,
            client,
            zone: parts.extensions.get::<Zone>().copied().unwrap_or_default(),
        })
    }
}

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
    if !matches!(credential, Some(Credential::Bearer(_))) && cross_site(headers, authority) {
        return reply::error(KeelError::forbidden("keel.error.cross_site"), &request_id);
    }
    let mut client = client(headers, request.extensions());
    let mut zone = Zone::default();
    let principal = match credential {
        Some(credential) => {
            let db = Db::from_connection(state.db.clone());
            match resolve(&live.authenticators, &credential, &client, &db).await {
                Ok(authenticated) => {
                    client.session = authenticated.session;
                    let mut principal = authenticated.principal;
                    match preferences(&live.registry, &principal, &db).await {
                        Ok(preferred) => zone = overlay(&live.registry, &mut principal, preferred),
                        Err(error) => return reply::error(error, &request_id),
                    }
                    principal
                }
                Err(error) => {
                    let stale = matches!(credential, Credential::Session(_))
                        && error.code() == ErrorCode::Unauthenticated;
                    let mut response = reply::error(error, &request_id);
                    if stale {
                        cookie::apply(&mut response, &SessionChange::Clear);
                    }
                    return response;
                }
            }
        }
        None => guest(&accept_language(headers, &live.registry)),
    };
    request.extensions_mut().insert(principal);
    request.extensions_mut().insert(client);
    request.extensions_mut().insert(zone);
    next.run(request).await
}

async fn preferences(
    registry: &Registry,
    principal: &Principal,
    db: &Db,
) -> Result<Preferences, KeelError> {
    if principal.level == Level::Guest || principal.level >= Level::Owner {
        return Ok(Preferences::default());
    }
    match registry.services().get::<dyn PreferenceSource>() {
        Some(source) => source.preferences(principal.id, db).await,
        None => Ok(Preferences::default()),
    }
}

pub(super) fn overlay(
    registry: &Registry,
    principal: &mut Principal,
    preferred: Preferences,
) -> Zone {
    if let Some(lang) = preferred.lang
        && registry.i18n(&lang).is_some()
    {
        principal.lang = lang;
    }
    Zone(preferred.tz.and_then(|name| name.parse::<Tz>().ok()))
}

async fn resolve(
    authenticators: &[Arc<dyn Authenticator>],
    credential: &Credential,
    client: &Client,
    db: &Db,
) -> Result<Authenticated, KeelError> {
    for authenticator in authenticators {
        if let Some(authenticated) = authenticator.authenticate(credential, client, db).await? {
            return Ok(authenticated);
        }
    }
    Err(KeelError::unauthenticated("keel.error.unauthenticated"))
}

pub(super) fn client(headers: &HeaderMap, extensions: &Extensions) -> Client {
    Client {
        ip: extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| addr.ip()),
        user_agent: headers
            .get(USER_AGENT)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|agent| !agent.is_empty())
            .map(str::to_owned),
        session: None,
    }
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
    fn the_client_is_read_from_the_connection_and_the_user_agent_header() {
        let mut extensions = Extensions::new();
        let bare = client(&headers(&[("user-agent", " probe/1 ")]), &extensions);
        assert_eq!(bare.ip, None);
        assert_eq!(bare.user_agent.as_deref(), Some("probe/1"));
        assert!(bare.session.is_none());
        let addr: SocketAddr = "127.0.0.1:4321".parse().expect("an address");
        extensions.insert(ConnectInfo(addr));
        let connected = client(&headers(&[("user-agent", "")]), &extensions);
        assert_eq!(connected.ip, Some(addr.ip()));
        assert_eq!(connected.user_agent, None);
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
