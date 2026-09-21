use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use super::BackupError;

const TEMPORARY_SUFFIX: &str = ".partial";
pub const KEPT_SUFFIX: &str = ".restored";
const DEFAULT_MODE: u32 = 0o644;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Restored {
    pub written: Vec<String>,
    pub kept: Vec<String>,
}

pub fn snapshot(from: &Path, into: &Path, skip: &[PathBuf]) -> Result<Vec<String>, BackupError> {
    let mut copied = Vec::new();
    if !from.is_dir() {
        return Ok(copied);
    }
    let skipped: Vec<PathBuf> = skip
        .iter()
        .filter_map(|path| path.canonicalize().ok())
        .collect();
    walk(from, into, Path::new(""), &skipped, &mut copied)?;
    copied.sort();
    Ok(copied)
}

fn walk(
    dir: &Path,
    into: &Path,
    relative: &Path,
    skipped: &[PathBuf],
    copied: &mut Vec<String>,
) -> Result<(), BackupError> {
    let entries = fs::read_dir(dir).map_err(|source| BackupError::Read {
        path: dir.to_owned(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| BackupError::Read {
            path: dir.to_owned(),
            source,
        })?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|source| BackupError::Read {
            path: path.clone(),
            source,
        })?;
        let child = relative.join(entry.file_name());
        if kind.is_symlink() || is_skipped(&path, skipped) {
            continue;
        }
        if kind.is_dir() {
            walk(&path, into, &child, skipped, copied)?;
        } else if kind.is_file() {
            let target = into.join(&child);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|source| BackupError::Create {
                    path: parent.to_owned(),
                    source,
                })?;
            }
            fs::copy(&path, &target).map_err(|source| BackupError::Read {
                path: path.clone(),
                source,
            })?;
            copied.push(child.display().to_string());
        }
    }
    Ok(())
}

fn is_skipped(path: &Path, skipped: &[PathBuf]) -> bool {
    path.canonicalize()
        .is_ok_and(|canonical| skipped.iter().any(|skip| canonical.starts_with(skip)))
}

pub fn restore(
    from: &Path,
    into: &Path,
    keep: &[PathBuf],
    except: &[&str],
) -> Result<Restored, BackupError> {
    let mut restored = Restored::default();
    if !from.is_dir() {
        return Ok(restored);
    }
    let kept: Vec<PathBuf> = keep
        .iter()
        .filter_map(|path| path.canonicalize().ok())
        .collect();
    let mut files = Vec::new();
    collect(from, Path::new(""), &mut files)?;
    files.sort();
    for relative in files {
        if except.iter().any(|name| Path::new(name) == relative) {
            continue;
        }
        let source = from.join(&relative);
        let target = into.join(&relative);
        let mode = fs::metadata(&source)
            .map(|meta| meta.permissions().mode() & 0o777)
            .unwrap_or(DEFAULT_MODE);
        let content = fs::read(&source).map_err(|source_error| BackupError::Read {
            path: source.clone(),
            source: source_error,
        })?;
        let name = relative.display().to_string();
        if kept.iter().any(|kept| {
            target
                .canonicalize()
                .is_ok_and(|canonical| canonical == *kept)
        }) {
            let mut beside = target.as_os_str().to_owned();
            beside.push(KEPT_SUFFIX);
            write_whole(Path::new(&beside), &content, mode)?;
            restored.kept.push(name);
        } else {
            write_whole(&target, &content, mode)?;
            restored.written.push(name);
        }
    }
    Ok(restored)
}

fn collect(dir: &Path, relative: &Path, files: &mut Vec<PathBuf>) -> Result<(), BackupError> {
    let entries = fs::read_dir(dir).map_err(|source| BackupError::Read {
        path: dir.to_owned(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| BackupError::Read {
            path: dir.to_owned(),
            source,
        })?;
        let kind = entry.file_type().map_err(|source| BackupError::Read {
            path: entry.path(),
            source,
        })?;
        let child = relative.join(entry.file_name());
        if kind.is_dir() {
            collect(&entry.path(), &child, files)?;
        } else if kind.is_file() {
            files.push(child);
        }
    }
    Ok(())
}

pub fn write_whole(path: &Path, content: &[u8], mode: u32) -> Result<(), BackupError> {
    let failed = |source| BackupError::Write {
        path: path.to_owned(),
        source,
    };
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|source| BackupError::Create {
            path: parent.to_owned(),
            source,
        })?;
    }
    if fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        fs::remove_file(path).map_err(failed)?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(TEMPORARY_SUFFIX);
    let temporary = PathBuf::from(temporary);
    let _ = fs::remove_file(&temporary);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&temporary)
        .map_err(failed)?;
    let written = file
        .write_all(content)
        .and_then(|()| file.set_permissions(fs::Permissions::from_mode(mode)))
        .and_then(|()| file.sync_all());
    if let Err(source) = written {
        let _ = fs::remove_file(&temporary);
        return Err(failed(source));
    }
    fs::rename(&temporary, path).map_err(failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_copies_regular_files_and_skips_links_and_the_named_trees() {
        let dir = tempfile::tempdir().unwrap();
        let etc = dir.path().join("etc");
        fs::create_dir_all(etc.join("templates").join("bind")).unwrap();
        fs::create_dir_all(etc.join("lib").join("backups")).unwrap();
        fs::write(etc.join("config.yaml"), "listen: x\n").unwrap();
        fs::write(etc.join("templates/bind/zone.j2"), "zone").unwrap();
        fs::write(etc.join("lib/backups/old.tar.zst"), "big").unwrap();
        std::os::unix::fs::symlink("/etc/hostname", etc.join("link")).unwrap();
        let into = dir.path().join("staging");
        let copied = snapshot(&etc, &into, &[etc.join("lib")]).unwrap();
        assert_eq!(copied, ["config.yaml", "templates/bind/zone.j2"]);
        assert_eq!(
            fs::read_to_string(into.join("templates/bind/zone.j2")).unwrap(),
            "zone"
        );
        assert!(!into.join("link").exists());
        assert!(!into.join("lib").exists());
        assert!(
            snapshot(&dir.path().join("missing"), &into, &[])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_restore_writes_files_with_their_modes_replaces_links_and_keeps_the_named_file_beside() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("staged");
        fs::create_dir_all(staged.join("tls")).unwrap();
        fs::write(staged.join("config.yaml"), "from: archive\n").unwrap();
        fs::write(staged.join("tls/key.pem"), "KEY").unwrap();
        fs::set_permissions(
            staged.join("tls/key.pem"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        fs::write(staged.join("agent.yaml"), "agent").unwrap();
        let target = dir.path().join("etc");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("config.yaml"), "from: host\n").unwrap();
        fs::write(dir.path().join("elsewhere"), "old").unwrap();
        std::os::unix::fs::symlink(dir.path().join("elsewhere"), target.join("agent.yaml"))
            .unwrap();
        fs::write(staged.join("secret.key.sealed"), "sealed").unwrap();
        let restored = restore(
            &staged,
            &target,
            &[target.join("config.yaml")],
            &["secret.key.sealed"],
        )
        .unwrap();
        assert_eq!(restored.written, ["agent.yaml", "tls/key.pem"]);
        assert!(!target.join("secret.key.sealed").exists());
        assert_eq!(restored.kept, ["config.yaml"]);
        assert_eq!(
            fs::read_to_string(target.join("config.yaml")).unwrap(),
            "from: host\n"
        );
        assert_eq!(
            fs::read_to_string(target.join("config.yaml.restored")).unwrap(),
            "from: archive\n"
        );
        assert!(
            !fs::symlink_metadata(target.join("agent.yaml"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_to_string(target.join("agent.yaml")).unwrap(),
            "agent"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("elsewhere")).unwrap(),
            "old"
        );
        assert_eq!(
            fs::metadata(target.join("tls/key.pem"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            restore(&dir.path().join("missing"), &target, &[], &[]).unwrap(),
            Restored::default()
        );
    }
}
