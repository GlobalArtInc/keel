use std::path::Path;
use std::sync::Arc;

use keel_sdk::runtime::{Contributions, Services};
use keel_sdk::sea_orm::DatabaseConnection;
use keel_sdk::{BackupDir, BackupSource, Db};

use super::BackupError;

pub async fn write_all(
    contributions: &Contributions,
    services: &Arc<Services>,
    db: &DatabaseConnection,
    into: &Path,
    data_dir: &Path,
) -> Result<Vec<String>, BackupError> {
    let mut written = Vec::new();
    for source in contributions.of::<BackupSource>() {
        let dir = into.join(source.name());
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|error| BackupError::Create {
                path: dir.clone(),
                source: error,
            })?;
        let handle = BackupDir::new(
            source.name(),
            dir,
            Db::from_connection(db.clone()),
            data_dir,
            Arc::clone(services),
        );
        (source.write_hook())(handle)
            .await
            .map_err(|error| BackupError::Source {
                name: source.name().to_owned(),
                source: error,
            })?;
        written.push(source.name().to_owned());
    }
    written.sort();
    Ok(written)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourcesRestored {
    pub restored: Vec<String>,
    pub skipped: Vec<String>,
}

pub async fn restore_all(
    contributions: &Contributions,
    services: &Arc<Services>,
    db: &DatabaseConnection,
    from: &Path,
    names: &[String],
    data_dir: &Path,
) -> Result<SourcesRestored, BackupError> {
    let mut outcome = SourcesRestored::default();
    for name in names {
        let dir = from.join(name);
        let hook = contributions
            .of::<BackupSource>()
            .find(|source| source.name() == name)
            .and_then(BackupSource::restore_hook);
        let Some(hook) = hook else {
            outcome.skipped.push(name.clone());
            continue;
        };
        if !dir.is_dir() {
            outcome.skipped.push(name.clone());
            continue;
        }
        hook(BackupDir::new(
            name,
            dir,
            Db::from_connection(db.clone()),
            data_dir,
            Arc::clone(services),
        ))
        .await
        .map_err(|error| BackupError::Source {
            name: name.clone(),
            source: error,
        })?;
        outcome.restored.push(name.clone());
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use keel_sdk::KeelError;

    use super::*;

    #[tokio::test]
    async fn every_source_writes_into_its_own_directory_and_restores_only_with_a_hook() {
        let dir = tempfile::tempdir().unwrap();
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let mut contributions = Contributions::default();
        let zones_seen = seen.clone();
        contributions.add(
            BackupSource::new("dns.zonefiles", |dir: BackupDir| async move {
                std::fs::write(dir.path().join("a.zone"), "zone a")
                    .map_err(|error| KeelError::internal("dns.error.write").source(error))?;
                Ok(())
            })
            .restore(move |dir: BackupDir| {
                let seen = zones_seen.clone();
                async move {
                    let text = std::fs::read_to_string(dir.path().join("a.zone"))
                        .map_err(|error| KeelError::internal("dns.error.read").source(error))?;
                    seen.lock()
                        .unwrap()
                        .push(format!("{} {text}", dir.source()));
                    Ok(())
                }
            }),
        );
        contributions.add(BackupSource::new(
            "mail.spool",
            |dir: BackupDir| async move {
                std::fs::write(dir.path().join("queue"), "empty")
                    .map_err(|error| KeelError::internal("mail.error.write").source(error))?;
                Ok(())
            },
        ));
        let db = DatabaseConnection::Disconnected;
        let services = Arc::new(Services::new());
        let staging = dir.path().join("sources");
        let written = write_all(&contributions, &services, &db, &staging, dir.path())
            .await
            .unwrap();
        assert_eq!(written, ["dns.zonefiles", "mail.spool"]);
        assert_eq!(
            std::fs::read_to_string(staging.join("dns.zonefiles/a.zone")).unwrap(),
            "zone a"
        );
        let restored = restore_all(
            &contributions,
            &services,
            &db,
            &staging,
            &[
                "dns.zonefiles".to_owned(),
                "mail.spool".to_owned(),
                "gone.source".to_owned(),
            ],
            dir.path(),
        )
        .await
        .unwrap();
        assert_eq!(restored.restored, ["dns.zonefiles"]);
        assert_eq!(restored.skipped, ["mail.spool", "gone.source"]);
        assert_eq!(*seen.lock().unwrap(), ["dns.zonefiles zone a"]);

        let mut failing = Contributions::default();
        failing.add(BackupSource::new("x.fails", |_: BackupDir| async {
            Err(KeelError::internal("x.error.boom"))
        }));
        let error = write_all(&failing, &services, &db, &staging, dir.path())
            .await
            .unwrap_err();
        assert!(
            matches!(&error, BackupError::Source { name, .. } if name == "x.fails"),
            "{error}"
        );
        assert_eq!(
            restore_all(
                &Contributions::default(),
                &services,
                &db,
                &PathBuf::from("/nowhere"),
                &[],
                dir.path()
            )
            .await
            .unwrap(),
            SourcesRestored::default()
        );
    }
}
