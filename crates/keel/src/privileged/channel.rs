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

impl Source {
    pub(crate) fn parse(value: &str) -> Result<Self, SourceError> {
        if let Some(path) = value.strip_prefix("file://") {
            return Self::directory(Path::new(path));
        }
        if value.starts_with('/') {
            return Self::directory(Path::new(value));
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
            matches!(Source::parse(&text).unwrap(), Source::Directory(path) if path == dir.path())
        );
        assert!(matches!(
            Source::parse(&format!("file://{text}")).unwrap(),
            Source::Directory(path) if path == dir.path()
        ));
        assert!(matches!(
            Source::parse("/nowhere/at/all"),
            Err(SourceError::NoDirectory(_))
        ));
        assert!(matches!(
            Source::parse("https://dl.globalart.hu/keel").unwrap(),
            Source::Http { base, .. } if base == "https://dl.globalart.hu/keel/"
        ));
        assert!(matches!(
            Source::parse("ftp://dl.globalart.hu/keel/"),
            Err(SourceError::Shape(value)) if value == "ftp://dl.globalart.hu/keel/"
        ));
        assert!(matches!(
            Source::parse("relative/dir"),
            Err(SourceError::Shape(_))
        ));
        assert_eq!(
            Source::parse("https://dl.globalart.hu/keel/")
                .unwrap()
                .to_string(),
            "https://dl.globalart.hu/keel/"
        );
    }

    #[tokio::test]
    async fn a_directory_channel_answers_its_files_within_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("stable")).unwrap();
        std::fs::write(dir.path().join("stable/manifest.json"), b"head").unwrap();
        let source = Source::parse(&dir.path().display().to_string()).unwrap();
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
