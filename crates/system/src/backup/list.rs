use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::system::{BackupManifest, Backups};
use keel_sdk::types::{Filter, FilterOp, FilterValue};

use super::{CREATE, LIST, MENU_ICON, RESTORE};

pub const ARCHIVE_SUFFIX: &str = ".tar.zst";
pub const ERROR_DIRECTORY: &str = "backup.error.directory";
const SORT_AT: &str = "at";
const SORT_FILE: &str = "file";
const SORT_BYTES: &str = "bytes";

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub struct ArchiveRow {
    pub file: String,
    pub bytes: u64,
    pub at: DateTime<Utc>,
    pub keel: String,
    pub products: String,
    pub tables: u32,
    pub sources: u32,
    pub secret_key: bool,
}

impl ArchiveRow {
    fn of(file: String, bytes: u64, manifest: &BackupManifest) -> Self {
        Self {
            file,
            bytes,
            at: manifest.at,
            keel: manifest.keel.clone(),
            products: manifest
                .products
                .iter()
                .map(|product| product.id.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            tables: u32::try_from(manifest.tables.len()).unwrap_or(u32::MAX),
            sources: u32::try_from(manifest.sources.len()).unwrap_or(u32::MAX),
            secret_key: manifest.secret_key,
        }
    }

    fn value(&self, key: &str) -> String {
        match key {
            "file" => self.file.clone(),
            "bytes" => self.bytes.to_string(),
            "at" => self.at.to_rfc3339(),
            "keel" => self.keel.clone(),
            "products" => self.products.clone(),
            "tables" => self.tables.to_string(),
            "sources" => self.sources.to_string(),
            "secretKey" => self.secret_key.to_string(),
            _ => String::new(),
        }
    }

    fn matches(&self, filter: &Filter) -> bool {
        let actual = self.value(&filter.key);
        let compare = |expected: &str| match filter.key.as_str() {
            "bytes" | "tables" | "sources" => {
                let left = actual.parse::<u64>().unwrap_or_default();
                let right = expected.parse::<u64>().unwrap_or_default();
                left.cmp(&right)
            }
            _ => actual.as_str().cmp(expected),
        };
        match &filter.value {
            FilterValue::Many(expected) => match filter.op {
                FilterOp::Ne => !expected
                    .iter()
                    .any(|value| compare(value) == Ordering::Equal),
                _ => expected
                    .iter()
                    .any(|value| compare(value) == Ordering::Equal),
            },
            FilterValue::One(expected) => {
                let ordering = compare(expected);
                match filter.op {
                    FilterOp::Eq => ordering == Ordering::Equal,
                    FilterOp::Ne => ordering != Ordering::Equal,
                    FilterOp::Gt => ordering == Ordering::Greater,
                    FilterOp::Gte => ordering != Ordering::Less,
                    FilterOp::Lt => ordering == Ordering::Less,
                    FilterOp::Lte => ordering != Ordering::Greater,
                }
            }
        }
    }
}

pub fn is_archive_name(name: &str) -> bool {
    name.ends_with(ARCHIVE_SUFFIX)
        && name.len() > ARCHIVE_SUFFIX.len()
        && !name.starts_with('.')
        && !name.contains(['/', '\\'])
}

pub fn archive_path(directory: &Path, name: &str) -> Option<PathBuf> {
    is_archive_name(name).then(|| directory.join(name))
}

async fn rows(backups: &dyn Backups) -> Result<Vec<ArchiveRow>> {
    let directory = backups.directory();
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(KeelError::internal(ERROR_DIRECTORY)
                .param("path", directory.display().to_string())
                .source(error));
        }
    };
    let mut rows = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            KeelError::internal(ERROR_DIRECTORY)
                .param("path", directory.display().to_string())
                .source(error)
        })?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !is_archive_name(&name) || !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let bytes = entry.metadata().map(|meta| meta.len()).unwrap_or_default();
        match backups.inspect(&entry.path()).await {
            Ok(manifest) => rows.push(ArchiveRow::of(name, bytes, &manifest)),
            Err(error) => {
                tracing::warn!(file = %name, error = %error, "an archive under backups/ is not readable and is not listed");
            }
        }
    }
    Ok(rows)
}

struct ArchiveSource;

#[async_trait]
impl ListSource for ArchiveSource {
    type Row = ArchiveRow;

    async fn fetch(&self, call: &Call<ListQuery>, _: &ListSpec) -> Result<Page<ArchiveRow>> {
        let backups = call.require::<dyn Backups>()?;
        let query = call.params();
        let search = query.search.trim().to_lowercase();
        let mut rows: Vec<ArchiveRow> = rows(backups.as_ref())
            .await?
            .into_iter()
            .filter(|row| search.is_empty() || row.file.to_lowercase().contains(&search))
            .filter(|row| query.filters.iter().all(|filter| row.matches(filter)))
            .collect();
        let (key, dir) = query
            .sort
            .as_ref()
            .map_or((SORT_AT, SortDir::Desc), |sort| {
                (sort.key.as_str(), sort.dir)
            });
        rows.sort_by(|left, right| {
            let ordering = match key {
                SORT_BYTES => left.bytes.cmp(&right.bytes),
                SORT_FILE => left.file.cmp(&right.file),
                _ => left.at.cmp(&right.at),
            }
            .then_with(|| left.file.cmp(&right.file));
            match dir {
                SortDir::Asc => ordering,
                SortDir::Desc => ordering.reverse(),
            }
        });
        let total = rows.len() as u64;
        let size = query.page_size as usize;
        let start = (query.page as usize - 1).saturating_mul(size);
        let data = rows.into_iter().skip(start).take(size).collect();
        Ok(Page::new(data, total))
    }
}

pub(super) fn list() -> FunctionDef {
    function::<ListQuery, Page<ArchiveRow>>(LIST)
        .kind(Kind::List)
        .level(Level::Admin)
        .unowned()
        .key(SORT_FILE)
        .ui(List::new("backup.list.title")
            .search([SORT_FILE])
            .column(
                Column::new(SORT_FILE, ColumnType::Text)
                    .sortable()
                    .primary(),
            )
            .column(Column::new(SORT_AT, ColumnType::Datetime).sortable())
            .column(Column::new(SORT_BYTES, ColumnType::Number).sortable())
            .column(Column::new("keel", ColumnType::Text))
            .column(Column::new("products", ColumnType::Text))
            .column(Column::new("tables", ColumnType::Number))
            .column(Column::new("sources", ColumnType::Number))
            .column(Column::new("secretKey", ColumnType::Boolean))
            .action(
                Action::new("create", ActionRole::New)
                    .screen(CREATE)
                    .icon(MENU_ICON),
            )
            .action(
                Action::new("restore", ActionRole::Button)
                    .screen(RESTORE)
                    .params([("archive", "$id")])
                    .icon("history"),
            )
            .action(Action::new("refresh", ActionRole::Refresh))
            .row_actions(["restore"])
            .empty("backup.empty", Some("create"))
            .invalidate_on(["backup.*"]))
        .default_sort(SORT_AT, SortDir::Desc)
        .source(ArchiveSource)
}

#[cfg(test)]
mod tests {
    use keel_sdk::serde_json;

    use super::*;

    fn manifest(at: &str, products: &[&str]) -> BackupManifest {
        BackupManifest {
            keel: "0.1.0".into(),
            at: at.parse().unwrap(),
            instance: None,
            schema: "keel".into(),
            products: products
                .iter()
                .map(|id| keel_sdk::system::BackupProduct {
                    id: (*id).to_owned(),
                    version: "0.1.0".into(),
                    edition: None,
                })
                .collect(),
            modules: Vec::new(),
            tables: Vec::new(),
            sources: vec!["dns.zonefiles".into()],
            config: Vec::new(),
            data: Vec::new(),
            secret_key: true,
        }
    }

    #[test]
    fn archive_names_end_with_the_suffix_and_stay_inside_the_directory() {
        assert!(is_archive_name("keel-0.1.0-20260921T101530Z.tar.zst"));
        assert!(!is_archive_name(".tar.zst"));
        assert!(!is_archive_name(".hidden.tar.zst"));
        assert!(!is_archive_name("../other.tar.zst"));
        assert!(!is_archive_name("notes.txt"));
        assert_eq!(
            archive_path(Path::new("/var/lib/keel/backups"), "a.tar.zst"),
            Some(PathBuf::from("/var/lib/keel/backups/a.tar.zst"))
        );
        assert_eq!(
            archive_path(Path::new("/var/lib/keel/backups"), "a/b.tar.zst"),
            None
        );
    }

    #[test]
    fn a_row_is_built_from_the_manifest_and_answers_filters_by_column() {
        let row = ArchiveRow::of(
            "keel-0.1.0-a.tar.zst".into(),
            2048,
            &manifest("2026-09-21T10:15:30Z", &["demo", "dns"]),
        );
        assert_eq!(row.products, "demo, dns");
        assert_eq!(row.sources, 1);
        assert!(row.secret_key);
        let json = serde_json::to_value(&row).unwrap();
        assert_eq!(json["secretKey"], json!(true));
        assert_eq!(json["bytes"], json!(2048));
        let filter = |key: &str, op: FilterOp, value: FilterValue| Filter {
            key: key.to_owned(),
            op,
            value,
        };
        assert!(row.matches(&filter(
            "keel",
            FilterOp::Eq,
            FilterValue::One("0.1.0".into())
        )));
        assert!(!row.matches(&filter(
            "keel",
            FilterOp::Ne,
            FilterValue::One("0.1.0".into())
        )));
        assert!(row.matches(&filter(
            "bytes",
            FilterOp::Gt,
            FilterValue::One("1000".into())
        )));
        assert!(row.matches(&filter(
            "bytes",
            FilterOp::Lte,
            FilterValue::One("2048".into())
        )));
        assert!(!row.matches(&filter(
            "bytes",
            FilterOp::Lt,
            FilterValue::One("999".into())
        )));
        assert!(row.matches(&filter(
            "tables",
            FilterOp::Gte,
            FilterValue::One("0".into())
        )));
        assert!(row.matches(&filter(
            "secretKey",
            FilterOp::Eq,
            FilterValue::Many(vec!["true".into(), "maybe".into()])
        )));
        assert!(row.matches(&filter(
            "products",
            FilterOp::Ne,
            FilterValue::Many(vec!["demo".into()])
        )));
        assert_eq!(row.value("unknown"), "");
        assert_eq!(row.value("at"), "2026-09-21T10:15:30+00:00");
    }

    #[test]
    fn the_list_is_an_admin_screen_with_the_create_and_restore_actions() {
        let def = list();
        assert_eq!(def.name(), LIST);
        assert_eq!(def.kind(), Kind::List);
        assert_eq!(def.access().level, Level::Admin);
        assert_eq!(def.key(), Some("file"));
        let descriptor = serde_json::to_value(def.descriptor().expect("compiles")).unwrap();
        let actions: Vec<&str> = descriptor["ui"]["actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|action| action["id"].as_str().unwrap())
            .collect();
        assert_eq!(actions, ["create", "restore", "refresh"]);
        assert_eq!(descriptor["ui"]["row_actions"], json!(["restore"]));
        assert_eq!(
            descriptor["ui"]["actions"][1]["params"]["archive"],
            json!("$id")
        );
        assert_eq!(descriptor["ui"]["empty"]["title"], json!("backup.empty"));
        let spec = def.list_spec().expect("a list spec");
        assert_eq!(spec.sortable, ["file", "at", "bytes"]);
        assert_eq!(
            spec.default_sort.as_ref().map(|sort| sort.key.as_str()),
            Some("at")
        );
    }
}
