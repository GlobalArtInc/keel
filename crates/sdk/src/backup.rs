use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::future::BoxFuture;

use crate::db::Db;
use crate::error::Result;

pub type SourceHook = Arc<dyn Fn(BackupDir) -> BoxFuture<'static, Result<()>> + Send + Sync>;

pub struct BackupSource {
    name: String,
    write: SourceHook,
    restore: Option<SourceHook>,
}

impl BackupSource {
    pub fn new<F, Fut>(name: &str, write: F) -> Self
    where
        F: Fn(BackupDir) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        Self {
            name: name.to_owned(),
            write: Arc::new(move |dir| Box::pin(write(dir))),
            restore: None,
        }
    }

    pub fn restore<F, Fut>(mut self, restore: F) -> Self
    where
        F: Fn(BackupDir) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.restore = Some(Arc::new(move |dir| Box::pin(restore(dir))));
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn module(&self) -> &str {
        self.name
            .split_once('.')
            .map_or(&self.name, |(module, _)| module)
    }

    fn restores(&self) -> bool {
        self.restore.is_some()
    }

    #[doc(hidden)]
    pub fn write_hook(&self) -> SourceHook {
        self.write.clone()
    }

    #[doc(hidden)]
    pub fn restore_hook(&self) -> Option<SourceHook> {
        self.restore.clone()
    }
}

impl fmt::Debug for BackupSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BackupSource")
            .field("name", &self.name)
            .field("restores", &self.restores())
            .finish()
    }
}

#[derive(Clone)]
pub struct BackupDir {
    source: Arc<str>,
    path: PathBuf,
    db: Db,
    data_dir: Arc<Path>,
}

impl BackupDir {
    #[doc(hidden)]
    pub fn new(source: &str, path: PathBuf, db: Db, data_dir: &Path) -> Self {
        Self {
            source: Arc::from(source),
            path,
            db,
            data_dir: Arc::from(data_dir),
        }
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn db(&self) -> &Db {
        &self.db
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
}

impl fmt::Debug for BackupDir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BackupDir")
            .field("source", &self.source)
            .field("path", &self.path)
            .field("data_dir", &self.data_dir)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use sea_orm::DatabaseConnection;

    use super::*;

    #[tokio::test]
    async fn a_source_names_its_module_and_runs_its_hooks_with_the_directory() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let written = seen.clone();
        let restored = seen.clone();
        let source = BackupSource::new("dns.zonefiles", move |dir: BackupDir| {
            let written = written.clone();
            async move {
                written.lock().unwrap().push(format!(
                    "write {} {}",
                    dir.source(),
                    dir.path().display()
                ));
                Ok(())
            }
        });
        assert_eq!(source.name(), "dns.zonefiles");
        assert_eq!(source.module(), "dns");
        assert!(!source.restores());
        assert!(source.restore_hook().is_none());
        assert_eq!(
            format!("{source:?}"),
            "BackupSource { name: \"dns.zonefiles\", restores: false }"
        );
        let source = source.restore(move |dir: BackupDir| {
            let restored = restored.clone();
            async move {
                restored
                    .lock()
                    .unwrap()
                    .push(format!("restore {}", dir.data_dir().display()));
                Ok(())
            }
        });
        assert!(source.restores());
        let dir = BackupDir::new(
            "dns.zonefiles",
            PathBuf::from("/tmp/staging/sources/dns.zonefiles"),
            Db::from_connection(DatabaseConnection::Disconnected),
            Path::new("/var/lib/keel"),
        );
        assert!(!dir.db().is_transaction());
        assert!(format!("{dir:?}").starts_with("BackupDir { source: \"dns.zonefiles\""));
        (source.write_hook())(dir.clone()).await.unwrap();
        (source.restore_hook().expect("a restore hook"))(dir)
            .await
            .unwrap();
        assert_eq!(
            *seen.lock().unwrap(),
            [
                "write dns.zonefiles /tmp/staging/sources/dns.zonefiles",
                "restore /var/lib/keel"
            ]
        );
        assert_eq!(
            BackupSource::new("bare", |_| async { Ok(()) }).module(),
            "bare"
        );
    }
}
