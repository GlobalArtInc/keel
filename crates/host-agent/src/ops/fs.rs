use std::fs::{self, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};

use nix::fcntl::OFlag;
use nix::unistd::geteuid;
use serde::Deserialize;
use serde_json::{Value, json};

use super::files::{Kind, Placement, Previous, place, settle};
use super::{Call, Dest};
use crate::protocol::{ERROR_IO, ERROR_NOT_FOUND, Failure};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Write {
    dest: Dest,
    content: String,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    group: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Read {
    dest: Dest,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Remove {
    dest: Dest,
    #[serde(default)]
    recursive: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Mkdir {
    dest: Dest,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    group: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct List {
    root: String,
}

pub(super) fn write(call: &Call<'_>, args: Write) -> Result<Value, Failure> {
    let target = call.resolve(&args.dest)?;
    let placement = Placement::resolve(
        call,
        args.mode.as_deref(),
        args.owner.as_deref(),
        args.group.as_deref(),
        Kind::File,
    )?;
    let previous = Previous::read(&target)?;
    if previous.as_ref().is_some_and(|previous| {
        previous.content == args.content.as_bytes() && placement.matches(previous)
    }) {
        return Ok(json!({ "changed": false }));
    }
    place(
        &target,
        args.content.as_bytes(),
        placement.mode,
        placement.owner,
    )?;
    Ok(json!({ "changed": true }))
}

pub(super) fn read(call: &Call<'_>, args: Read) -> Result<Value, Failure> {
    let target = call.resolve(&args.dest)?;
    let previous = Previous::read(&target)?.ok_or_else(|| {
        Failure::new(
            ERROR_NOT_FOUND,
            format!("{} does not exist", args.dest.name),
        )
    })?;
    let content = String::from_utf8(previous.content)
        .map_err(|_| Failure::new(ERROR_IO, format!("{} is not UTF-8 text", args.dest.name)))?;
    Ok(json!({
        "content": content,
        "mode": format!("{:04o}", previous.mode),
        "uid": previous.owner.uid,
        "gid": previous.owner.gid,
        "size": content.len(),
    }))
}

pub(super) fn remove(call: &Call<'_>, args: Remove) -> Result<Value, Failure> {
    let target = call.resolve(&args.dest)?;
    let meta = match fs::symlink_metadata(&target) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(json!({ "removed": false }));
        }
        Err(error) => return Err(Failure::io(target.display(), &error)),
    };
    let removed = if meta.is_dir() {
        if args.recursive {
            fs::remove_dir_all(&target)
        } else {
            fs::remove_dir(&target)
        }
    } else {
        fs::remove_file(&target)
    };
    removed.map_err(|error| Failure::io(target.display(), &error))?;
    Ok(json!({ "removed": true }))
}

pub(super) fn mkdir(call: &Call<'_>, args: Mkdir) -> Result<Value, Failure> {
    let target = call.resolve(&args.dest)?;
    let placement = Placement::resolve(
        call,
        args.mode.as_deref(),
        args.owner.as_deref(),
        args.group.as_deref(),
        Kind::Dir,
    )?;
    match fs::symlink_metadata(&target) {
        Ok(meta) if meta.is_dir() => return Ok(json!({ "created": false })),
        Ok(_) => {
            return Err(Failure::policy(format!(
                "{} exists and is not a directory",
                target.display()
            )));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(Failure::io(target.display(), &error)),
    }
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&target)
        .map_err(|error| Failure::io(target.display(), &error))?;
    let dir = OpenOptions::new()
        .read(true)
        .custom_flags((OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW).bits())
        .open(&target)
        .map_err(|error| Failure::io(target.display(), &error))?;
    let meta = dir
        .metadata()
        .map_err(|error| Failure::io(target.display(), &error))?;
    if meta.uid() != geteuid().as_raw() {
        return Err(Failure::policy(format!(
            "{} was replaced before the agent could settle it",
            target.display()
        )));
    }
    settle(&dir, placement.mode, placement.owner)
        .map_err(|error| Failure::io(target.display(), &error))?;
    Ok(json!({ "created": true }))
}

pub(super) fn list(call: &Call<'_>, args: List) -> Result<Value, Failure> {
    let prefix = call.file_prefix(&args.root)?;
    let dir = call.root_dir(&args.root)?;
    let mut entries = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|error| Failure::io(dir.display(), &error))? {
        let entry = entry.map_err(|error| Failure::io(dir.display(), &error))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(prefix) {
            continue;
        }
        let meta = fs::symlink_metadata(entry.path())
            .map_err(|error| Failure::io(entry.path().display(), &error))?;
        entries.push(json!({
            "name": name,
            "kind": kind(&meta),
            "size": meta.len(),
            "mode": format!("{:04o}", meta.mode() & 0o7777),
            "uid": meta.uid(),
            "gid": meta.gid(),
        }));
    }
    entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(json!({ "entries": entries }))
}

fn kind(meta: &fs::Metadata) -> &'static str {
    let file_type = meta.file_type();
    if file_type.is_symlink() {
        "symlink"
    } else if file_type.is_dir() {
        "dir"
    } else if file_type.is_file() {
        "file"
    } else {
        "other"
    }
}
