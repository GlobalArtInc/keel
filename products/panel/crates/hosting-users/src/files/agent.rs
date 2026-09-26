use std::io;
use std::path::Path;

use keel::prelude::*;
use keel::sdk::bytes::Bytes;
use keel::sdk::chrono::{DateTime, Utc};
use keel::sdk::download::FILE_CHUNK;
use keel::sdk::futures::stream;
use keel::sdk::serde::de::DeserializeOwned;
use keel::sdk::tokio::fs::{self, File, OpenOptions};
use keel::sdk::tokio::io::{AsyncReadExt, AsyncWriteExt};
use keel_host::api::{Dest, HostExecutor, Origin};

use crate::PRODUCT;
use crate::config::HostingConfig;
use crate::user;

const OP_LIST: &str = "host.home.list";
const OP_MKDIR: &str = "host.home.mkdir";
const OP_RENAME: &str = "host.home.rename";
const OP_REMOVE: &str = "host.home.remove";
const OP_CHMOD: &str = "host.home.chmod";
const OP_READ: &str = "host.home.read";
const OP_WRITE: &str = "host.home.write";
const TRANSFER_ROOT: &str = "transfer";
const STAGED_PREFIX: &str = "keel-panel-";
const STAGED_MODE: u32 = 0o600;
const OCTET_STREAM: &str = "application/octet-stream";
const OUTSIDE: &str = "hosting.file.error.outside";
const MISSING: &str = "hosting.file.error.missing";
const EXISTS: &str = "hosting.file.error.exists";
const FAILED: &str = "hosting.file.error.failed";
const TRANSFER: &str = "hosting.file.error.transfer";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", rename_all = "lowercase")]
#[schemars(crate = "keel::sdk::schemars", rename = "HostingFileKind")]
enum FileKind {
    Dir,
    File,
    Symlink,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", rename_all = "camelCase")]
#[schemars(
    crate = "keel::sdk::schemars",
    rename = "HostingFileEntry",
    rename_all = "camelCase"
)]
struct FileEntry {
    name: String,
    kind: FileKind,
    size: u64,
    mode: String,
    modified: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", rename_all = "camelCase")]
#[schemars(
    crate = "keel::sdk::schemars",
    rename = "HostingFileListing",
    rename_all = "camelCase"
)]
pub(crate) struct Listing {
    entries: Vec<FileEntry>,
    truncated: bool,
}

#[derive(Deserialize)]
#[serde(crate = "keel::sdk::serde")]
struct Listed {
    entries: Vec<ListedEntry>,
    truncated: bool,
}

#[derive(Deserialize)]
#[serde(crate = "keel::sdk::serde")]
struct ListedEntry {
    name: String,
    kind: FileKind,
    size: u64,
    mode: String,
    modified: i64,
}

#[derive(Deserialize)]
#[serde(crate = "keel::sdk::serde")]
struct Nothing {}

pub(crate) struct Home {
    host: Arc<dyn HostExecutor>,
    origin: Origin,
    user: String,
}

impl Home {
    pub(crate) fn of<P>(call: &Call<P>, row: &user::Model) -> Result<Self> {
        Ok(Self {
            host: call.require::<dyn HostExecutor>()?,
            origin: Origin::new(PRODUCT, call.principal(), call.request_id()),
            user: row.name.clone(),
        })
    }

    async fn op<R: DeserializeOwned>(
        &self,
        name: &str,
        args: Value,
        field: Blame<'_>,
    ) -> Result<R> {
        self.host
            .on(self.origin.clone())
            .op(name, &args)
            .await
            .map_err(|error| field.remap(error))
    }

    pub(crate) async fn list(&self, path: &str) -> Result<Listing> {
        let listed: Listed = self
            .op(
                OP_LIST,
                json!({ "user": self.user, "path": path }),
                Blame::path(path),
            )
            .await?;
        Ok(Listing {
            entries: listed
                .entries
                .into_iter()
                .map(|entry| FileEntry {
                    name: entry.name,
                    kind: entry.kind,
                    size: entry.size,
                    mode: entry.mode,
                    modified: DateTime::from_timestamp(entry.modified, 0),
                })
                .collect(),
            truncated: listed.truncated,
        })
    }

    pub(crate) async fn mkdir(&self, path: &str) -> Result<()> {
        let _: Nothing = self
            .op(
                OP_MKDIR,
                json!({ "user": self.user, "path": path }),
                Blame::path(path),
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn rename(&self, from: &str, to: &str) -> Result<()> {
        let _: Nothing = self
            .op(
                OP_RENAME,
                json!({ "user": self.user, "from": from, "to": to }),
                Blame::rename(from, to),
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn remove(&self, path: &str) -> Result<()> {
        let _: Nothing = self
            .op(
                OP_REMOVE,
                json!({ "user": self.user, "path": path }),
                Blame::path(path),
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn chmod(&self, path: &str, mode: &str) -> Result<()> {
        let _: Nothing = self
            .op(
                OP_CHMOD,
                json!({ "user": self.user, "path": path, "mode": mode }),
                Blame::path(path),
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn write(
        &self,
        config: &HostingConfig,
        path: &str,
        staged: &str,
    ) -> Result<()> {
        let written: Result<Value> = self
            .op(
                OP_WRITE,
                json!({ "user": self.user, "path": path, "source": Dest::new(TRANSFER_ROOT, staged) }),
                Blame::path(path),
            )
            .await;
        discard(&config.transfer_path(staged)).await;
        written.map(drop)
    }

    pub(crate) async fn download(&self, config: &HostingConfig, path: &str) -> Result<Download> {
        let staged = staged_name("download");
        let _: Value = self
            .op(
                OP_READ,
                json!({ "user": self.user, "path": path, "dest": Dest::new(TRANSFER_ROOT, &staged) }),
                Blame::path(path),
            )
            .await?;
        let local = config.transfer_path(&staged);
        let file = File::open(&local).await;
        discard(&local).await;
        let file = file.map_err(|error| transfer_error(&local, error))?;
        Ok(Download::new(filename(path), OCTET_STREAM, body(file)))
    }
}

pub(crate) fn staged_name(kind: &str) -> String {
    format!("{STAGED_PREFIX}{kind}-{}", Id::generate())
}

pub(crate) async fn stage(config: &HostingConfig, name: &str, bytes: &[u8]) -> Result<()> {
    let path = config.transfer_path(name);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(STAGED_MODE)
        .open(&path)
        .await
        .map_err(|error| transfer_error(&path, error))?;
    let written = async {
        file.write_all(bytes).await?;
        file.sync_all().await
    }
    .await;
    if let Err(error) = written {
        discard(&path).await;
        return Err(transfer_error(&path, error));
    }
    Ok(())
}

pub(crate) async fn unstage(config: &HostingConfig, name: &str) {
    discard(&config.transfer_path(name)).await;
}

async fn discard(path: &Path) {
    let _ = fs::remove_file(path).await;
}

fn transfer_error(path: &Path, error: io::Error) -> KeelError {
    KeelError::internal(TRANSFER)
        .param("path", path.display().to_string())
        .source(error)
}

fn filename(path: &str) -> String {
    path.rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or("download")
        .to_owned()
}

fn body(file: File) -> impl keel::sdk::futures::Stream<Item = io::Result<Bytes>> + Send + 'static {
    stream::try_unfold(file, |mut file| async move {
        let mut chunk = vec![0; FILE_CHUNK];
        let read = file.read(&mut chunk).await?;
        if read == 0 {
            return Ok(None);
        }
        chunk.truncate(read);
        Ok(Some((Bytes::from(chunk), file)))
    })
}

#[derive(Clone, Copy)]
struct Blame<'a> {
    path: &'a str,
    taken: &'a str,
}

impl<'a> Blame<'a> {
    fn path(path: &'a str) -> Self {
        Self { path, taken: path }
    }

    fn rename(from: &'a str, to: &'a str) -> Self {
        Self {
            path: from,
            taken: to,
        }
    }

    fn remap(self, error: KeelError) -> KeelError {
        match error.key() {
            "host.error.policy" => KeelError::forbidden(OUTSIDE).param("path", self.path),
            "host.error.not_found" => KeelError::not_found(MISSING).param("path", self.path),
            "host.error.exists" => KeelError::conflict(EXISTS).param("path", self.taken),
            "host.error.args" | "host.error.io" => {
                let detail = error
                    .params()
                    .get("detail")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                KeelError::validation(FAILED)
                    .param("path", self.path)
                    .param("detail", detail)
            }
            _ => error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agent_errors_become_file_errors_naming_the_path_that_caused_them() {
        let outside = Blame::path("../x").remap(
            KeelError::forbidden("host.error.policy").param("detail", "../x leaves the home"),
        );
        assert_eq!((outside.key(), outside.status()), (OUTSIDE, 403));
        assert_eq!(outside.params()["path"], "../x");
        let taken = Blame::rename("a", "b").remap(KeelError::internal("host.error.exists"));
        assert_eq!((taken.key(), taken.status()), (EXISTS, 409));
        assert_eq!(taken.params()["path"], "b");
        let missing = Blame::rename("a", "b").remap(KeelError::not_found("host.error.not_found"));
        assert_eq!((missing.key(), missing.status()), (MISSING, 404));
        assert_eq!(missing.params()["path"], "a");
        let denied = Blame::path("x")
            .remap(KeelError::internal("host.error.io").param("detail", "Permission denied"));
        assert_eq!((denied.key(), denied.status()), (FAILED, 422));
        assert_eq!(denied.params()["detail"], "Permission denied");
        let other = Blame::path("x").remap(KeelError::unavailable("host.error.unreachable"));
        assert_eq!(other.key(), "host.error.unreachable");
    }

    #[test]
    fn a_download_is_named_after_the_last_part_of_its_path() {
        assert_eq!(filename("site/index.html"), "index.html");
        assert_eq!(filename("site/logs/"), "logs");
        assert_eq!(filename(""), "download");
        assert!(staged_name("upload").starts_with("keel-panel-upload-"));
    }
}
