use std::net::{IpAddr, SocketAddr};

use axum::extract::ConnectInfo;
use axum::http::header::HOST;
use axum::http::uri::Authority;
use axum::http::{Extensions, HeaderMap, HeaderName, Uri};
use ipnet::IpNet;

pub(super) const X_FORWARDED_FOR: HeaderName = HeaderName::from_static("x-forwarded-for");
pub(super) const X_FORWARDED_HOST: HeaderName = HeaderName::from_static("x-forwarded-host");

pub(super) fn peer(extensions: &Extensions) -> Option<IpAddr> {
    extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip())
}

pub(super) fn is_trusted(peer: Option<IpAddr>, trusted: &[IpNet]) -> bool {
    peer.is_some_and(|ip| trusted.iter().any(|net| net.contains(&ip)))
}

pub(super) fn forwarded_for(headers: &HeaderMap, trusted: &[IpNet]) -> Option<IpAddr> {
    let mut entries = Vec::new();
    for value in headers.get_all(X_FORWARDED_FOR) {
        let Ok(text) = value.to_str() else {
            tracing::debug!("X-Forwarded-For is not a text header; the peer address stands");
            return None;
        };
        entries.extend(text.split(',').map(str::trim));
    }
    if entries.is_empty() {
        return None;
    }
    for entry in entries.iter().rev() {
        let Ok(ip) = entry.parse::<IpAddr>() else {
            tracing::debug!(
                entry,
                "X-Forwarded-For holds an entry that is not an IP address; the peer address stands"
            );
            return None;
        };
        if !trusted.iter().any(|net| net.contains(&ip)) {
            return Some(ip);
        }
    }
    tracing::debug!("every X-Forwarded-For entry is a trusted proxy; the peer address stands");
    None
}

pub(super) fn forwarded_host(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers.get_all(X_FORWARDED_HOST).iter();
    let value = values.next()?;
    if values.next().is_some() {
        tracing::debug!("X-Forwarded-Host is repeated; the Host header stands");
        return None;
    }
    let text = value.to_str().ok()?.trim();
    if is_authority(text) {
        Some(text)
    } else {
        tracing::debug!(
            host = text,
            "X-Forwarded-Host is not a single authority; the Host header stands"
        );
        None
    }
}

fn is_authority(text: &str) -> bool {
    !text.is_empty()
        && !text.contains([',', '/', '@', '?', '#'])
        && !text.contains(char::is_whitespace)
        && !text.contains("://")
        && text.parse::<Authority>().is_ok()
}

pub(super) fn authority<'a>(
    headers: &'a HeaderMap,
    uri: &'a Uri,
    trusted_peer: bool,
) -> Option<&'a str> {
    if trusted_peer && let Some(host) = forwarded_host(headers) {
        return Some(host);
    }
    headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| uri.authority().map(Authority::as_str))
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

    fn nets(list: &[&str]) -> Vec<IpNet> {
        list.iter().map(|net| net.parse().expect("a net")).collect()
    }

    #[test]
    fn the_rightmost_untrusted_forwarded_entry_wins_and_a_bad_entry_yields_nothing() {
        let trusted = nets(&["10.0.0.0/8", "192.168.1.1/32"]);
        let chain = headers(&[("x-forwarded-for", "203.0.113.9, 198.51.100.2, 10.1.2.3")]);
        assert_eq!(
            forwarded_for(&chain, &trusted),
            Some("198.51.100.2".parse().expect("an address"))
        );
        let repeated = headers(&[
            ("x-forwarded-for", "203.0.113.9"),
            ("x-forwarded-for", "10.9.9.9, 192.168.1.1"),
        ]);
        assert_eq!(
            forwarded_for(&repeated, &trusted),
            Some("203.0.113.9".parse().expect("an address"))
        );
        let only_proxies = headers(&[("x-forwarded-for", "10.1.1.1, 192.168.1.1")]);
        assert_eq!(forwarded_for(&only_proxies, &trusted), None);
        let malformed = headers(&[("x-forwarded-for", "203.0.113.9, unknown")]);
        assert_eq!(forwarded_for(&malformed, &trusted), None);
        let with_port = headers(&[("x-forwarded-for", "203.0.113.9:1234")]);
        assert_eq!(forwarded_for(&with_port, &trusted), None);
        assert_eq!(forwarded_for(&headers(&[]), &trusted), None);
        let v6 = headers(&[("x-forwarded-for", "2001:db8::7, 10.0.0.1")]);
        assert_eq!(
            forwarded_for(&v6, &trusted),
            Some("2001:db8::7".parse().expect("an address"))
        );
    }

    #[test]
    fn a_forwarded_host_is_taken_only_as_a_single_authority() {
        for good in [
            "panel.example",
            "panel.example:8443",
            "[::1]:1500",
            "127.0.0.1",
        ] {
            let map = headers(&[("x-forwarded-host", good)]);
            assert_eq!(forwarded_host(&map), Some(good), "{good}");
        }
        for bad in [
            "",
            "https://panel.example",
            "panel.example/path",
            "a.example, b.example",
            "user@panel.example",
            "panel example",
            "panel.example?x=1",
        ] {
            let map = headers(&[("x-forwarded-host", bad)]);
            assert_eq!(forwarded_host(&map), None, "{bad:?}");
        }
        let repeated = headers(&[
            ("x-forwarded-host", "a.example"),
            ("x-forwarded-host", "b.example"),
        ]);
        assert_eq!(forwarded_host(&repeated), None);
    }

    #[test]
    fn the_authority_prefers_the_forwarded_host_behind_a_trusted_peer_alone() {
        let uri = Uri::from_static("/api/v1/boot");
        let both = headers(&[
            ("host", "upstream.internal:1500"),
            ("x-forwarded-host", "panel.example"),
        ]);
        assert_eq!(authority(&both, &uri, true), Some("panel.example"));
        assert_eq!(
            authority(&both, &uri, false),
            Some("upstream.internal:1500")
        );
        let bad = headers(&[
            ("host", "upstream.internal"),
            ("x-forwarded-host", "https://panel.example"),
        ]);
        assert_eq!(authority(&bad, &uri, true), Some("upstream.internal"));
        let absolute = Uri::from_static("http://keel.test:9/api/v1/boot");
        assert_eq!(
            authority(&headers(&[]), &absolute, true),
            Some("keel.test:9")
        );
        assert_eq!(authority(&headers(&[]), &uri, true), None);
        let trusted = nets(&["127.0.0.0/8"]);
        assert!(is_trusted(Some(IpAddr::from([127, 0, 0, 1])), &trusted));
        assert!(!is_trusted(Some(IpAddr::from([10, 0, 0, 1])), &trusted));
        assert!(!is_trusted(None, &trusted));
    }
}
