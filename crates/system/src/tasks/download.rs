use keel_sdk::prelude::*;
use keel_sdk::sea_orm::EntityTrait;
use keel_sdk::tokio::fs;

use super::api::Drain;
use super::entity::job;
use super::functions::{PERMISSION_READ, Pick};
use super::log;
use super::runner::ERROR_NOT_FOUND;

pub const FUNCTION: &str = keel_sdk::types::DOWNLOAD_FUNCTION;
pub const ERROR_NO_FILE: &str = "task.error.no_file";

pub fn download() -> FunctionDef {
    function::<Pick, DownloadFile>(FUNCTION)
        .access(Level::User, PERMISSION_READ)
        .owned::<job::Entity>()
        .download(|call: Call<Pick>| async move {
            let id = call.params().id;
            let row = job::Entity::find_by_id(id.to_string())
                .one(call.db())
                .await?
                .ok_or_else(|| KeelError::not_found(ERROR_NOT_FOUND).param("id", id.to_string()))?;
            let no_file = || KeelError::not_found(ERROR_NO_FILE).param("id", id.to_string());
            let (Some(filename), Some(content_type)) = (row.file_name, row.file_type) else {
                return Err(no_file());
            };
            let data_dir = call.require::<Drain>()?.data_dir()?;
            let path = log::output_path(&data_dir, id);
            if fs::metadata(&path).await.is_err() {
                return Err(no_file());
            }
            Ok(Download::file(filename, content_type, path).into())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_download_is_an_owned_download_of_the_job_table_behind_the_read_permission() {
        let def = download();
        assert_eq!(def.name(), "task.download");
        assert_eq!(def.kind(), Kind::Download);
        assert_eq!(def.mode(), Mode::Read);
        assert_eq!(def.key(), Some("id"));
        assert_eq!(
            def.owned().map(|owned| owned.table.as_str()),
            Some("task_job")
        );
        assert_eq!(def.access().level, Level::User);
        assert_eq!(def.access().permission.as_deref(), Some(PERMISSION_READ));
        let descriptor =
            keel_sdk::serde_json::to_value(def.descriptor().expect("compiles")).expect("json");
        assert_eq!(descriptor["kind"], json!("download"));
        assert_eq!(
            descriptor["result"]["required"],
            json!(["filename", "contentType"])
        );
        assert!(descriptor.get("ui").is_none());
    }
}
