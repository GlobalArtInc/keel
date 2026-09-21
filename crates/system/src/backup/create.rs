use std::path::Path;

use keel_sdk::prelude::*;
use keel_sdk::system::Backups;
use keel_sdk::types::ScreenResult;

use super::CREATE;
use super::list::ARCHIVE_SUFFIX;

pub const CLASS: &str = "backup";
pub const ERROR_INLINE: &str = "backup.error.inline";
pub const ERROR_LINK: &str = "backup.error.link";
pub const PROGRESS_ARCHIVE: &str = "backup.progress.archive";
pub const PROGRESS_LINK: &str = "backup.progress.link";
const CONTENT_TYPE: &str = "application/zstd";
const STEPS: u64 = 2;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub(super) struct Nothing {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub(super) struct Created {
    pub(super) archive: String,
    pub(super) file: String,
    pub(super) bytes: u64,
    pub(super) keel: String,
    pub(super) products: Vec<String>,
    pub(super) tables: u32,
    pub(super) sources: Vec<String>,
    pub(super) config: u32,
}

pub(super) fn create() -> FunctionDef {
    function::<Nothing, Created>(CREATE)
        .kind(Kind::Action)
        .level(Level::Admin)
        .unowned()
        .class(CLASS)
        .ui(ActionScreen::new("backup.create.title").result(ScreenResult::Task))
        .handler(|call: Call<Nothing>| async move {
            let task = call
                .task()
                .ok_or_else(|| KeelError::internal(ERROR_INLINE))?;
            let backups = call.require::<dyn Backups>()?;
            let now = call.require::<dyn Clock>()?.now();
            let platform = call.require::<keel_sdk::types::Platform>()?;
            let file = format!(
                "keel-{}-{}{ARCHIVE_SUFFIX}",
                platform.version,
                now.format("%Y%m%dT%H%M%SZ")
            );
            let archive = backups.directory().join(&file);
            task.progress(0, Some(STEPS), Some(PROGRESS_ARCHIVE))
                .await?;
            task.log(&format!(
                "writing {} requested by {}",
                archive.display(),
                call.principal().login
            ))
            .await?;
            let created = backups.create(&archive).await?;
            let manifest = &created.manifest;
            task.log(&format!(
                "dumped {} table(s) of {}, {} configuration file(s), {} source(s); {} bytes",
                manifest.tables.len(),
                manifest.schema,
                manifest.config.len(),
                manifest.sources.len(),
                created.bytes
            ))
            .await?;
            task.progress(1, Some(STEPS), Some(PROGRESS_LINK)).await?;
            link_output(&created.archive, task.output()?.path())?;
            task.stored(DownloadFile::new(file.clone(), CONTENT_TYPE));
            task.log("the archive is ready to download").await?;
            task.progress(STEPS, Some(STEPS), Some(PROGRESS_LINK))
                .await?;
            Ok(Created {
                archive: created.archive.display().to_string(),
                file,
                bytes: created.bytes,
                keel: manifest.keel.clone(),
                products: manifest
                    .products
                    .iter()
                    .map(|product| product.id.clone())
                    .collect(),
                tables: u32::try_from(manifest.tables.len()).unwrap_or(u32::MAX),
                sources: manifest.sources.clone(),
                config: u32::try_from(manifest.config.len()).unwrap_or(u32::MAX),
            })
        })
}

fn link_output(archive: &Path, output: &Path) -> Result<()> {
    let failed = |error: std::io::Error| {
        KeelError::internal(ERROR_LINK)
            .param("archive", archive.display().to_string())
            .param("output", output.display().to_string())
            .source(error)
    };
    match std::fs::remove_file(output) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(failed(error)),
    }
    if std::fs::hard_link(archive, output).is_ok() {
        return Ok(());
    }
    std::fs::copy(archive, output).map(|_| ()).map_err(failed)
}

#[cfg(test)]
mod tests {
    use keel_sdk::serde_json;

    use super::*;

    #[test]
    fn create_is_an_admin_task_of_the_backup_class() {
        let def = create();
        assert_eq!(def.name(), CREATE);
        assert_eq!(def.kind(), Kind::Action);
        assert_eq!(def.mode(), Mode::Write);
        assert_eq!(def.access().level, Level::Admin);
        assert_eq!(def.class(), CLASS);
        assert!(!def.is_resumable());
        let descriptor = serde_json::to_value(def.descriptor().expect("compiles")).unwrap();
        assert_eq!(
            descriptor["ui"],
            json!({ "title": "backup.create.title", "result": "task" })
        );
        assert_eq!(
            descriptor["result"]["required"],
            json!([
                "archive", "file", "bytes", "keel", "products", "tables", "sources", "config"
            ])
        );
    }

    #[test]
    fn the_output_link_replaces_the_task_file_and_falls_back_to_a_copy() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.tar.zst");
        std::fs::write(&archive, "archive").unwrap();
        let output = dir.path().join("tasks").join("x.out");
        std::fs::create_dir_all(output.parent().unwrap()).unwrap();
        std::fs::write(&output, "").unwrap();
        link_output(&archive, &output).unwrap();
        assert_eq!(std::fs::read_to_string(&output).unwrap(), "archive");
        let error = link_output(&dir.path().join("missing"), &output).unwrap_err();
        assert_eq!(error.key(), ERROR_LINK);
    }
}
