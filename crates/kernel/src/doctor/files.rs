use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use nix::sys::statvfs::statvfs;
use nix::unistd::{Uid, User, geteuid};

use super::{CONFIG_DIR, Check, DATA_DIR, Findings, INSTALL_DIR, SECRET_KEY, SPACE};
use crate::instance::SECRET_KEY_FILE;

const OTHERS_WRITE: u32 = 0o022;
const OTHERS_ANY: u32 = 0o077;
const MODE_BITS: u32 = 0o7777;
const CURRENT_LINK: [&str; 2] = ["bin", "current"];
const VERSIONS_DIR: &str = "versions";
const BINARY: &str = "keel";
const SPACE_WARN: u64 = 1024 * 1024 * 1024;
const SPACE_FAIL: u64 = 100 * 1024 * 1024;

fn user_name(uid: u32) -> String {
    match User::from_uid(Uid::from_raw(uid)) {
        Ok(Some(user)) => format!("{} (uid {uid})", user.name),
        _ => format!("uid {uid}"),
    }
}

fn mode_of(meta: &fs::Metadata) -> u32 {
    meta.mode() & MODE_BITS
}

fn describe(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::NotFound => "does not exist".to_owned(),
        io::ErrorKind::PermissionDenied => "is not readable by this user".to_owned(),
        _ => format!("cannot be read: {error}"),
    }
}

pub(super) fn data_dir(path: &Path) -> Check {
    let mut findings = Findings::new(DATA_DIR);
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(error) => {
            findings.fail(format!("{} {}", path.display(), describe(&error)));
            return findings.finish("");
        }
    };
    if !meta.is_dir() {
        findings.fail(format!("{} is not a directory", path.display()));
        return findings.finish("");
    }
    let me = geteuid().as_raw();
    let owner = meta.uid();
    if owner != me {
        if me == 0 {
            findings.warn(format!(
                "{} is owned by {}; the doctor runs as root, run it as that user for an exact answer",
                path.display(),
                user_name(owner)
            ));
        } else {
            findings.fail(format!(
                "{} is owned by {}, the doctor runs as {}",
                path.display(),
                user_name(owner),
                user_name(me)
            ));
        }
    }
    let mode = mode_of(&meta);
    if mode & OTHERS_WRITE != 0 {
        findings.fail(format!(
            "{} has mode {mode:04o}; the group or others may write",
            path.display()
        ));
    }
    findings.finish(format!(
        "{} is owned by {} with mode {mode:04o}",
        path.display(),
        user_name(owner)
    ))
}

pub(super) fn config_dir(file: &Path) -> Check {
    let mut findings = Findings::new(CONFIG_DIR);
    let dir = match file.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let dir_meta = match fs::metadata(&dir) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            findings.warn(format!(
                "{} does not exist; the configuration defaults apply",
                dir.display()
            ));
            return findings.finish("");
        }
        Err(error) => {
            findings.fail(format!("{} {}", dir.display(), describe(&error)));
            return findings.finish("");
        }
    };
    let dir_mode = mode_of(&dir_meta);
    if dir_mode & OTHERS_WRITE != 0 {
        findings.fail(format!(
            "{} has mode {dir_mode:04o}; the group or others may write",
            dir.display()
        ));
    }
    let file_text = match fs::metadata(file) {
        Ok(meta) => {
            let mode = mode_of(&meta);
            if mode & OTHERS_WRITE != 0 {
                findings.fail(format!(
                    "{} has mode {mode:04o}; the group or others may write",
                    file.display()
                ));
            }
            format!("{} has mode {mode:04o}", file.display())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            format!("{} is absent, the defaults apply", file.display())
        }
        Err(error) => {
            findings.fail(format!("{} {}", file.display(), describe(&error)));
            String::new()
        }
    };
    findings.finish(format!(
        "{} has mode {dir_mode:04o}; {file_text}",
        dir.display()
    ))
}

pub(super) fn install_dir(opt: &Path, version: &str) -> Check {
    let mut findings = Findings::new(INSTALL_DIR);
    if let Err(error) = fs::metadata(opt) {
        findings.fail(format!(
            "no installation under {}: it {}",
            opt.display(),
            describe(&error)
        ));
        return findings.finish("");
    }
    let link = CURRENT_LINK
        .iter()
        .fold(opt.to_path_buf(), |path, part| path.join(part));
    let target = match fs::read_link(&link) {
        Ok(target) => target,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            findings.fail(format!("{} does not exist", link.display()));
            return findings.finish("");
        }
        Err(_) if fs::symlink_metadata(&link).is_ok() => {
            findings.fail(format!("{} is not a symbolic link", link.display()));
            return findings.finish("");
        }
        Err(error) => {
            findings.fail(format!("{} {}", link.display(), describe(&error)));
            return findings.finish("");
        }
    };
    let resolved = if target.is_absolute() {
        target.clone()
    } else {
        link.parent()
            .map_or(target.clone(), |dir| dir.join(&target))
    };
    let versions = opt.join(VERSIONS_DIR);
    let linked_version = resolved
        .parent()
        .filter(|dir| dir.parent() == Some(versions.as_path()))
        .and_then(|dir| dir.file_name())
        .and_then(|name| name.to_str())
        .filter(|_| resolved.file_name().and_then(|name| name.to_str()) == Some(BINARY));
    let Some(linked_version) = linked_version else {
        findings.fail(format!(
            "{} points at {}, not at {}/<version>/{BINARY}",
            link.display(),
            target.display(),
            versions.display()
        ));
        return findings.finish("");
    };
    match fs::metadata(&resolved) {
        Ok(meta) if meta.is_file() && mode_of(&meta) & 0o111 != 0 => {}
        Ok(_) => findings.fail(format!("{} is not an executable file", resolved.display())),
        Err(error) => findings.fail(format!("{} {}", resolved.display(), describe(&error))),
    }
    if linked_version != version {
        findings.warn(format!(
            "{} points at {linked_version} while this binary is {version}",
            link.display()
        ));
    }
    findings.finish(format!(
        "{} -> {}/{linked_version}/{BINARY}",
        link.display(),
        versions.display()
    ))
}

pub(super) fn secret_key(data_dir: &Path) -> Check {
    let mut findings = Findings::new(SECRET_KEY);
    let path = data_dir.join(SECRET_KEY_FILE);
    match fs::metadata(&path) {
        Ok(meta) => {
            let mode = mode_of(&meta);
            if mode & OTHERS_ANY != 0 {
                findings.fail(format!(
                    "{} has mode {mode:04o}; it must be readable by the service user alone (0600)",
                    path.display()
                ));
            }
            findings.finish(format!("{} has mode {mode:04o}", path.display()))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            findings.fail(format!(
                "{} is absent; the first start of the service writes it",
                path.display()
            ));
            findings.finish("")
        }
        Err(error) => {
            findings.fail(format!("{} {}", path.display(), describe(&error)));
            findings.finish("")
        }
    }
}

fn human(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    let value = bytes as f64;
    if value >= GIB {
        format!("{:.1} GiB", value / GIB)
    } else {
        format!("{:.0} MiB", value / MIB)
    }
}

fn widen(value: impl Into<u64>) -> u64 {
    value.into()
}

fn free_under(path: &Path) -> Result<u64, String> {
    let stat = statvfs(path).map_err(|error| error.to_string())?;
    Ok(widen(stat.blocks_available()).saturating_mul(widen(stat.fragment_size())))
}

pub(super) fn space(data_dir: &Path, opt: &Path) -> Check {
    let mut findings = Findings::new(SPACE);
    let mut parts = Vec::new();
    for path in [data_dir, opt] {
        if fs::metadata(path).is_err() {
            continue;
        }
        match free_under(path) {
            Ok(free) => {
                if free < SPACE_FAIL {
                    findings.fail(format!(
                        "only {} free under {}",
                        human(free),
                        path.display()
                    ));
                } else if free < SPACE_WARN {
                    findings.warn(format!(
                        "{} free under {}; below 1 GiB",
                        human(free),
                        path.display()
                    ));
                }
                parts.push(format!("{} free under {}", human(free), path.display()));
            }
            Err(error) => findings.fail(format!(
                "the free space under {} cannot be read: {error}",
                path.display()
            )),
        }
    }
    if parts.is_empty() {
        findings.fail(format!(
            "neither {} nor {} exists",
            data_dir.display(),
            opt.display()
        ));
    }
    findings.finish(parts.join(", "))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{PermissionsExt, symlink};

    use super::super::Status;
    use super::*;

    fn tree() -> tempfile::TempDir {
        tempfile::tempdir().expect("a temporary tree")
    }

    fn chmod(path: &Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("chmod");
    }

    #[test]
    fn the_data_directory_is_green_when_owned_by_the_caller_and_closed_to_others() {
        let dir = tree();
        let lib = dir.path().join("lib");
        fs::create_dir(&lib).unwrap();
        chmod(&lib, 0o700);
        let check = data_dir(&lib);
        assert_eq!(check.status, Status::Ok, "{}", check.message);
        assert!(check.message.contains("mode 0700"), "{}", check.message);
        chmod(&lib, 0o777);
        let open = data_dir(&lib);
        assert_eq!(open.status, Status::Fail);
        assert!(
            open.message.contains("mode 0777") && open.message.contains("others may write"),
            "{}",
            open.message
        );
        let missing = data_dir(&dir.path().join("nowhere"));
        assert_eq!(missing.status, Status::Fail);
        assert!(
            missing.message.ends_with("does not exist"),
            "{}",
            missing.message
        );
        let file = dir.path().join("file");
        fs::write(&file, "x").unwrap();
        let not_dir = data_dir(&file);
        assert_eq!(not_dir.status, Status::Fail);
        assert!(not_dir.message.ends_with("is not a directory"));
    }

    #[test]
    fn the_config_directory_tolerates_an_absent_file_and_refuses_a_writable_one() {
        let dir = tree();
        let etc = dir.path().join("etc");
        fs::create_dir(&etc).unwrap();
        chmod(&etc, 0o750);
        let file = etc.join("config.yaml");
        let absent = config_dir(&file);
        assert_eq!(absent.status, Status::Ok, "{}", absent.message);
        assert!(absent.message.contains("is absent, the defaults apply"));
        fs::write(&file, "listen: 127.0.0.1:1500\n").unwrap();
        chmod(&file, 0o640);
        let present = config_dir(&file);
        assert_eq!(present.status, Status::Ok, "{}", present.message);
        assert!(present.message.contains("config.yaml has mode 0640"));
        chmod(&file, 0o666);
        let writable = config_dir(&file);
        assert_eq!(writable.status, Status::Fail);
        assert!(writable.message.contains("config.yaml has mode 0666"));
        let nowhere = config_dir(&dir.path().join("missing").join("config.yaml"));
        assert_eq!(nowhere.status, Status::Warn);
        assert!(
            nowhere
                .message
                .contains("does not exist; the configuration defaults apply")
        );
        chmod(&etc, 0o777);
        let open_dir = config_dir(&file);
        assert_eq!(open_dir.status, Status::Fail);
        assert!(
            open_dir.message.contains("etc has mode 0777"),
            "{}",
            open_dir.message
        );
    }

    #[test]
    fn the_install_directory_wants_current_pointing_at_an_executable_of_this_version() {
        let dir = tree();
        let opt = dir.path().join("opt");
        let missing = install_dir(&opt, "0.1.0");
        assert_eq!(missing.status, Status::Fail);
        assert!(missing.message.contains("no installation under"));
        fs::create_dir_all(opt.join("bin")).unwrap();
        let no_link = install_dir(&opt, "0.1.0");
        assert_eq!(no_link.status, Status::Fail);
        assert!(no_link.message.contains("bin/current does not exist"));
        let version_dir = opt.join("versions").join("0.1.0");
        fs::create_dir_all(&version_dir).unwrap();
        let binary = version_dir.join("keel");
        fs::write(&binary, "#!/bin/sh\n").unwrap();
        chmod(&binary, 0o755);
        symlink(&binary, opt.join("bin").join("current")).unwrap();
        let green = install_dir(&opt, "0.1.0");
        assert_eq!(green.status, Status::Ok, "{}", green.message);
        assert!(
            green.message.ends_with("versions/0.1.0/keel"),
            "{}",
            green.message
        );
        let other = install_dir(&opt, "0.2.0");
        assert_eq!(other.status, Status::Warn);
        assert!(
            other
                .message
                .contains("points at 0.1.0 while this binary is 0.2.0")
        );
        chmod(&binary, 0o644);
        let plain = install_dir(&opt, "0.1.0");
        assert_eq!(plain.status, Status::Fail);
        assert!(plain.message.contains("is not an executable file"));
        fs::remove_file(opt.join("bin").join("current")).unwrap();
        symlink(
            dir.path().join("elsewhere"),
            opt.join("bin").join("current"),
        )
        .unwrap();
        let shape = install_dir(&opt, "0.1.0");
        assert_eq!(shape.status, Status::Fail);
        assert!(shape.message.contains("not at"), "{}", shape.message);
        fs::remove_file(opt.join("bin").join("current")).unwrap();
        fs::write(opt.join("bin").join("current"), "x").unwrap();
        let file = install_dir(&opt, "0.1.0");
        assert_eq!(file.status, Status::Fail);
        assert!(file.message.contains("is not a symbolic link"));
    }

    #[test]
    fn the_secret_key_must_exist_with_mode_0600() {
        let dir = tree();
        let absent = secret_key(dir.path());
        assert_eq!(absent.status, Status::Fail);
        assert!(absent.message.contains("is absent; the first start"));
        let key = dir.path().join(SECRET_KEY_FILE);
        fs::write(&key, "00").unwrap();
        chmod(&key, 0o600);
        let green = secret_key(dir.path());
        assert_eq!(green.status, Status::Ok, "{}", green.message);
        chmod(&key, 0o644);
        let open = secret_key(dir.path());
        assert_eq!(open.status, Status::Fail);
        assert!(
            open.message.contains("secret.key has mode 0644"),
            "{}",
            open.message
        );
    }

    #[test]
    fn the_free_space_is_reported_per_root_and_a_missing_root_is_skipped() {
        let dir = tree();
        let check = space(dir.path(), &dir.path().join("nowhere"));
        assert_ne!(check.status, Status::Fail, "{}", check.message);
        assert!(check.message.contains("free under"), "{}", check.message);
        assert_eq!(check.message.matches("free under").count(), 1);
        let none = space(&dir.path().join("a"), &dir.path().join("b"));
        assert_eq!(none.status, Status::Fail);
        assert!(none.message.starts_with("neither"));
        assert_eq!(human(3 * 1024 * 1024 * 1024 / 2), "1.5 GiB");
        assert_eq!(human(50 * 1024 * 1024), "50 MiB");
        assert!(user_name(geteuid().as_raw()).contains("uid"));
        assert_eq!(user_name(u32::MAX - 7), format!("uid {}", u32::MAX - 7));
    }
}
