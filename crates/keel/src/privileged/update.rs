use std::fs;
use std::io;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Component, Path, PathBuf};
use std::process::ExitStatus;

use keel_release::{
    Channel, KeyRing, Manifest, ReleaseError, Signed, Version, archive_path, manifest_path,
};
use tar::EntryType;

use super::channel::{Source, SourceError};
use super::request::Request;

const MANIFEST_LIMIT: u64 = 1024 * 1024;
const ARCHIVE_LIMIT: u64 = 512 * 1024 * 1024;
const UNPACKED_LIMIT: u64 = 2 * 1024 * 1024 * 1024;
const ENTRIES_LIMIT: usize = 1024;
const REFUSED_MODE_BITS: u32 = 0o7022;
const BINARY: &str = "keel";
const MANIFEST: &str = "manifest.json";
const UNITS: &str = "deploy/systemd";
const KERNEL_UNIT: &str = "keel.service";
const PRIVILEGED_UNIT: &str = "keel-privileged.service";

pub(crate) struct Runner {
    pub(crate) opt: PathBuf,
    pub(crate) unit_dir: PathBuf,
    pub(crate) systemctl: PathBuf,
    pub(crate) keys: KeyRing,
    pub(crate) source: Source,
}

#[derive(Debug)]
pub(crate) struct Installed {
    pub(crate) version: Version,
    pub(crate) channel: Channel,
    pub(crate) requested_by: String,
    pub(crate) previous: Version,
    pub(crate) units: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct Switched {
    pub(crate) version: Version,
    pub(crate) previous: Version,
    pub(crate) units: Vec<String>,
    pub(crate) restore: Option<PathBuf>,
}

struct Link {
    version: Version,
    target: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum UpdateError {
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error(transparent)]
    Release(#[from] ReleaseError),
    #[error(
        "the head of {channel} verified, the manifest of {version} too, but it describes {described}"
    )]
    VersionMismatch {
        channel: Channel,
        version: Version,
        described: Version,
    },
    #[error("{version} is the version bin/current points at")]
    AlreadyCurrent { version: Version },
    #[error(
        "{version} is below the current {current} and is not the version bin/previous points at ({previous}); a request moves down only to the previous version"
    )]
    NotPrevious {
        version: Version,
        current: Version,
        previous: String,
    },
    #[error("{link} does not point into {versions}/<version>/{BINARY}: {target}")]
    LinkShape {
        link: PathBuf,
        versions: PathBuf,
        target: PathBuf,
    },
    #[error("no previous version is recorded at {0}")]
    NoPrevious(PathBuf),
    #[error("the previous version has no binary at {0}")]
    PreviousGone(PathBuf),
    #[error("{0} has no {MANIFEST}; its contract is unknown, so the rollback needs --restore")]
    NoManifest(PathBuf),
    #[error(
        "the rollback from {current} (contract {current_contract}) to {previous} (contract {previous_contract}) crosses a contract boundary; --restore <archive> restores the pre-update backup with it"
    )]
    AcrossContract {
        current: Version,
        current_contract: u32,
        previous: Version,
        previous_contract: u32,
    },
    #[error("the backup archive {0} is not a file")]
    RestoreArchive(PathBuf),
    #[error("the archive carries no {BINARY} at its root")]
    NoBinary,
    #[error("the archive entry {path} is a {kind:?}, which a release never carries")]
    EntryKind { path: PathBuf, kind: EntryType },
    #[error("the archive entry {0} escapes the release directory")]
    EntryEscapes(PathBuf),
    #[error(
        "the archive entry {path} carries mode {mode:o} with a setuid, setgid or sticky bit or a write bit for group or others"
    )]
    EntryMode { path: PathBuf, mode: u32 },
    #[error("the archive unpacks to more than {0} bytes")]
    TooLarge(u64),
    #[error("the archive carries more than {0} entries")]
    TooManyEntries(usize),
    #[error("could not read the archive: {0}")]
    Archive(#[source] io::Error),
    #[error("could not {action} {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{systemctl} {args} could not be started: {source}")]
    Spawn {
        systemctl: PathBuf,
        args: String,
        #[source]
        source: io::Error,
    },
    #[error("{systemctl} {args} exited with {status}: {stderr}")]
    Systemctl {
        systemctl: PathBuf,
        args: String,
        status: ExitStatus,
        stderr: String,
    },
    #[error("the unpacking thread did not finish: {0}")]
    Join(#[source] tokio::task::JoinError),
}

fn io_error(action: &'static str, path: &Path) -> impl FnOnce(io::Error) -> UpdateError {
    let path = path.to_owned();
    move |source| UpdateError::Io {
        action,
        path,
        source,
    }
}

impl Runner {
    fn versions(&self) -> PathBuf {
        self.opt.join("versions")
    }

    fn version_dir(&self, version: &Version) -> PathBuf {
        self.versions().join(version.to_string())
    }

    fn current_link(&self) -> PathBuf {
        self.opt.join("bin").join("current")
    }

    fn previous_link(&self) -> PathBuf {
        self.opt.join("bin").join("previous")
    }

    pub(crate) async fn install(&self, request: &Request) -> Result<Installed, UpdateError> {
        let current = self.read_link(&self.current_link())?;
        if current.version == request.version {
            return Err(UpdateError::AlreadyCurrent {
                version: request.version.clone(),
            });
        }
        println!(
            "update: {} from {} requested by {}; current {}",
            request.version, request.channel, request.requested_by, current.version
        );
        let head = self.verified(&request.channel.head_path()).await?;
        head.admits(&request.version)?;
        println!(
            "update: the head of {} is {} with min_upgrade_from {}",
            request.channel, head.version, head.min_upgrade_from
        );
        let manifest_bytes = self
            .source
            .fetch(&manifest_path(&request.version), MANIFEST_LIMIT)
            .await?;
        let manifest = Signed::parse(&manifest_bytes)?.verify(&self.keys)?;
        if manifest.version != request.version {
            return Err(UpdateError::VersionMismatch {
                channel: request.channel,
                version: request.version.clone(),
                described: manifest.version,
            });
        }
        manifest.admits(&current.version)?;
        if request.version < current.version {
            self.admits_descent(&current, &manifest)?;
        }
        println!(
            "update: the manifest of {} verified (contract {}, min_upgrade_from {})",
            manifest.version, manifest.contract, manifest.min_upgrade_from
        );
        let versions = self.versions();
        fs::create_dir_all(&versions).map_err(io_error("create", &versions))?;
        let staging = tempfile::Builder::new()
            .prefix(".update-")
            .tempdir_in(&versions)
            .map_err(io_error("create a staging directory under", &versions))?;
        let archive = staging.path().join(manifest.archive_name());
        let digest = self
            .source
            .fetch_to(&archive_path(&request.version), &archive, ARCHIVE_LIMIT)
            .await?;
        manifest.check_digest(&digest)?;
        println!("update: the archive matches sha256 {digest}");
        let unpacked = staging.path().join("release");
        let unpack_into = unpacked.clone();
        tokio::task::spawn_blocking(move || unpack(&archive, &unpack_into, UNPACKED_LIMIT))
            .await
            .map_err(UpdateError::Join)??;
        let unpacked_manifest = unpacked.join(MANIFEST);
        fs::write(&unpacked_manifest, &manifest_bytes)
            .map_err(io_error("write", &unpacked_manifest))?;
        let dest = self.version_dir(&request.version);
        if dest.exists() {
            fs::remove_dir_all(&dest).map_err(io_error("remove", &dest))?;
        }
        fs::rename(&unpacked, &dest).map_err(io_error("move the release to", &dest))?;
        println!("update: installed {}", dest.display());
        self.switch(&current, &request.version)?;
        let units = self.refresh_units(&request.version)?;
        self.restart(false).await?;
        Ok(Installed {
            version: request.version.clone(),
            channel: request.channel,
            requested_by: request.requested_by.clone(),
            previous: current.version,
            units,
        })
    }

    pub(crate) async fn rollback(&self, restore: Option<&Path>) -> Result<Switched, UpdateError> {
        let current = self.read_link(&self.current_link())?;
        let previous_link = self.previous_link();
        if !previous_link.is_symlink() {
            return Err(UpdateError::NoPrevious(previous_link));
        }
        let previous = self.read_link(&previous_link)?;
        if !previous.target.is_file() {
            return Err(UpdateError::PreviousGone(previous.target));
        }
        match restore {
            Some(archive) => {
                if !archive.is_file() {
                    return Err(UpdateError::RestoreArchive(archive.to_owned()));
                }
            }
            None => {
                let current_contract = self.installed_manifest(&current.version)?.contract;
                let previous_contract = self.installed_manifest(&previous.version)?.contract;
                if current_contract != previous_contract {
                    return Err(UpdateError::AcrossContract {
                        current: current.version,
                        current_contract,
                        previous: previous.version,
                        previous_contract,
                    });
                }
            }
        }
        println!(
            "rollback: from {} to {}{}",
            current.version,
            previous.version,
            restore
                .map(|archive| format!(" with the backup {}", archive.display()))
                .unwrap_or_default()
        );
        self.switch(&current, &previous.version)?;
        let units = self.refresh_units(&previous.version)?;
        self.restart(restore.is_some()).await?;
        Ok(Switched {
            version: previous.version,
            previous: current.version,
            units,
            restore: restore.map(Path::to_owned),
        })
    }

    fn admits_descent(&self, current: &Link, target: &Manifest) -> Result<(), UpdateError> {
        let previous_link = self.previous_link();
        let previous = if previous_link.is_symlink() {
            Some(self.read_link(&previous_link)?.version)
        } else {
            None
        };
        if previous.as_ref() != Some(&target.version) {
            return Err(UpdateError::NotPrevious {
                version: target.version.clone(),
                current: current.version.clone(),
                previous: previous.map_or_else(|| "none".to_owned(), |version| version.to_string()),
            });
        }
        let current_contract = self.installed_manifest(&current.version)?.contract;
        if current_contract != target.contract {
            return Err(UpdateError::AcrossContract {
                current: current.version.clone(),
                current_contract,
                previous: target.version.clone(),
                previous_contract: target.contract,
            });
        }
        Ok(())
    }

    async fn verified(&self, relative: &str) -> Result<Manifest, UpdateError> {
        let bytes = self.source.fetch(relative, MANIFEST_LIMIT).await?;
        Ok(Signed::parse(&bytes)?.verify(&self.keys)?)
    }

    fn installed_manifest(&self, version: &Version) -> Result<Manifest, UpdateError> {
        let path = self.version_dir(version).join(MANIFEST);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(UpdateError::NoManifest(path));
            }
            Err(source) => return Err(io_error("read", &path)(source)),
        };
        Ok(Signed::parse(&bytes)?.verify(&self.keys)?)
    }

    fn read_link(&self, link: &Path) -> Result<Link, UpdateError> {
        let target = fs::read_link(link).map_err(io_error("read the link", link))?;
        let versions = self.versions();
        let shape = || UpdateError::LinkShape {
            link: link.to_owned(),
            versions: versions.clone(),
            target: target.clone(),
        };
        if target.file_name().and_then(|name| name.to_str()) != Some(BINARY) {
            return Err(shape());
        }
        let dir = target.parent().ok_or_else(shape)?;
        if dir.parent() != Some(versions.as_path()) {
            return Err(shape());
        }
        let version = dir
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.parse().ok())
            .ok_or_else(shape)?;
        Ok(Link {
            version,
            target: target.clone(),
        })
    }

    fn switch(&self, current: &Link, to: &Version) -> Result<(), UpdateError> {
        let previous_link = self.previous_link();
        replace_link(&previous_link, &current.target)?;
        let target = self.version_dir(to).join(BINARY);
        replace_link(&self.current_link(), &target)?;
        println!(
            "update: bin/current -> {} (previous {})",
            target.display(),
            current.target.display()
        );
        Ok(())
    }

    fn refresh_units(&self, version: &Version) -> Result<Vec<String>, UpdateError> {
        let shipped = self.version_dir(version).join(UNITS);
        let entries = match fs::read_dir(&shipped) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                println!(
                    "update: the release carries no {UNITS}; the unit files are left as they are"
                );
                return Ok(Vec::new());
            }
            Err(source) => return Err(io_error("read", &shipped)(source)),
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.map_err(io_error("read", &shipped))?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let is_unit =
                name.starts_with("keel") && (name.ends_with(".service") || name.ends_with(".path"));
            if !is_unit || !entry.path().is_file() {
                continue;
            }
            let content = fs::read(entry.path()).map_err(io_error("read", &entry.path()))?;
            let dest = self.unit_dir.join(&name);
            write_replacing(&dest, &content, 0o644)?;
            names.push(name);
        }
        names.sort();
        if !names.is_empty() {
            println!(
                "update: unit files refreshed in {}: {}",
                self.unit_dir.display(),
                names.join(", ")
            );
        }
        Ok(names)
    }

    async fn restart(&self, stop_kernel: bool) -> Result<(), UpdateError> {
        self.systemctl(&["daemon-reload"]).await?;
        self.systemctl(&["restart", PRIVILEGED_UNIT]).await?;
        if stop_kernel {
            self.systemctl(&["stop", KERNEL_UNIT]).await?;
            println!("update: {KERNEL_UNIT} stopped for the restore");
        } else {
            self.systemctl(&["restart", KERNEL_UNIT]).await?;
            println!("update: {PRIVILEGED_UNIT} and {KERNEL_UNIT} restarted");
        }
        Ok(())
    }

    async fn systemctl(&self, args: &[&str]) -> Result<(), UpdateError> {
        let output = tokio::process::Command::new(&self.systemctl)
            .args(args)
            .output()
            .await
            .map_err(|source| UpdateError::Spawn {
                systemctl: self.systemctl.clone(),
                args: args.join(" "),
                source,
            })?;
        if !output.status.success() {
            return Err(UpdateError::Systemctl {
                systemctl: self.systemctl.clone(),
                args: args.join(" "),
                status: output.status,
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }
        Ok(())
    }
}

fn replace_link(link: &Path, target: &Path) -> Result<(), UpdateError> {
    let parent = link.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(io_error("create", parent))?;
    let name = link
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("link");
    let staged = parent.join(format!(".{name}.new"));
    match fs::remove_file(&staged) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => return Err(io_error("remove", &staged)(source)),
    }
    symlink(target, &staged).map_err(io_error("create the link", &staged))?;
    fs::rename(&staged, link).map_err(io_error("replace the link", link))
}

fn write_replacing(dest: &Path, content: &[u8], mode: u32) -> Result<(), UpdateError> {
    let parent = dest.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(io_error("create", parent))?;
    let name = dest
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unit");
    let staged = parent.join(format!(".{name}.new"));
    fs::write(&staged, content).map_err(io_error("write", &staged))?;
    fs::set_permissions(&staged, fs::Permissions::from_mode(mode))
        .map_err(io_error("set the mode of", &staged))?;
    fs::rename(&staged, dest).map_err(io_error("replace", dest))
}

fn survey(archive: &Path, limit: u64) -> Result<(), UpdateError> {
    let file = fs::File::open(archive).map_err(io_error("open", archive))?;
    let decoder = zstd::Decoder::new(file).map_err(UpdateError::Archive)?;
    let mut tar = tar::Archive::new(decoder);
    let mut entries = 0usize;
    let mut unpacked = 0u64;
    for entry in tar.entries().map_err(UpdateError::Archive)? {
        let entry = entry.map_err(UpdateError::Archive)?;
        entries += 1;
        if entries > ENTRIES_LIMIT {
            return Err(UpdateError::TooManyEntries(ENTRIES_LIMIT));
        }
        unpacked = unpacked.saturating_add(entry.size());
        if unpacked > limit {
            return Err(UpdateError::TooLarge(limit));
        }
    }
    Ok(())
}

fn unpack(archive: &Path, dest: &Path, limit: u64) -> Result<(), UpdateError> {
    survey(archive, limit)?;
    fs::create_dir_all(dest).map_err(io_error("create", dest))?;
    let file = fs::File::open(archive).map_err(io_error("open", archive))?;
    let decoder = zstd::Decoder::new(file).map_err(UpdateError::Archive)?;
    let mut tar = tar::Archive::new(decoder);
    tar.set_preserve_permissions(true);
    tar.set_preserve_mtime(false);
    tar.set_overwrite(false);
    let mut unpacked = 0u64;
    for entry in tar.entries().map_err(UpdateError::Archive)? {
        let mut entry = entry.map_err(UpdateError::Archive)?;
        let path = entry.path().map_err(UpdateError::Archive)?.into_owned();
        unpacked = unpacked.saturating_add(entry.size());
        if unpacked > limit {
            return Err(UpdateError::TooLarge(limit));
        }
        let kind = entry.header().entry_type();
        if !matches!(kind, EntryType::Regular | EntryType::Directory) {
            return Err(UpdateError::EntryKind { path, kind });
        }
        let mode = entry.header().mode().map_err(UpdateError::Archive)?;
        if mode & REFUSED_MODE_BITS != 0 {
            return Err(UpdateError::EntryMode { path, mode });
        }
        if path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(UpdateError::EntryEscapes(path));
        }
        if !entry.unpack_in(dest).map_err(UpdateError::Archive)? {
            return Err(UpdateError::EntryEscapes(path));
        }
    }
    let binary = dest.join(BINARY);
    if !binary.is_file() {
        return Err(UpdateError::NoBinary);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive_with(entries: &[(&str, EntryType, &[u8])], dir: &Path) -> PathBuf {
        let with_modes: Vec<_> = entries
            .iter()
            .map(|(name, kind, content)| (*name, *kind, 0o755, *content))
            .collect();
        archive_with_modes(&with_modes, dir)
    }

    fn archive_with_modes(entries: &[(&str, EntryType, u32, &[u8])], dir: &Path) -> PathBuf {
        let path = dir.join("release.tar.zst");
        let file = fs::File::create(&path).unwrap();
        let encoder = zstd::Encoder::new(file, 3).unwrap();
        let mut builder = tar::Builder::new(encoder);
        for (name, kind, mode, content) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(*kind);
            header.set_mode(*mode);
            header.set_size(content.len() as u64);
            match kind {
                EntryType::Symlink => {
                    header.set_size(0);
                    builder
                        .append_link(&mut header, name, "/etc/passwd")
                        .unwrap();
                }
                _ if name.contains("..") => {
                    let gnu = header.as_gnu_mut().unwrap();
                    gnu.name[..name.len()].copy_from_slice(name.as_bytes());
                    header.set_cksum();
                    builder.append(&header, &content[..]).unwrap();
                }
                _ => {
                    builder
                        .append_data(&mut header, name, &content[..])
                        .unwrap();
                }
            }
        }
        builder.into_inner().unwrap().finish().unwrap();
        path
    }

    #[test]
    fn unpack_takes_files_and_directories_and_refuses_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let archive = archive_with(
            &[
                ("keel", EntryType::Regular, b"#!/bin/sh\n"),
                ("deploy/", EntryType::Directory, b""),
                (
                    "deploy/systemd/keel.service",
                    EntryType::Regular,
                    b"[Unit]\n",
                ),
            ],
            dir.path(),
        );
        let dest = dir.path().join("out");
        unpack(&archive, &dest, UNPACKED_LIMIT).unwrap();
        assert_eq!(fs::read(dest.join("keel")).unwrap(), b"#!/bin/sh\n");
        assert_eq!(
            fs::metadata(dest.join("keel"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert!(dest.join("deploy/systemd/keel.service").is_file());

        let without = archive_with(&[("other", EntryType::Regular, b"x")], dir.path());
        assert!(matches!(
            unpack(&without, &dir.path().join("out2"), UNPACKED_LIMIT),
            Err(UpdateError::NoBinary)
        ));

        let link = archive_with(
            &[
                ("keel", EntryType::Regular, b"x"),
                ("passwd", EntryType::Symlink, b""),
            ],
            dir.path(),
        );
        assert!(matches!(
            unpack(&link, &dir.path().join("out3"), UNPACKED_LIMIT),
            Err(UpdateError::EntryKind {
                kind: EntryType::Symlink,
                ..
            })
        ));

        let escaping = archive_with(
            &[
                ("keel", EntryType::Regular, b"x"),
                ("../outside", EntryType::Regular, b"x"),
            ],
            dir.path(),
        );
        assert!(matches!(
            unpack(&escaping, &dir.path().join("out4"), UNPACKED_LIMIT),
            Err(UpdateError::EntryEscapes(path)) if path == Path::new("../outside")
        ));
        assert!(!dir.path().join("outside").exists());

        let setuid = archive_with_modes(
            &[("keel", EntryType::Regular, 0o4755, b"#!/bin/sh\n")],
            dir.path(),
        );
        let out6 = dir.path().join("out6");
        assert!(matches!(
            unpack(&setuid, &out6, UNPACKED_LIMIT),
            Err(UpdateError::EntryMode { path, mode: 0o4755 }) if path == Path::new("keel")
        ));
        assert!(!out6.join("keel").exists());

        let setgid = archive_with_modes(
            &[
                ("keel", EntryType::Regular, 0o755, b"x"),
                ("lib/", EntryType::Directory, 0o2755, b""),
            ],
            dir.path(),
        );
        assert!(matches!(
            unpack(&setgid, &dir.path().join("out7"), UNPACKED_LIMIT),
            Err(UpdateError::EntryMode { mode: 0o2755, .. })
        ));

        let broken = dir.path().join("broken.tar.zst");
        fs::write(&broken, b"not zstd").unwrap();
        assert!(matches!(
            unpack(&broken, &dir.path().join("out5"), UNPACKED_LIMIT),
            Err(UpdateError::Archive(_))
        ));
    }

    fn pax_record(key: &str, value: &str) -> Vec<u8> {
        let body = format!(" {key}={value}\n");
        let mut length = body.len() + 1;
        while format!("{length}{body}").len() != length {
            length += 1;
        }
        format!("{length}{body}").into_bytes()
    }

    fn archive_with_pax_size(dir: &Path, hidden: usize) -> PathBuf {
        let path = dir.join("pax.tar.zst");
        let file = fs::File::create(&path).unwrap();
        let mut builder = tar::Builder::new(zstd::Encoder::new(file, 3).unwrap());
        let mut header = tar::Header::new_ustar();
        header.set_mode(0o755);
        header.set_size(10);
        builder
            .append_data(&mut header, "keel", &b"#!/bin/sh\n"[..])
            .unwrap();
        let record = pax_record("size", &hidden.to_string());
        let mut pax = tar::Header::new_ustar();
        pax.set_entry_type(EntryType::XHeader);
        pax.set_path("PaxHeaders/big").unwrap();
        pax.set_mode(0o644);
        pax.set_size(record.len() as u64);
        pax.set_cksum();
        builder.append(&pax, &record[..]).unwrap();
        let mut big = tar::Header::new_ustar();
        big.set_path("big").unwrap();
        big.set_mode(0o644);
        big.set_size(0);
        big.set_cksum();
        builder.append(&big, &vec![b'x'; hidden][..]).unwrap();
        builder.into_inner().unwrap().finish().unwrap();
        path
    }

    #[test]
    fn an_entry_whose_pax_size_overrides_its_header_counts_against_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let archive = archive_with_pax_size(dir.path(), 4096);
        let out = dir.path().join("small");
        assert!(matches!(
            unpack(&archive, &out, 1024),
            Err(UpdateError::TooLarge(1024))
        ));
        assert!(!out.join("big").exists());
        let roomy = dir.path().join("roomy");
        unpack(&archive, &roomy, 8192).unwrap();
        assert_eq!(fs::metadata(roomy.join("big")).unwrap().len(), 4096);
    }

    #[test]
    fn an_archive_with_too_many_entries_is_refused_by_the_unpacker() {
        let dir = tempfile::tempdir().unwrap();
        let names: Vec<String> = (1..ENTRIES_LIMIT)
            .map(|index| format!("files/{index}"))
            .collect();
        let mut entries: Vec<(&str, EntryType, &[u8])> =
            vec![("keel", EntryType::Regular, &b"#!/bin/sh\n"[..])];
        entries.extend(
            names
                .iter()
                .map(|name| (name.as_str(), EntryType::Regular, &b"x"[..])),
        );
        assert_eq!(entries.len(), ENTRIES_LIMIT);
        let at_limit = archive_with(&entries, dir.path());
        unpack(&at_limit, &dir.path().join("at"), UNPACKED_LIMIT).unwrap();
        entries.push(("one-more", EntryType::Regular, &b"x"[..]));
        let over = archive_with(&entries, dir.path());
        let out = dir.path().join("over");
        assert!(matches!(
            unpack(&over, &out, UNPACKED_LIMIT),
            Err(UpdateError::TooManyEntries(ENTRIES_LIMIT))
        ));
        assert!(!out.exists());
    }

    #[test]
    fn an_entry_writable_by_group_or_others_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        for (index, mode) in [0o775, 0o757, 0o666, 0o777].into_iter().enumerate() {
            let archive = archive_with_modes(
                &[
                    ("keel", EntryType::Regular, 0o755, b"#!/bin/sh\n"),
                    (
                        "deploy/agent.yaml",
                        EntryType::Regular,
                        mode,
                        b"policy: {}\n",
                    ),
                ],
                dir.path(),
            );
            let out = dir.path().join(format!("out{index}"));
            assert!(
                matches!(
                    unpack(&archive, &out, UNPACKED_LIMIT),
                    Err(UpdateError::EntryMode { ref path, mode: refused })
                        if path == Path::new("deploy/agent.yaml") && refused == mode
                ),
                "{mode:o}"
            );
            assert!(!out.join("deploy/agent.yaml").exists());
        }
        let dirs = archive_with_modes(
            &[
                ("keel", EntryType::Regular, 0o755, b"x"),
                ("deploy/", EntryType::Directory, 0o775, b""),
            ],
            dir.path(),
        );
        assert!(matches!(
            unpack(&dirs, &dir.path().join("dirs"), UNPACKED_LIMIT),
            Err(UpdateError::EntryMode { mode: 0o775, .. })
        ));
    }

    #[test]
    fn links_and_unit_files_are_replaced_atomically_through_a_staged_name() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("bin/current");
        replace_link(&link, Path::new("/opt/keel/versions/0.1.0/keel")).unwrap();
        replace_link(&link, Path::new("/opt/keel/versions/0.2.0/keel")).unwrap();
        assert_eq!(
            fs::read_link(&link).unwrap(),
            Path::new("/opt/keel/versions/0.2.0/keel")
        );
        assert!(!dir.path().join("bin/.current.new").exists());
        let unit = dir.path().join("units/keel.service");
        write_replacing(&unit, b"[Unit]\n", 0o644).unwrap();
        write_replacing(&unit, b"[Unit]\nDescription=x\n", 0o644).unwrap();
        assert_eq!(fs::read(&unit).unwrap(), b"[Unit]\nDescription=x\n");
        assert_eq!(
            fs::metadata(&unit).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert!(!dir.path().join("units/.keel.service.new").exists());
    }
}
