use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use keel_kernel::tls;
use keel_release::{Digest, Hasher};
use tokio::io::AsyncWriteExt;

use crate::VERSION;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(60);
const REDIRECT_LIMIT: usize = 10;

pub(crate) enum Source {
    Directory(PathBuf),
    Http {
        base: String,
        client: reqwest::Client,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum SourceError {
    #[error(
        "the channel \"{0}\" is neither an https:// URL, a file:// URL nor an absolute directory"
    )]
    Shape(String),
    #[error(
        "the channel \"{0}\" is plain http, which a release channel is not read over; --insecure-rehearsal-channel admits it for a rehearsal"
    )]
    PlainHttp(String),
    #[error("the channel directory {0} does not exist")]
    NoDirectory(PathBuf),
    #[error("the http client could not be built: {0}")]
    Client(#[source] reqwest::Error),
    #[error("could not fetch {location}: {source}")]
    Fetch {
        location: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("{location} answered {status}")]
    Status {
        location: String,
        status: reqwest::StatusCode,
    },
    #[error("could not read {location}: {source}")]
    Read {
        location: String,
        #[source]
        source: io::Error,
    },
    #[error("{location} is not in the channel")]
    Missing { location: String },
    #[error("{location} is over {limit} bytes")]
    TooLarge { location: String, limit: u64 },
    #[error("could not write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

fn plain_http_allowed(insecure_rehearsal: bool, base: &str) -> bool {
    insecure_rehearsal && base.starts_with("http://")
}

fn redirect_refusal(
    from: &reqwest::Url,
    to: &reqwest::Url,
    allow_plain_http: bool,
) -> Option<&'static str> {
    let scheme_kept = to.scheme() == "https" || (allow_plain_http && to.scheme() == "http");
    if !scheme_kept {
        return Some("the redirect leaves https");
    }
    if from.host_str() != to.host_str()
        || from.port_or_known_default() != to.port_or_known_default()
    {
        return Some("the redirect changes the host");
    }
    None
}

fn redirect_policy(allow_plain_http: bool) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= REDIRECT_LIMIT {
            return attempt.error(format!("more than {REDIRECT_LIMIT} redirects"));
        }
        let Some(from) = attempt.previous().last() else {
            return attempt.error("a redirect without a request before it");
        };
        match redirect_refusal(from, attempt.url(), allow_plain_http) {
            Some(reason) => {
                let message = format!("{reason}: {from} to {}", attempt.url());
                attempt.error(message)
            }
            None => attempt.follow(),
        }
    })
}

impl Source {
    pub(crate) fn parse(value: &str, insecure_rehearsal: bool) -> Result<Self, SourceError> {
        if let Some(path) = value.strip_prefix("file://") {
            return Self::directory(Path::new(path));
        }
        if value.starts_with('/') {
            return Self::directory(Path::new(value));
        }
        if value.starts_with("http://") && !insecure_rehearsal {
            return Err(SourceError::PlainHttp(value.to_owned()));
        }
        if value.starts_with("https://") || value.starts_with("http://") {
            let mut base = value.to_owned();
            if !base.ends_with('/') {
                base.push('/');
            }
            tls::install_provider();
            let client = reqwest::Client::builder()
                .user_agent(format!("keel/{VERSION}"))
                .connect_timeout(CONNECT_TIMEOUT)
                .read_timeout(READ_TIMEOUT)
                .redirect(redirect_policy(plain_http_allowed(
                    insecure_rehearsal,
                    &base,
                )))
                .build()
                .map_err(SourceError::Client)?;
            return Ok(Source::Http { base, client });
        }
        Err(SourceError::Shape(value.to_owned()))
    }

    fn directory(path: &Path) -> Result<Self, SourceError> {
        if !path.is_dir() {
            return Err(SourceError::NoDirectory(path.to_owned()));
        }
        Ok(Source::Directory(path.to_owned()))
    }

    fn location(&self, relative: &str) -> String {
        match self {
            Source::Directory(dir) => dir.join(relative).display().to_string(),
            Source::Http { base, .. } => format!("{base}{relative}"),
        }
    }

    pub(crate) async fn fetch(&self, relative: &str, limit: u64) -> Result<Vec<u8>, SourceError> {
        let location = self.location(relative);
        match self {
            Source::Directory(dir) => {
                let path = dir.join(relative);
                let metadata = tokio::fs::metadata(&path)
                    .await
                    .map_err(|source| missing_or_read(source, &location))?;
                if metadata.len() > limit {
                    return Err(SourceError::TooLarge { location, limit });
                }
                tokio::fs::read(&path)
                    .await
                    .map_err(|source| missing_or_read(source, &location))
            }
            Source::Http { client, .. } => {
                let mut response = self.get(client, &location).await?;
                let mut bytes = Vec::new();
                while let Some(chunk) =
                    response
                        .chunk()
                        .await
                        .map_err(|source| SourceError::Fetch {
                            location: location.clone(),
                            source,
                        })?
                {
                    if bytes.len() as u64 + chunk.len() as u64 > limit {
                        return Err(SourceError::TooLarge { location, limit });
                    }
                    bytes.extend_from_slice(&chunk);
                }
                Ok(bytes)
            }
        }
    }

    pub(crate) async fn fetch_to(
        &self,
        relative: &str,
        dest: &Path,
        limit: u64,
    ) -> Result<Digest, SourceError> {
        let location = self.location(relative);
        let write_failed = |source| SourceError::Write {
            path: dest.to_owned(),
            source,
        };
        let mut file = tokio::fs::File::create(dest).await.map_err(write_failed)?;
        let mut hasher = Hasher::default();
        let mut written = 0u64;
        match self {
            Source::Directory(dir) => {
                let path = dir.join(relative);
                let mut source = tokio::fs::File::open(&path)
                    .await
                    .map_err(|source| missing_or_read(source, &location))?;
                let mut buffer = vec![0u8; 64 * 1024];
                loop {
                    let read = tokio::io::AsyncReadExt::read(&mut source, &mut buffer)
                        .await
                        .map_err(|source| SourceError::Read {
                            location: location.clone(),
                            source,
                        })?;
                    if read == 0 {
                        break;
                    }
                    written += read as u64;
                    if written > limit {
                        return Err(SourceError::TooLarge { location, limit });
                    }
                    hasher.update(&buffer[..read]);
                    file.write_all(&buffer[..read])
                        .await
                        .map_err(write_failed)?;
                }
            }
            Source::Http { client, .. } => {
                let mut response = self.get(client, &location).await?;
                while let Some(chunk) =
                    response
                        .chunk()
                        .await
                        .map_err(|source| SourceError::Fetch {
                            location: location.clone(),
                            source,
                        })?
                {
                    written += chunk.len() as u64;
                    if written > limit {
                        return Err(SourceError::TooLarge { location, limit });
                    }
                    hasher.update(&chunk);
                    file.write_all(&chunk).await.map_err(write_failed)?;
                }
            }
        }
        file.flush().await.map_err(write_failed)?;
        Ok(hasher.finish())
    }

    async fn get(
        &self,
        client: &reqwest::Client,
        location: &str,
    ) -> Result<reqwest::Response, SourceError> {
        let response = client
            .get(location)
            .send()
            .await
            .map_err(|source| SourceError::Fetch {
                location: location.to_owned(),
                source,
            })?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(SourceError::Missing {
                location: location.to_owned(),
            });
        }
        if !status.is_success() {
            return Err(SourceError::Status {
                location: location.to_owned(),
                status,
            });
        }
        Ok(response)
    }
}

fn missing_or_read(source: io::Error, location: &str) -> SourceError {
    if source.kind() == io::ErrorKind::NotFound {
        SourceError::Missing {
            location: location.to_owned(),
        }
    } else {
        SourceError::Read {
            location: location.to_owned(),
            source,
        }
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Directory(dir) => write!(f, "{}", dir.display()),
            Source::Http { base, .. } => f.write_str(base),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_channel_value_is_a_directory_or_an_http_base_ending_in_a_slash() {
        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().display().to_string();
        assert!(
            matches!(Source::parse(&text, false).unwrap(), Source::Directory(path) if path == dir.path())
        );
        assert!(matches!(
            Source::parse(&format!("file://{text}"), false).unwrap(),
            Source::Directory(path) if path == dir.path()
        ));
        assert!(matches!(
            Source::parse("/nowhere/at/all", false),
            Err(SourceError::NoDirectory(_))
        ));
        assert!(matches!(
            Source::parse("https://dl.globalart.hu/keel", false).unwrap(),
            Source::Http { base, .. } if base == "https://dl.globalart.hu/keel/"
        ));
        assert!(matches!(
            Source::parse("ftp://dl.globalart.hu/keel/", false),
            Err(SourceError::Shape(value)) if value == "ftp://dl.globalart.hu/keel/"
        ));
        assert!(matches!(
            Source::parse("relative/dir", false),
            Err(SourceError::Shape(_))
        ));
        assert_eq!(
            Source::parse("https://dl.globalart.hu/keel/", false)
                .unwrap()
                .to_string(),
            "https://dl.globalart.hu/keel/"
        );
    }

    #[test]
    fn a_plain_http_channel_is_refused_without_the_rehearsal_flag() {
        for address in ["http://127.0.0.1:8080/keel", "http://dl.globalart.hu/keel/"] {
            assert!(matches!(
                Source::parse(address, false),
                Err(SourceError::PlainHttp(value)) if value == address
            ));
        }
        assert!(matches!(
            Source::parse("http://127.0.0.1:8080/keel", true).unwrap(),
            Source::Http { base, .. } if base == "http://127.0.0.1:8080/keel/"
        ));
        assert!(Source::parse("https://dl.globalart.hu/keel", true).is_ok());
        assert!(matches!(
            Source::parse("ftp://dl.globalart.hu/keel/", true),
            Err(SourceError::Shape(_))
        ));
        let dir = tempfile::tempdir().unwrap();
        assert!(Source::parse(&dir.path().display().to_string(), false).is_ok());
    }

    #[test]
    fn a_redirect_off_https_or_to_another_host_is_refused_and_a_rehearsal_lifts_plain_http_alone() {
        let url = |text: &str| reqwest::Url::parse(text).unwrap();
        let from = url("https://dl.globalart.hu/keel/stable/manifest.json");
        assert_eq!(
            redirect_refusal(
                &from,
                &url("https://dl.globalart.hu/other/manifest.json"),
                false
            ),
            None
        );
        assert_eq!(
            redirect_refusal(
                &from,
                &url("http://dl.globalart.hu/keel/stable/manifest.json"),
                false
            ),
            Some("the redirect leaves https")
        );
        assert_eq!(
            redirect_refusal(
                &from,
                &url("https://mirror.example.org/keel/stable/manifest.json"),
                false
            ),
            Some("the redirect changes the host")
        );
        assert_eq!(
            redirect_refusal(
                &from,
                &url("https://dl.globalart.hu:8443/keel/stable/manifest.json"),
                false
            ),
            Some("the redirect changes the host")
        );
        assert!(!plain_http_allowed(true, "https://dl.globalart.hu/keel/"));
        assert!(!plain_http_allowed(false, "http://127.0.0.1:8080/keel/"));
        assert!(plain_http_allowed(true, "http://127.0.0.1:8080/keel/"));
        assert_eq!(
            redirect_refusal(
                &from,
                &url("http://dl.globalart.hu/keel/stable/manifest.json"),
                plain_http_allowed(true, "https://dl.globalart.hu/keel/")
            ),
            Some("the redirect leaves https"),
            "the rehearsal setting with an https channel does not admit a redirect to http"
        );
        let rehearsal = url("http://127.0.0.1:8080/keel/stable/manifest.json");
        assert_eq!(
            redirect_refusal(&rehearsal, &url("http://127.0.0.1:8080/other"), true),
            None
        );
        assert_eq!(
            redirect_refusal(&rehearsal, &url("http://127.0.0.1:8080/other"), false),
            Some("the redirect leaves https")
        );
        assert_eq!(
            redirect_refusal(&rehearsal, &url("http://localhost:8080/other"), true),
            Some("the redirect changes the host")
        );
        assert_eq!(
            redirect_refusal(&rehearsal, &url("http://127.0.0.1:9090/other"), true),
            Some("the redirect changes the host")
        );
    }

    fn serve_redirects() -> std::net::SocketAddr {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                std::thread::spawn(move || {
                    let mut buffer = [0u8; 2048];
                    let read = stream.read(&mut buffer).unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
                    let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
                    let redirect = |host: &str| {
                        format!(
                            "HTTP/1.1 302 Found\r\nLocation: http://{host}:{}/final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                            addr.port()
                        )
                    };
                    let response = match path.as_str() {
                        "/final" => "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_owned(),
                        "/same" => redirect("127.0.0.1"),
                        "/other" => redirect("localhost"),
                        _ => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned(),
                    };
                    let _ = stream.write_all(response.as_bytes());
                });
            }
        });
        addr
    }

    fn chain(error: &dyn std::error::Error) -> String {
        crate::error::describe(error)
    }

    #[tokio::test]
    async fn a_real_client_follows_a_redirect_only_where_its_policy_allows_it() {
        tls::install_provider();
        let addr = serve_redirects();
        let base = format!("http://{addr}/");

        let strict = reqwest::Client::builder()
            .redirect(redirect_policy(false))
            .build()
            .unwrap();
        let refused = strict.get(format!("{base}same")).send().await.unwrap_err();
        assert!(refused.is_redirect());
        assert!(
            chain(&refused).contains("the redirect leaves https"),
            "{}",
            chain(&refused)
        );

        let source = Source::parse(&base, true).unwrap();
        assert_eq!(source.fetch("same", 1024).await.unwrap(), b"ok");
        let error = source.fetch("other", 1024).await.unwrap_err();
        let text = chain(&crate::privileged::PrivilegedError::from(
            crate::privileged::update::UpdateError::from(error),
        ));
        assert!(text.contains("could not fetch"), "{text}");
        assert!(
            text.contains("the redirect changes the host"),
            "the text that reaches the operator names the refused redirect: {text}"
        );
    }

    #[tokio::test]
    async fn a_directory_channel_answers_its_files_within_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("stable")).unwrap();
        std::fs::write(dir.path().join("stable/manifest.json"), b"head").unwrap();
        let source = Source::parse(&dir.path().display().to_string(), false).unwrap();
        assert_eq!(
            source.fetch("stable/manifest.json", 1024).await.unwrap(),
            b"head"
        );
        assert!(matches!(
            source.fetch("stable/manifest.json", 3).await,
            Err(SourceError::TooLarge { limit: 3, .. })
        ));
        assert!(matches!(
            source.fetch("beta/manifest.json", 1024).await,
            Err(SourceError::Missing { location }) if location.ends_with("beta/manifest.json")
        ));
        let dest = dir.path().join("copy");
        let digest = source
            .fetch_to("stable/manifest.json", &dest, 1024)
            .await
            .unwrap();
        assert_eq!(digest, Digest::of(b"head"));
        assert_eq!(std::fs::read(&dest).unwrap(), b"head");
        assert!(matches!(
            source.fetch_to("stable/manifest.json", &dest, 2).await,
            Err(SourceError::TooLarge { limit: 2, .. })
        ));
        assert!(matches!(
            source.fetch_to("stable/none.json", &dest, 2).await,
            Err(SourceError::Missing { .. })
        ));
    }
}
