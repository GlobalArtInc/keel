use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use keel_sdk::ConfigSection;
use keel_sdk::runtime::Contributions;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, Value as DbValue};
use serde::Serialize;
use serde_json::{Map, Value, json};
use tar::{Builder, EntryType, Header};

use crate::config::redact_url;
use crate::dispatcher::{MASK, Plan};
use crate::doctor::{statement, table_exists};

const ARCHIVE_PREFIX: &str = "keel-support-";
const EXTENSION: &str = "tar.zst";
pub const CONFIG_FILE: &str = "config.yaml";
pub const POLICY_FILE: &str = "agent.yaml";
pub const LOGS_FILE: &str = "logs.jsonl";
pub const LEDGERS_FILE: &str = "ledgers.json";
pub const AUDIT_FILE: &str = "audit.jsonl";
pub const PROBLEMS_FILE: &str = "problems.json";
pub const READINESS_FILE: &str = "readiness.json";
pub const DOCTOR_FILE: &str = "doctor.txt";
pub const MANIFEST_FILE: &str = "manifest.json";
const AUDIT_TAIL: u64 = 1000;

const AUDIT_TABLE: &str = "audit_call";
const PROBLEM_TABLE: &str = "notify_problem";
const AUDIT_QUERY: &str = "SELECT row_to_json(keel_row)::text FROM (SELECT * FROM audit_call ORDER BY at DESC LIMIT $1) keel_row";
const PROBLEM_QUERY: &str = "SELECT row_to_json(keel_row)::text FROM (SELECT * FROM notify_problem WHERE cleared_at IS NULL ORDER BY raised_at) keel_row";
const KERNEL_KEYS: [&str; 7] = [
    "listen",
    "socket",
    "data_dir",
    "database",
    "tls",
    "trusted_proxies",
    "log",
];
const DATABASE_KEY: &str = "database";
const SUSPICIOUS: [&str; 4] = ["password", "secret", "token", "passphrase"];
const ARCHIVE_MODE: u32 = 0o600;
const ENTRY_MODE: u32 = 0o600;
const LEVEL: i32 = 3;
const TEMPORARY_SUFFIX: &str = ".partial";

#[derive(Debug, thiserror::Error)]
pub enum SupportError {
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path} is not YAML: {source}")]
    Yaml {
        path: PathBuf,
        #[source]
        source: serde_yaml_ng::Error,
    },
    #[error("could not write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path} exists already; a support bundle never overwrites a file")]
    Exists { path: PathBuf },
    #[error("the database refused {step}: {source}")]
    Database {
        step: &'static str,
        #[source]
        source: DbErr,
    },
    #[error("{what} could not be encoded: {source}")]
    Encode {
        what: String,
        #[source]
        source: serde_json::Error,
    },
}

pub struct Masks {
    plans: Vec<(String, Plan)>,
}

impl Masks {
    pub fn of(contributions: &Contributions) -> Self {
        Self {
            plans: contributions
                .of::<ConfigSection>()
                .map(|section| {
                    (
                        section.key().to_owned(),
                        Plan::of_schema(section.schema().as_value()),
                    )
                })
                .collect(),
        }
    }

    pub fn none() -> Self {
        Self { plans: Vec::new() }
    }

    fn plan(&self, key: &str) -> Option<&Plan> {
        self.plans
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, plan)| plan)
    }
}

fn is_suspicious(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    SUSPICIOUS.iter().any(|needle| lower.contains(needle))
}

fn mask_suspicious(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, nested) in object.iter_mut() {
                if is_suspicious(key) {
                    *nested = Value::String(MASK.to_owned());
                } else {
                    mask_suspicious(nested);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(mask_suspicious),
        _ => {}
    }
}

fn redact_config(text: &str, masks: &Masks) -> Result<String, serde_yaml_ng::Error> {
    let mut value: Value = match serde_yaml_ng::from_str::<serde_yaml_ng::Value>(text)? {
        serde_yaml_ng::Value::Null => Value::Object(Map::new()),
        yaml => serde_yaml_ng::from_value(yaml)?,
    };
    if let Value::Object(object) = &mut value {
        if let Some(Value::String(url)) = object.get_mut(DATABASE_KEY) {
            *url = redact_url(url);
        }
        for (key, section) in object.iter_mut() {
            if KERNEL_KEYS.contains(&key.as_str()) {
                continue;
            }
            match masks.plan(key) {
                Some(plan) => plan.mask(section),
                None => mask_suspicious(section),
            }
        }
    }
    serde_yaml_ng::to_string(&value)
}

pub fn redacted_file(path: &Path, masks: &Masks) -> Result<Option<String>, SupportError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(SupportError::Read {
                path: path.to_owned(),
                source,
            });
        }
    };
    redact_config(&text, masks)
        .map(Some)
        .map_err(|source| SupportError::Yaml {
            path: path.to_owned(),
            source,
        })
}

pub fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, SupportError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(SupportError::Read {
            path: path.to_owned(),
            source,
        }),
    }
}

async fn json_rows(
    db: &DatabaseConnection,
    table: &str,
    sql: &str,
    values: Vec<DbValue>,
    step: &'static str,
) -> Result<Option<Vec<Value>>, SupportError> {
    let failed = |source| SupportError::Database { step, source };
    if !table_exists(db, table).await.map_err(failed)? {
        return Ok(None);
    }
    let rows = db.query_all(statement(sql, values)).await.map_err(failed)?;
    let mut parsed = Vec::with_capacity(rows.len());
    for row in rows {
        let text: String = row.try_get_by_index(0).map_err(failed)?;
        let value = serde_json::from_str(&text).map_err(|source| SupportError::Encode {
            what: format!("a row of {table}"),
            source,
        })?;
        parsed.push(value);
    }
    Ok(Some(parsed))
}

pub async fn audit_tail(db: &DatabaseConnection) -> Result<Option<Vec<Value>>, SupportError> {
    json_rows(
        db,
        AUDIT_TABLE,
        AUDIT_QUERY,
        vec![DbValue::from(i64::try_from(AUDIT_TAIL).unwrap_or(i64::MAX))],
        "the audit tail",
    )
    .await
}

pub async fn open_problems(db: &DatabaseConnection) -> Result<Option<Vec<Value>>, SupportError> {
    json_rows(
        db,
        PROBLEM_TABLE,
        PROBLEM_QUERY,
        Vec::new(),
        "the open problems",
    )
    .await
}

pub fn archive_name(version: &str, at: DateTime<Utc>) -> String {
    format!(
        "{ARCHIVE_PREFIX}{version}-{}.{EXTENSION}",
        at.format("%Y%m%dT%H%M%SZ")
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Written {
    pub archive: PathBuf,
    pub bytes: u64,
    pub entries: Vec<String>,
}

pub struct Bundle {
    version: String,
    at: DateTime<Utc>,
    instance: Option<String>,
    entries: Vec<(String, Vec<u8>)>,
    notes: Map<String, Value>,
}

impl Bundle {
    pub fn new(version: &str, at: DateTime<Utc>, instance: Option<String>) -> Self {
        Self {
            version: version.to_owned(),
            at,
            instance,
            entries: Vec::new(),
            notes: Map::new(),
        }
    }

    pub fn add(&mut self, name: &str, bytes: Vec<u8>) {
        match self
            .entries
            .iter_mut()
            .find(|(existing, _)| existing == name)
        {
            Some((_, existing)) => *existing = bytes,
            None => self.entries.push((name.to_owned(), bytes)),
        }
    }

    pub fn add_text(&mut self, name: &str, text: &str) {
        self.add(name, text.as_bytes().to_vec());
    }

    pub fn add_json<T: Serialize>(&mut self, name: &str, value: &T) -> Result<(), SupportError> {
        let text = serde_json::to_vec_pretty(value).map_err(|source| SupportError::Encode {
            what: name.to_owned(),
            source,
        })?;
        self.add(name, text);
        Ok(())
    }

    pub fn add_lines(&mut self, name: &str, rows: &[Value]) {
        let mut text = Vec::new();
        for row in rows {
            text.extend_from_slice(row.to_string().as_bytes());
            text.push(b'\n');
        }
        self.add(name, text);
    }

    pub fn note(&mut self, key: &str, text: impl Into<String>) {
        self.notes
            .insert(key.to_owned(), Value::String(text.into()));
    }

    fn names(&self) -> Vec<String> {
        self.entries.iter().map(|(name, _)| name.clone()).collect()
    }

    pub fn write(mut self, out: &Path) -> Result<Written, SupportError> {
        let manifest = json!({
            "keel": self.version,
            "at": self.at,
            "instance": self.instance,
            "entries": self.names(),
            "notes": self.notes,
        });
        self.add_json(MANIFEST_FILE, &manifest)?;
        if fs::symlink_metadata(out).is_ok() {
            return Err(SupportError::Exists {
                path: out.to_owned(),
            });
        }
        if let Some(parent) = out.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|source| SupportError::Write {
                path: parent.to_owned(),
                source,
            })?;
        }
        let temporary = {
            let mut name = out.as_os_str().to_owned();
            name.push(TEMPORARY_SUFFIX);
            PathBuf::from(name)
        };
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(ARCHIVE_MODE)
            .open(&temporary)
            .map_err(|source| match source.kind() {
                io::ErrorKind::AlreadyExists => SupportError::Exists {
                    path: temporary.clone(),
                },
                _ => SupportError::Write {
                    path: temporary.clone(),
                    source,
                },
            })?;
        let packed = self.pack(file).and_then(|file| {
            file.sync_all()?;
            file.metadata().map(|meta| meta.len())
        });
        let bytes = match packed {
            Ok(bytes) => bytes,
            Err(source) => {
                let _ = fs::remove_file(&temporary);
                return Err(SupportError::Write {
                    path: out.to_owned(),
                    source,
                });
            }
        };
        fs::rename(&temporary, out).map_err(|source| SupportError::Write {
            path: out.to_owned(),
            source,
        })?;
        Ok(Written {
            archive: out.to_owned(),
            bytes,
            entries: self.names(),
        })
    }

    fn pack(&self, file: File) -> io::Result<File> {
        let encoder = zstd::Encoder::new(file, LEVEL)?;
        let mut builder = Builder::new(encoder);
        let mtime = u64::try_from(self.at.timestamp()).unwrap_or(0);
        for (name, bytes) in &self.entries {
            let mut header = Header::new_ustar();
            header.set_entry_type(EntryType::Regular);
            header.set_mode(ENTRY_MODE);
            header.set_size(bytes.len() as u64);
            header.set_mtime(mtime);
            header.set_uid(0);
            header.set_gid(0);
            builder.append_data(&mut header, name, bytes.as_slice())?;
        }
        let encoder = builder.into_inner()?;
        encoder.finish()
    }
}

pub fn read_entries(archive: &Path) -> Result<Vec<(String, Vec<u8>)>, SupportError> {
    use std::io::Read;

    let file = File::open(archive).map_err(|source| SupportError::Read {
        path: archive.to_owned(),
        source,
    })?;
    let failed = |source| SupportError::Read {
        path: archive.to_owned(),
        source,
    };
    let decoder = zstd::Decoder::new(file).map_err(failed)?;
    let mut tar = tar::Archive::new(decoder);
    let mut entries = Vec::new();
    for entry in tar.entries().map_err(failed)? {
        let mut entry = entry.map_err(failed)?;
        let name = entry.path().map_err(failed)?.to_string_lossy().into_owned();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(failed)?;
        entries.push((name, bytes));
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use keel_sdk::prelude::*;
    use keel_sdk::registrar::Contributions;

    use super::*;

    #[derive(Default, Deserialize, JsonSchema)]
    struct Relay {
        host: String,
        #[schemars(extend("x-keel" = json!({ "secret": true })))]
        api_key: String,
    }

    fn masks() -> Masks {
        let mut contributions = Contributions::default();
        contributions.add(ConfigSection::new::<Relay>("relay"));
        Masks::of(&contributions)
    }

    #[test]
    fn the_configuration_is_redacted_by_the_database_url_the_section_schema_and_the_key_names() {
        let text = "listen: 0.0.0.0:8443\ndatabase: postgres://keel:hunter2@db.example.net/keel\ntls: { cert: /etc/keel/cert.pem, key: /etc/keel/key.pem }\nrelay:\n  host: relay.example\n  api_key: sk-live-42\nfeature:\n  enabled: true\n  password: nope\n  nested: { token: t0k, name: fine }\n";
        let redacted = redact_config(text, &masks()).unwrap();
        assert!(!redacted.contains("hunter2"), "{redacted}");
        assert!(!redacted.contains("sk-live-42"), "{redacted}");
        assert!(!redacted.contains("nope"), "{redacted}");
        assert!(!redacted.contains("t0k"), "{redacted}");
        assert!(
            redacted.contains("postgres://keel:***@db.example.net/keel"),
            "{redacted}"
        );
        assert!(redacted.contains("key: /etc/keel/key.pem"), "{redacted}");
        assert!(redacted.contains("host: relay.example"), "{redacted}");
        assert!(redacted.contains("name: fine"), "{redacted}");
        assert!(redacted.contains("api_key: '***'"), "{redacted}");
        assert_eq!(masks().plans.len(), 1);
        let relay: Relay = serde_json::from_value(json!({ "host": "h", "api_key": "k" })).unwrap();
        assert_eq!((relay.host.as_str(), relay.api_key.as_str()), ("h", "k"));
        assert_eq!(redact_config("", &Masks::none()).unwrap(), "{}\n");
        assert!(redact_config("listen: [", &Masks::none()).is_err());
        assert!(is_suspicious("SMTP_PASSWORD") && !is_suspicious("host"));
    }

    #[test]
    fn a_bundle_is_written_as_a_tar_zst_with_a_manifest_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let at: DateTime<Utc> = "2026-09-21T10:15:30Z".parse().unwrap();
        let mut bundle = Bundle::new("0.1.0", at, Some("inst-1".into()));
        bundle.add_text(CONFIG_FILE, "listen: 127.0.0.1:1500\n");
        bundle.add_lines(LOGS_FILE, &[json!({ "seq": 1 }), json!({ "seq": 2 })]);
        bundle.add_json(PROBLEMS_FILE, &json!([])).unwrap();
        bundle.add_text(CONFIG_FILE, "listen: 127.0.0.1:1501\n");
        bundle.note("logs", "the service answered");
        assert_eq!(bundle.names(), [CONFIG_FILE, LOGS_FILE, PROBLEMS_FILE]);
        assert_eq!(bundle.entries[0].1, b"listen: 127.0.0.1:1501\n");
        let out = dir.path().join(archive_name("0.1.0", at));
        assert_eq!(
            out.file_name().unwrap().to_str().unwrap(),
            "keel-support-0.1.0-20260921T101530Z.tar.zst"
        );
        let written = bundle.write(&out).unwrap();
        assert_eq!(written.archive, out);
        assert!(written.bytes > 0);
        assert_eq!(
            written.entries,
            [CONFIG_FILE, LOGS_FILE, PROBLEMS_FILE, MANIFEST_FILE]
        );
        let entries = read_entries(&out).unwrap();
        let names: Vec<&str> = entries.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            [CONFIG_FILE, LOGS_FILE, PROBLEMS_FILE, MANIFEST_FILE]
        );
        let logs = &entries[1].1;
        assert_eq!(String::from_utf8_lossy(logs), "{\"seq\":1}\n{\"seq\":2}\n");
        let manifest: Value = serde_json::from_slice(&entries[3].1).unwrap();
        assert_eq!(manifest["keel"], "0.1.0");
        assert_eq!(manifest["instance"], "inst-1");
        assert_eq!(manifest["notes"]["logs"], "the service answered");
        assert_eq!(
            manifest["entries"],
            json!([CONFIG_FILE, LOGS_FILE, PROBLEMS_FILE])
        );
        let again = Bundle::new("0.1.0", at, None).write(&out).unwrap_err();
        assert!(matches!(again, SupportError::Exists { .. }), "{again}");
        assert!(
            !dir.path()
                .join(format!("{}{TEMPORARY_SUFFIX}", out.display()))
                .exists()
        );
        let mode = std::os::unix::fs::MetadataExt::mode(&fs::metadata(&out).unwrap()) & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn a_symlink_planted_at_the_temporary_path_is_refused_and_never_followed() {
        let dir = tempfile::tempdir().unwrap();
        let at: DateTime<Utc> = "2026-09-21T10:15:30Z".parse().unwrap();
        let out = dir.path().join(archive_name("0.1.0", at));
        let temporary = PathBuf::from(format!("{}{TEMPORARY_SUFFIX}", out.display()));
        let victim = dir.path().join("victim");
        fs::write(&victim, b"secret material\n").unwrap();
        std::os::unix::fs::symlink(&victim, &temporary).unwrap();
        let mut bundle = Bundle::new("0.1.0", at, None);
        bundle.add_text(CONFIG_FILE, "listen: 127.0.0.1:1500\n");
        let refused = bundle.write(&out).unwrap_err();
        assert!(
            matches!(&refused, SupportError::Exists { path } if *path == temporary),
            "{refused}"
        );
        assert_eq!(fs::read(&victim).unwrap(), b"secret material\n");
        assert!(fs::symlink_metadata(&temporary).unwrap().is_symlink());
        assert!(!out.exists());
        fs::remove_file(&temporary).unwrap();
        fs::write(&temporary, b"stale\n").unwrap();
        let refused = Bundle::new("0.1.0", at, None).write(&out).unwrap_err();
        assert!(
            matches!(&refused, SupportError::Exists { path } if *path == temporary),
            "{refused}"
        );
        assert_eq!(fs::read(&temporary).unwrap(), b"stale\n");
        assert!(!out.exists());
    }

    #[test]
    fn optional_files_are_read_when_present_and_skipped_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_optional(&dir.path().join("agent.yaml")).unwrap(), None);
        assert_eq!(
            redacted_file(&dir.path().join("config.yaml"), &Masks::none()).unwrap(),
            None
        );
        let config = dir.path().join("config.yaml");
        fs::write(&config, "database: postgres://a:b@h/d\n").unwrap();
        let text = redacted_file(&config, &Masks::none()).unwrap().unwrap();
        assert_eq!(text, "database: postgres://a:***@h/d\n");
        fs::write(&config, "database: [").unwrap();
        assert!(matches!(
            redacted_file(&config, &Masks::none()),
            Err(SupportError::Yaml { .. })
        ));
        assert_eq!(read_optional(&config).unwrap().unwrap(), b"database: [");
    }
}
