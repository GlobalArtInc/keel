use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

use keel_sdk::system::BackupManifest;
use tar::{Archive, Builder, EntryType};

use super::BackupError;

pub const EXTENSION: &str = "tar.zst";
pub const MANIFEST: &str = "manifest.json";
pub const DB_DIR: &str = "db";
pub const ETC_DIR: &str = "etc";
pub const DATA_DIR: &str = "data";
pub const SOURCES_DIR: &str = "sources";
const ARCHIVE_MODE: u32 = 0o600;

const TOP_LEVEL: [&str; 4] = [DB_DIR, ETC_DIR, DATA_DIR, SOURCES_DIR];
const MANIFEST_LIMIT: u64 = 1024 * 1024;
const LEVEL: i32 = 3;
const TEMPORARY_SUFFIX: &str = ".partial";

pub fn pack(staging: &Path, out: &Path) -> Result<u64, BackupError> {
    if fs::symlink_metadata(out).is_ok() {
        return Err(BackupError::Exists {
            path: out.to_owned(),
        });
    }
    let temporary = temporary_name(out);
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(ARCHIVE_MODE)
        .open(&temporary)
        .map_err(|source| BackupError::Create {
            path: temporary.clone(),
            source,
        })?;
    let packed = write_entries(staging, file).and_then(|file| {
        file.sync_all()?;
        file.metadata().map(|meta| meta.len())
    });
    let bytes = match packed {
        Ok(bytes) => bytes,
        Err(source) => {
            let _ = fs::remove_file(&temporary);
            return Err(BackupError::Write {
                path: out.to_owned(),
                source,
            });
        }
    };
    fs::rename(&temporary, out).map_err(|source| BackupError::Write {
        path: out.to_owned(),
        source,
    })?;
    Ok(bytes)
}

fn temporary_name(out: &Path) -> PathBuf {
    let mut name = out.as_os_str().to_owned();
    name.push(TEMPORARY_SUFFIX);
    PathBuf::from(name)
}

fn write_entries(staging: &Path, file: File) -> io::Result<File> {
    let encoder = zstd::Encoder::new(file, LEVEL)?;
    let mut builder = Builder::new(encoder);
    builder.follow_symlinks(false);
    builder.append_path_with_name(staging.join(MANIFEST), MANIFEST)?;
    for top in TOP_LEVEL {
        let dir = staging.join(top);
        if dir.is_dir() {
            append_tree(&mut builder, &dir, Path::new(top))?;
        }
    }
    let encoder = builder.into_inner()?;
    encoder.finish()
}

fn append_tree(
    builder: &mut Builder<zstd::Encoder<'static, File>>,
    dir: &Path,
    name: &Path,
) -> io::Result<()> {
    builder.append_dir(name, dir)?;
    let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<io::Result<_>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let kind = entry.file_type()?;
        let child = name.join(entry.file_name());
        if kind.is_dir() {
            append_tree(builder, &path, &child)?;
        } else if kind.is_file() {
            builder.append_path_with_name(&path, &child)?;
        }
    }
    Ok(())
}

fn open(
    archive: &Path,
) -> Result<Archive<zstd::Decoder<'static, io::BufReader<File>>>, BackupError> {
    let file = File::open(archive).map_err(|source| BackupError::Read {
        path: archive.to_owned(),
        source,
    })?;
    let decoder = zstd::Decoder::new(file).map_err(|source| BackupError::Archive {
        path: archive.to_owned(),
        reason: format!("not a zstd stream: {source}"),
    })?;
    Ok(Archive::new(decoder))
}

fn broken(archive: &Path, source: io::Error) -> BackupError {
    BackupError::Archive {
        path: archive.to_owned(),
        reason: source.to_string(),
    }
}

pub fn unpack(archive: &Path, into: &Path) -> Result<(), BackupError> {
    let failed = |source| broken(archive, source);
    let mut tar = open(archive)?;
    let mut manifest_seen = false;
    for entry in tar.entries().map_err(failed)? {
        let mut entry = entry.map_err(failed)?;
        let relative = checked_path(&entry.path().map_err(failed)?)?;
        let kind = entry.header().entry_type();
        let destination = into.join(&relative);
        match kind {
            EntryType::Directory => {
                fs::create_dir_all(&destination).map_err(|source| BackupError::Create {
                    path: destination.clone(),
                    source,
                })?;
            }
            EntryType::Regular => {
                if let Some(parent) = destination.parent() {
                    fs::create_dir_all(parent).map_err(|source| BackupError::Create {
                        path: parent.to_owned(),
                        source,
                    })?;
                }
                if fs::symlink_metadata(&destination).is_ok() {
                    return Err(BackupError::Entry {
                        entry: relative.display().to_string(),
                        reason: "it appears twice in the archive",
                    });
                }
                entry
                    .unpack(&destination)
                    .map_err(|source| BackupError::Write {
                        path: destination.clone(),
                        source,
                    })?;
                if relative == Path::new(MANIFEST) {
                    manifest_seen = true;
                }
            }
            _ => {
                return Err(BackupError::Entry {
                    entry: relative.display().to_string(),
                    reason: "only regular files and directories are unpacked",
                });
            }
        }
    }
    if !manifest_seen {
        return Err(BackupError::Archive {
            path: archive.to_owned(),
            reason: format!("it carries no {MANIFEST}"),
        });
    }
    Ok(())
}

pub fn read_manifest(archive: &Path) -> Result<BackupManifest, BackupError> {
    let failed = |source| broken(archive, source);
    let mut tar = open(archive)?;
    for entry in tar.entries().map_err(failed)? {
        let mut entry = entry.map_err(failed)?;
        let relative = checked_path(&entry.path().map_err(failed)?)?;
        if relative != Path::new(MANIFEST) {
            continue;
        }
        let mut text = String::new();
        entry
            .by_ref()
            .take(MANIFEST_LIMIT)
            .read_to_string(&mut text)
            .map_err(failed)?;
        return serde_json::from_str(&text).map_err(|error| BackupError::Archive {
            path: archive.to_owned(),
            reason: format!("its {MANIFEST} does not parse: {error}"),
        });
    }
    Err(BackupError::Archive {
        path: archive.to_owned(),
        reason: format!("it carries no {MANIFEST}"),
    })
}

fn checked_path(path: &Path) -> Result<PathBuf, BackupError> {
    let entry = path.display().to_string();
    let mut components = path.components();
    let first = match components.next() {
        Some(Component::Normal(first)) => first,
        _ => {
            return Err(BackupError::Entry {
                entry,
                reason: "its path is not relative",
            });
        }
    };
    let top = first.to_str().ok_or(BackupError::Entry {
        entry: entry.clone(),
        reason: "its path is not UTF-8",
    })?;
    if top != MANIFEST && !TOP_LEVEL.contains(&top) {
        return Err(BackupError::Entry {
            entry,
            reason: "its path is outside the archive layout",
        });
    }
    for component in components {
        match component {
            Component::Normal(_) => {}
            _ => {
                return Err(BackupError::Entry {
                    entry,
                    reason: "its path leaves its directory",
                });
            }
        }
    }
    Ok(path.to_owned())
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn staging_with_manifest(dir: &Path) {
        fs::create_dir_all(dir.join(DB_DIR)).unwrap();
        fs::create_dir_all(dir.join(DATA_DIR).join("tls")).unwrap();
        fs::write(
            dir.join(MANIFEST),
            serde_json::to_vec(&sample_manifest()).unwrap(),
        )
        .unwrap();
        fs::write(dir.join(DB_DIR).join("keel_kv.copy"), "k\tv\n").unwrap();
        let key = dir.join(DATA_DIR).join("tls").join("key.pem");
        fs::write(&key, "KEY").unwrap();
        fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(dir.join("stray.txt"), "not packed").unwrap();
    }

    fn sample_manifest() -> BackupManifest {
        BackupManifest {
            keel: "0.1.0".into(),
            at: "2026-09-21T10:15:30Z".parse().unwrap(),
            instance: None,
            schema: "keel".into(),
            products: Vec::new(),
            modules: Vec::new(),
            tables: Vec::new(),
            sources: Vec::new(),
            config: Vec::new(),
            data: vec!["tls/key.pem".into()],
            secret_key: false,
        }
    }

    #[test]
    fn an_archive_round_trips_its_tree_with_modes_and_answers_its_manifest_first() {
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("staging");
        staging_with_manifest(&staging);
        let out = dir.path().join(format!("site.{EXTENSION}"));
        let bytes = pack(&staging, &out).unwrap();
        assert!(bytes > 0);
        assert!(matches!(
            pack(&staging, &out).unwrap_err(),
            BackupError::Exists { path } if path == out
        ));
        assert_eq!(bytes, fs::metadata(&out).unwrap().len());
        assert_eq!(
            fs::metadata(&out).unwrap().permissions().mode() & 0o777,
            ARCHIVE_MODE
        );
        assert!(!temporary_name(&out).exists());
        assert_eq!(read_manifest(&out).unwrap(), sample_manifest());

        let into = dir.path().join("unpacked");
        unpack(&out, &into).unwrap();
        assert_eq!(
            fs::read_to_string(into.join(DB_DIR).join("keel_kv.copy")).unwrap(),
            "k\tv\n"
        );
        let key = into.join(DATA_DIR).join("tls").join("key.pem");
        assert_eq!(fs::read_to_string(&key).unwrap(), "KEY");
        assert_eq!(
            fs::metadata(&key).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(!into.join("stray.txt").exists());
    }

    #[test]
    fn an_archive_with_a_path_outside_the_layout_or_a_link_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let hostile = dir.path().join("hostile.tar.zst");
        {
            let file = File::create(&hostile).unwrap();
            let encoder = zstd::Encoder::new(file, 1).unwrap();
            let mut builder = Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            let raw = b"etc/../../evil";
            header.as_gnu_mut().unwrap().name[..raw.len()].copy_from_slice(raw);
            header.set_size(2);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, "hi".as_bytes()).unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }
        let error = unpack(&hostile, &dir.path().join("out")).unwrap_err();
        assert!(
            matches!(error, BackupError::Entry { reason, .. } if reason == "its path leaves its directory"),
            "{error}"
        );

        let linked = dir.path().join("linked.tar.zst");
        {
            let file = File::create(&linked).unwrap();
            let encoder = zstd::Encoder::new(file, 1).unwrap();
            let mut builder = Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(EntryType::Symlink);
            header.set_size(0);
            header.set_mode(0o777);
            header.set_cksum();
            builder
                .append_link(&mut header, "data/secret.key", "/etc/passwd")
                .unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }
        let error = unpack(&linked, &dir.path().join("out2")).unwrap_err();
        assert!(
            matches!(error, BackupError::Entry { reason, .. } if reason == "only regular files and directories are unpacked"),
            "{error}"
        );
        assert!(matches!(
            checked_path(Path::new("/etc/x")).unwrap_err(),
            BackupError::Entry {
                reason: "its path is not relative",
                ..
            }
        ));
        assert!(matches!(
            checked_path(Path::new("opt/x")).unwrap_err(),
            BackupError::Entry {
                reason: "its path is outside the archive layout",
                ..
            }
        ));
        assert_eq!(
            checked_path(Path::new("sources/dns.zonefiles/a.zone")).unwrap(),
            PathBuf::from("sources/dns.zonefiles/a.zone")
        );
    }

    #[test]
    fn an_archive_without_a_manifest_or_a_plain_file_is_not_a_backup() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty.tar.zst");
        {
            let file = File::create(&empty).unwrap();
            let encoder = zstd::Encoder::new(file, 1).unwrap();
            let mut builder = Builder::new(encoder);
            builder.append_dir(DB_DIR, dir.path()).unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }
        let error = read_manifest(&empty).unwrap_err();
        assert!(
            error.to_string().contains("carries no manifest.json"),
            "{error}"
        );
        let error = unpack(&empty, &dir.path().join("out")).unwrap_err();
        assert!(
            error.to_string().contains("carries no manifest.json"),
            "{error}"
        );

        let plain = dir.path().join("plain.txt");
        File::create(&plain)
            .unwrap()
            .write_all(b"hello there, not an archive")
            .unwrap();
        let error = read_manifest(&plain).unwrap_err();
        assert!(matches!(error, BackupError::Archive { .. }), "{error}");
        let error = read_manifest(&dir.path().join("missing")).unwrap_err();
        assert!(matches!(error, BackupError::Read { .. }), "{error}");
    }
}
