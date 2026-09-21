use keel_sdk::prelude::*;
use keel_sdk::system::Backups;
use keel_sdk::types::ScreenResult;

use super::RESTORE;
use super::create::{CLASS, ERROR_INLINE};
use super::list::archive_path;

pub const ERROR_NAME: &str = "backup.error.name";
pub const ERROR_NOT_FOUND: &str = "backup.error.not_found";
pub const PROGRESS_CHECK: &str = "backup.progress.check";
pub const PROGRESS_REQUEST: &str = "backup.progress.request";
const NAME_PATTERN: &str = r"^[A-Za-z0-9][A-Za-z0-9._-]*\.tar\.zst$";
const STEPS: u64 = 2;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub(super) struct Pick {
    #[schemars(length(min = 1, max = 255), regex(pattern = NAME_PATTERN))]
    pub archive: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub(super) struct Requested {
    pub(super) archive: String,
    pub(super) keel: String,
    pub(super) products: Vec<String>,
    pub(super) tables: u32,
    pub(super) secret_key: bool,
    pub(super) request: String,
    pub(super) requested_by: String,
}

pub(super) fn restore() -> FunctionDef {
    function::<Pick, Requested>(RESTORE)
        .kind(Kind::Form)
        .level(Level::Admin)
        .unowned()
        .class(CLASS)
        .ui(Form::new("backup.restore.title")
            .page("main", ["archive"])
            .result(ScreenResult::Task))
        .handler(|call: Call<Pick>| async move {
            let task = call
                .task()
                .ok_or_else(|| KeelError::internal(ERROR_INLINE))?;
            let backups = call.require::<dyn Backups>()?;
            let name = call.params().archive.trim().to_owned();
            let path = archive_path(&backups.directory(), &name).ok_or_else(|| {
                KeelError::validation(ERROR_NAME)
                    .field("archive")
                    .param("archive", name.clone())
            })?;
            if !path.is_file() {
                return Err(KeelError::not_found(ERROR_NOT_FOUND)
                    .field("archive")
                    .param("archive", name.clone()));
            }
            task.progress(0, Some(STEPS), Some(PROGRESS_CHECK)).await?;
            let manifest = backups.inspect(&path).await?;
            backups.admits(&manifest)?;
            task.log(&format!(
                "{name} was taken by keel {} at {} with {} table(s) and the products {}",
                manifest.keel,
                manifest.at.to_rfc3339(),
                manifest.tables.len(),
                manifest
                    .products
                    .iter()
                    .map(|product| product.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
            .await?;
            task.progress(1, Some(STEPS), Some(PROGRESS_REQUEST))
                .await?;
            let requested_by = call.principal().login.clone();
            let request = backups.request_restore(&path, &requested_by).await?;
            task.log(&format!(
                "the restore request is written to {}; the service restores the archive before it opens its port at the next start, which the operator triggers with a restart",
                request.display()
            ))
            .await?;
            task.progress(STEPS, Some(STEPS), Some(PROGRESS_REQUEST))
                .await?;
            Ok(Requested {
                archive: path.display().to_string(),
                keel: manifest.keel.clone(),
                products: manifest
                    .products
                    .iter()
                    .map(|product| product.id.clone())
                    .collect(),
                tables: u32::try_from(manifest.tables.len()).unwrap_or(u32::MAX),
                secret_key: manifest.secret_key,
                request: request.display().to_string(),
                requested_by,
            })
        })
}

#[cfg(test)]
mod tests {
    use keel_sdk::serde_json;

    use super::*;

    #[test]
    fn restore_is_an_admin_form_that_runs_as_a_task() {
        let def = restore();
        assert_eq!(def.name(), RESTORE);
        assert_eq!(def.kind(), Kind::Form);
        assert_eq!(def.mode(), Mode::Write);
        assert_eq!(def.access().level, Level::Admin);
        assert_eq!(def.class(), CLASS);
        let descriptor = serde_json::to_value(def.descriptor().expect("compiles")).unwrap();
        assert_eq!(descriptor["ui"]["result"], json!("task"));
        assert_eq!(descriptor["ui"]["title"], json!("backup.restore.title"));
        assert_eq!(descriptor["params"]["required"], json!(["archive"]));
        assert_eq!(
            descriptor["params"]["properties"]["archive"]["maxLength"],
            json!(255)
        );
        assert_eq!(
            descriptor["params"]["properties"]["archive"]["pattern"],
            json!(NAME_PATTERN)
        );
    }
}
