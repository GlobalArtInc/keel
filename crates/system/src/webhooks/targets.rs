use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use keel_sdk::async_trait::async_trait;
use keel_sdk::{KeelError, Result};
use reqwest::Url;

pub const ERROR_URL: &str = "webhook.error.url";
pub const ERROR_URL_PRIVATE: &str = "webhook.error.url_private";
pub const ERROR_URL_UNRESOLVED: &str = "webhook.error.url_unresolved";
pub const SCHEME_HTTPS: &str = "https";
pub const SCHEME_HTTP: &str = "http";

#[async_trait]
pub trait Resolver: Send + Sync {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>>;
}

pub struct SystemResolver;

#[async_trait]
impl Resolver for SystemResolver {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>> {
        let answered = tokio::net::lookup_host((host, port))
            .await
            .map_err(|error| {
                KeelError::validation(ERROR_URL_UNRESOLVED)
                    .field("url")
                    .param("host", host)
                    .param("reason", error.to_string())
            })?;
        Ok(answered.collect())
    }
}

pub fn resolver_or_system(found: Option<Arc<dyn Resolver>>) -> Arc<dyn Resolver> {
    found.unwrap_or_else(|| Arc::new(SystemResolver))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub url: Url,
    pub host: String,
    pub port: u16,
    pub addrs: Vec<SocketAddr>,
    pub pinned: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    Scheme(String),
    IpLiteral(IpAddr),
    Private(IpAddr),
}

impl Refusal {
    pub fn reason(&self) -> &'static str {
        match self {
            Refusal::Scheme(_) => "scheme",
            Refusal::IpLiteral(_) => "ip_literal",
            Refusal::Private(_) => "private_address",
        }
    }

    pub fn detail(&self) -> String {
        match self {
            Refusal::Scheme(scheme) => scheme.clone(),
            Refusal::IpLiteral(ip) | Refusal::Private(ip) => ip.to_string(),
        }
    }
}

impl From<Refusal> for KeelError {
    fn from(refusal: Refusal) -> Self {
        KeelError::validation(ERROR_URL_PRIVATE)
            .field("url")
            .param("reason", refusal.reason())
            .param("detail", refusal.detail())
    }
}

pub fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback() || v4.is_link_local() || v4.is_private() || v4.is_unspecified()
        }
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_private(IpAddr::V4(v4)),
            None => {
                v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_unique_local()
                    || v6.is_unicast_link_local()
            }
        },
    }
}

pub fn parse(text: &str) -> Result<Url> {
    let url = Url::parse(text).map_err(|error| {
        KeelError::validation(ERROR_URL)
            .field("url")
            .param("reason", error.to_string())
    })?;
    if !matches!(url.scheme(), SCHEME_HTTP | SCHEME_HTTPS) || url.host_str().is_none() {
        return Err(KeelError::validation(ERROR_URL)
            .field("url")
            .param("reason", "not a web address"));
    }
    Ok(url)
}

fn literal(url: &Url) -> Option<IpAddr> {
    let host = url.host_str()?;
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .parse()
        .ok()
}

pub async fn check(text: &str, allow_private: bool, resolver: &dyn Resolver) -> Result<Target> {
    let url = parse(text)?;
    if !allow_private && url.scheme() != SCHEME_HTTPS {
        return Err(Refusal::Scheme(url.scheme().to_owned()).into());
    }
    let host = url.host_str().unwrap_or_default().to_owned();
    let port = url
        .port_or_known_default()
        .ok_or_else(|| KeelError::validation(ERROR_URL).field("url"))?;
    if let Some(ip) = literal(&url) {
        if !allow_private {
            return Err(Refusal::IpLiteral(ip).into());
        }
        return Ok(Target {
            url,
            host,
            port,
            addrs: vec![SocketAddr::new(ip, port)],
            pinned: false,
        });
    }
    let addrs = resolver.resolve(&host, port).await?;
    if addrs.is_empty() {
        return Err(KeelError::validation(ERROR_URL_UNRESOLVED)
            .field("url")
            .param("host", host)
            .param("reason", "no address"));
    }
    if !allow_private && let Some(private) = addrs.iter().find(|addr| is_private(addr.ip())) {
        return Err(Refusal::Private(private.ip()).into());
    }
    Ok(Target {
        url,
        host,
        port,
        addrs,
        pinned: true,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use keel_sdk::serde_json::json;
    use keel_sdk::types::ErrorCode;

    use super::*;

    struct Fixed(Mutex<Vec<Vec<IpAddr>>>);

    #[async_trait]
    impl Resolver for Fixed {
        async fn resolve(&self, _: &str, port: u16) -> Result<Vec<SocketAddr>> {
            let mut answers = self.0.lock().unwrap();
            let ips = if answers.len() > 1 {
                answers.remove(0)
            } else {
                answers.first().cloned().unwrap_or_default()
            };
            Ok(ips
                .into_iter()
                .map(|ip| SocketAddr::new(ip, port))
                .collect())
        }
    }

    fn resolving(ips: &[&str]) -> Fixed {
        Fixed(Mutex::new(vec![
            ips.iter().map(|ip| ip.parse().unwrap()).collect(),
        ]))
    }

    fn refused(error: &KeelError, reason: &str) {
        assert_eq!(error.code(), ErrorCode::Validation);
        assert_eq!(error.key(), ERROR_URL_PRIVATE);
        assert_eq!(error.params()["reason"], json!(reason), "{error:?}");
        assert_eq!(error.field_path(), Some("url"));
    }

    #[test]
    fn private_addresses_are_the_loopback_link_local_rfc1918_unique_local_and_unspecified_ranges() {
        for private in [
            "127.0.0.1",
            "127.8.8.8",
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "0.0.0.0",
            "::1",
            "::",
            "fc00::1",
            "fd12::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:10.1.2.3",
        ] {
            assert!(is_private(private.parse().unwrap()), "{private}");
        }
        for public in [
            "93.184.216.34",
            "8.8.8.8",
            "172.32.0.1",
            "11.0.0.1",
            "2606:4700::1111",
            "::ffff:8.8.8.8",
        ] {
            assert!(!is_private(public.parse().unwrap()), "{public}");
        }
    }

    #[tokio::test]
    async fn the_check_refuses_plain_schemes_ip_literals_and_names_resolving_to_private_addresses()
    {
        let public = resolving(&["93.184.216.34"]);
        let target = check("https://hooks.example/in", false, &public)
            .await
            .unwrap();
        assert_eq!(target.host, "hooks.example");
        assert_eq!(target.port, 443);
        assert_eq!(target.addrs, ["93.184.216.34:443".parse().unwrap()]);
        assert!(target.pinned);
        refused(
            &check("http://hooks.example/in", false, &public)
                .await
                .unwrap_err(),
            "scheme",
        );
        refused(
            &check("https://8.8.8.8/in", false, &public)
                .await
                .unwrap_err(),
            "ip_literal",
        );
        refused(
            &check("https://[2606:4700::1111]/in", false, &public)
                .await
                .unwrap_err(),
            "ip_literal",
        );
        let mixed = resolving(&["93.184.216.34", "10.0.0.7"]);
        let error = check("https://hooks.example/in", false, &mixed)
            .await
            .unwrap_err();
        refused(&error, "private_address");
        assert_eq!(error.params()["detail"], json!("10.0.0.7"));
        let mapped = resolving(&["::ffff:127.0.0.1"]);
        refused(
            &check("https://hooks.example/in", false, &mapped)
                .await
                .unwrap_err(),
            "private_address",
        );

        let loopback = resolving(&["127.0.0.1"]);
        let allowed = check("http://hooks.example:8080/in", true, &loopback)
            .await
            .unwrap();
        assert_eq!(allowed.port, 8080);
        assert_eq!(allowed.addrs, ["127.0.0.1:8080".parse().unwrap()]);
        let literal = check("http://127.0.0.1:9000/in", true, &loopback)
            .await
            .unwrap();
        assert_eq!(literal.host, "127.0.0.1");
        assert_eq!(literal.addrs, ["127.0.0.1:9000".parse().unwrap()]);
        assert!(!literal.pinned);
        let bracketed = check("http://[::1]:9000/in", true, &loopback)
            .await
            .unwrap();
        assert_eq!(bracketed.addrs, ["[::1]:9000".parse().unwrap()]);

        for broken in [
            "ftp://hooks.example/in",
            "not a url",
            "mailto:ops@example.org",
        ] {
            let error = check(broken, true, &loopback).await.unwrap_err();
            assert_eq!(error.key(), ERROR_URL, "{broken}");
            assert_eq!(error.field_path(), Some("url"));
        }
        let empty = resolving(&[]);
        let error = check("https://nowhere.example/in", false, &empty)
            .await
            .unwrap_err();
        assert_eq!(error.key(), ERROR_URL_UNRESOLVED);
    }
}
