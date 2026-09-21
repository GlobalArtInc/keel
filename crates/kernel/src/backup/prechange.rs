use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use keel_sdk::sea_orm::DatabaseConnection;
use serde::Serialize;

use super::{BackupError, dump};

pub const MANIFEST: &str = "manifest.json";
const FORMAT: &str = "copy-text";

#[derive(Debug)]
pub struct PreChange {
    pub dir: PathBuf,
    pub schema: String,
    pub tables: Vec<String>,
}

#[derive(Serialize)]
struct Manifest<'a> {
    keel: &'a str,
    reason: &'a str,
    schema: &'a str,
    format: &'static str,
    tables: &'a [String],
    at: DateTime<Utc>,
}

pub(super) fn dir_of(backups: &Path, label: &str, version: &str, at: DateTime<Utc>) -> PathBuf {
    backups.join(format!(
        "pre-{label}-{version}-{}",
        at.format("%Y%m%dT%H%M%SZ")
    ))
}

pub(super) async fn write(
    db: &DatabaseConnection,
    dir: &Path,
    version: &str,
    reason: &str,
    at: DateTime<Utc>,
) -> Result<PreChange, BackupError> {
    let dumped = dump::write(db, dir).await?;
    let tables: Vec<String> = dumped.tables.into_iter().map(|table| table.name).collect();
    let manifest = dir.join(MANIFEST);
    let text = serde_json::to_vec_pretty(&Manifest {
        keel: version,
        reason,
        schema: &dumped.schema,
        format: FORMAT,
        tables: &tables,
        at,
    })
    .map_err(|source| BackupError::Encode {
        what: "the dump manifest",
        source,
    })?;
    tokio::fs::write(&manifest, text)
        .await
        .map_err(|source| BackupError::Write {
            path: manifest.clone(),
            source,
        })?;
    Ok(PreChange {
        dir: dir.to_owned(),
        schema: dumped.schema,
        tables,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dump_directory_names_the_change_the_version_and_the_instant() {
        let at: DateTime<Utc> = "2026-09-21T10:15:30Z".parse().unwrap();
        assert_eq!(
            dir_of(Path::new("/var/lib/keel/backups"), "restart", "0.1.0", at),
            PathBuf::from("/var/lib/keel/backups/pre-restart-0.1.0-20260921T101530Z")
        );
        let manifest = serde_json::to_value(Manifest {
            keel: "0.1.0",
            reason: "product enable dns",
            schema: "keel",
            format: FORMAT,
            tables: &["keel_kv".to_owned()],
            at,
        })
        .unwrap();
        assert_eq!(
            manifest,
            serde_json::json!({
                "keel": "0.1.0",
                "reason": "product enable dns",
                "schema": "keel",
                "format": "copy-text",
                "tables": ["keel_kv"],
                "at": "2026-09-21T10:15:30Z"
            })
        );
    }
}
