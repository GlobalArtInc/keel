use std::ffi::{OsStr, OsString};
use std::fs::{File, Metadata, Permissions};
use std::io::{self, Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt, fchown};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use rustix::fs::{self as at, AtFlags, Dir, FileType, Gid, Mode, OFlags, Uid};
use rustix::io::Errno;
use tar::{Builder, Entry, EntryType, Header};

use crate::seal::Hashing;

const PERMISSIONS: u32 = 0o777;
const RESTORED: u32 = 0o775;
const LINK_MODE: u32 = 0o777;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Placement {
    uid: u32,
    gid: u32,
    mode: u32,
}

impl Placement {
    pub(crate) fn of(dir: &OwnedFd, mode: u32) -> io::Result<Self> {
        let stat = at::fstat(dir)?;
        Ok(Self {
            uid: stat.st_uid,
            gid: stat.st_gid,
            mode,
        })
    }
}

fn directory() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
}

fn reading() -> OFlags {
    OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC
}

fn creating() -> OFlags {
    OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC
}

pub(crate) fn owner(dir: &OwnedFd) -> io::Result<u32> {
    Ok(at::fstat(dir)?.st_uid)
}

pub(crate) fn open_dir(path: &Path) -> io::Result<OwnedFd> {
    Ok(at::open(path, directory(), Mode::empty())?)
}

pub(crate) fn open_file(path: &Path) -> io::Result<File> {
    Ok(File::from(at::open(path, reading(), Mode::empty())?))
}

pub(crate) fn create_file(path: &Path, owner: u32) -> io::Result<File> {
    let file = create_private(path)?;
    if file.metadata()?.uid() != owner {
        fchown(&file, Some(owner), None)?;
    }
    Ok(file)
}

pub(crate) fn create_private(path: &Path) -> io::Result<File> {
    Ok(File::from(at::open(
        path,
        creating(),
        Mode::RUSR | Mode::WUSR,
    )?))
}

pub(crate) fn scratch(path: &Path) -> io::Result<File> {
    let file = File::from(at::open(
        path,
        OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )?);
    std::fs::remove_file(path)?;
    Ok(file)
}

pub(crate) fn descend(
    base: &OwnedFd,
    relative: &Path,
    create: Option<Placement>,
) -> io::Result<Option<OwnedFd>> {
    let mut current = base.try_clone()?;
    for name in names(relative)? {
        current = match at::openat(&current, name, directory(), Mode::empty()) {
            Ok(next) => next,
            Err(Errno::NOENT) => {
                let Some(placement) = create else {
                    return Ok(None);
                };
                at::mkdirat(&current, name, Mode::RWXU)?;
                let next = File::from(at::openat(&current, name, directory(), Mode::empty())?);
                settle(&next, placement.uid, placement.gid, placement.mode)?;
                OwnedFd::from(next)
            }
            Err(error) => return Err(error.into()),
        };
    }
    Ok(Some(current))
}

pub(crate) fn names(relative: &Path) -> io::Result<Vec<&OsStr>> {
    relative
        .components()
        .map(|component| match component {
            Component::Normal(name) => Ok(name),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a plain relative path", relative.display()),
            )),
        })
        .collect()
}

pub(crate) fn pack<W: Write>(
    builder: &mut Builder<W>,
    dir: &OwnedFd,
    prefix: &Path,
    owner: u32,
) -> io::Result<u64> {
    let mut packed = 0;
    for name in listing(dir)? {
        let path = prefix.join(&name);
        let Some(stat) = lookup(dir, &name)? else {
            continue;
        };
        match FileType::from_raw_mode(stat.st_mode) {
            FileType::Directory => {
                let Some(child) = opened(at::openat(dir, &name, directory(), Mode::empty()))?
                else {
                    continue;
                };
                let child = File::from(child);
                let meta = child.metadata()?;
                let mut header = header(EntryType::Directory, &meta, 0);
                builder.append_data(&mut header, &path, io::empty())?;
                packed += 1 + pack(builder, &OwnedFd::from(child), &path, owner)?;
            }
            FileType::RegularFile => {
                let Some(file) = opened(at::openat(dir, &name, reading(), Mode::empty()))? else {
                    continue;
                };
                let file = File::from(file);
                let meta = file.metadata()?;
                if !meta.is_file() || meta.uid() != owner {
                    continue;
                }
                let size = meta.len();
                let mut header = header(EntryType::Regular, &meta, size);
                let exact = file.take(size).chain(io::repeat(0)).take(size);
                builder.append_data(&mut header, &path, exact)?;
                packed += 1;
            }
            FileType::Symlink => {
                let Some(target) = opened(at::readlinkat(dir, &name, Vec::new()))? else {
                    continue;
                };
                let target = PathBuf::from(OsStr::from_bytes(target.as_bytes()));
                let mut header = Header::new_gnu();
                header.set_entry_type(EntryType::Symlink);
                header.set_mode(LINK_MODE);
                header.set_mtime(u64::try_from(stat.st_mtime).unwrap_or(0));
                header.set_uid(0);
                header.set_gid(0);
                header.set_size(0);
                builder.append_link(&mut header, &path, &target)?;
                packed += 1;
            }
            _ => {}
        }
    }
    Ok(packed)
}

pub(crate) fn append_file<W: Write>(
    builder: &mut Builder<W>,
    path: &Path,
    file: File,
) -> io::Result<String> {
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not a regular file", path.display()),
        ));
    }
    let size = meta.len();
    let mut header = header(EntryType::Regular, &meta, size);
    let mut hashing = Hashing::new(file.take(size));
    let exact = (&mut hashing).chain(io::repeat(0)).take(size);
    builder.append_data(&mut header, path, exact)?;
    Ok(hashing.finish().1)
}

pub(crate) fn append_bytes<W: Write>(
    builder: &mut Builder<W>,
    path: &Path,
    bytes: &[u8],
    mtime: u64,
) -> io::Result<()> {
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_mode(0o644);
    header.set_mtime(mtime);
    header.set_uid(0);
    header.set_gid(0);
    header.set_size(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
    builder.append_data(&mut header, path, bytes)
}

pub(crate) fn unpack<R: Read>(
    entry: &mut Entry<'_, R>,
    base: &OwnedFd,
    relative: &Path,
    placement: Placement,
) -> io::Result<bool> {
    let parts = names(relative)?;
    let Some((last, parents)) = parts.split_last() else {
        return Ok(false);
    };
    let parents: PathBuf = parents.iter().collect();
    let Some(parent) = descend(base, &parents, Some(placement))? else {
        return Ok(false);
    };
    let kind = entry.header().entry_type();
    let mode = entry.header().mode()? & RESTORED;
    let mtime = entry.header().mtime()?;
    match kind {
        EntryType::Directory => {
            match lookup(&parent, last)?.map(|stat| FileType::from_raw_mode(stat.st_mode)) {
                Some(FileType::Directory) => {}
                Some(_) => {
                    at::unlinkat(&parent, *last, AtFlags::empty())?;
                    at::mkdirat(&parent, *last, Mode::RWXU)?;
                }
                None => at::mkdirat(&parent, *last, Mode::RWXU)?,
            }
            let dir = File::from(at::openat(&parent, *last, directory(), Mode::empty())?);
            settle(&dir, placement.uid, placement.gid, mode)?;
        }
        EntryType::Regular => {
            clear(&parent, last)?;
            let mut file = File::from(at::openat(
                &parent,
                *last,
                creating(),
                Mode::RUSR | Mode::WUSR,
            )?);
            settle(&file, placement.uid, placement.gid, 0o600)?;
            io::copy(entry, &mut file)?;
            file.set_permissions(Permissions::from_mode(mode))?;
            file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(mtime))?;
        }
        EntryType::Symlink => {
            let Some(target) = entry.link_name()? else {
                return Ok(false);
            };
            clear(&parent, last)?;
            at::symlinkat(target.as_ref(), &parent, *last)?;
            let stat = at::statat(&parent, *last, AtFlags::SYMLINK_NOFOLLOW)?;
            if stat.st_uid != placement.uid || stat.st_gid != placement.gid {
                at::chownat(
                    &parent,
                    *last,
                    Some(Uid::from_raw(placement.uid)),
                    Some(Gid::from_raw(placement.gid)),
                    AtFlags::SYMLINK_NOFOLLOW,
                )?;
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn settle(file: &File, uid: u32, gid: u32, mode: u32) -> io::Result<()> {
    let meta = file.metadata()?;
    if meta.uid() != uid || meta.gid() != gid {
        fchown(file, Some(uid), Some(gid))?;
    }
    file.set_permissions(Permissions::from_mode(mode))
}

fn clear(parent: &OwnedFd, name: &OsStr) -> io::Result<()> {
    match lookup(parent, name)?.map(|stat| FileType::from_raw_mode(stat.st_mode)) {
        None => Ok(()),
        Some(FileType::Directory) => Ok(at::unlinkat(parent, name, AtFlags::REMOVEDIR)?),
        Some(_) => Ok(at::unlinkat(parent, name, AtFlags::empty())?),
    }
}

fn lookup(dir: &OwnedFd, name: &OsStr) -> io::Result<Option<at::Stat>> {
    match at::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => Ok(Some(stat)),
        Err(Errno::NOENT) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn opened<T>(result: Result<T, Errno>) -> io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(Errno::NOENT | Errno::LOOP | Errno::NOTDIR) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn listing(dir: &OwnedFd) -> io::Result<Vec<OsString>> {
    let mut names = Vec::new();
    for entry in Dir::read_from(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_bytes();
        if name != b"." && name != b".." {
            names.push(OsStr::from_bytes(name).to_owned());
        }
    }
    names.sort();
    Ok(names)
}

fn header(kind: EntryType, meta: &Metadata, size: u64) -> Header {
    let mut header = Header::new_gnu();
    header.set_entry_type(kind);
    header.set_mode(meta.mode() & PERMISSIONS);
    header.set_mtime(u64::try_from(meta.mtime()).unwrap_or(0));
    header.set_uid(0);
    header.set_gid(0);
    header.set_size(size);
    header
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;

    use tar::Archive;

    use super::*;

    fn uid(path: &Path) -> u32 {
        fs::metadata(path).unwrap().uid()
    }

    fn packed(root: &Path, owner: u32) -> Vec<(String, EntryType, Vec<u8>)> {
        let dir = open_dir(root).unwrap();
        let mut builder = Builder::new(Vec::new());
        pack(&mut builder, &dir, Path::new("files"), owner).unwrap();
        let bytes = builder.into_inner().unwrap();
        let mut archive = Archive::new(bytes.as_slice());
        archive
            .entries()
            .unwrap()
            .map(|entry| {
                let mut entry = entry.unwrap();
                let path = entry.path().unwrap().display().to_string();
                let kind = entry.header().entry_type();
                let mut content = Vec::new();
                if kind == EntryType::Symlink {
                    content = entry
                        .link_name_bytes()
                        .map(|link| link.into_owned())
                        .unwrap_or_default();
                } else {
                    entry.read_to_end(&mut content).unwrap();
                }
                (path, kind, content)
            })
            .collect()
    }

    #[test]
    fn a_tree_is_packed_in_order_with_its_links_kept_as_links_and_never_followed() {
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("shadow"), "secret").unwrap();
        let site = tempfile::tempdir().unwrap();
        fs::create_dir(site.path().join("css")).unwrap();
        fs::write(site.path().join("css/site.css"), "body {}").unwrap();
        fs::write(site.path().join("index.html"), "<h1>a</h1>").unwrap();
        symlink(outside.path(), site.path().join("escape")).unwrap();
        symlink(outside.path().join("shadow"), site.path().join("shadow")).unwrap();
        let entries = packed(site.path(), uid(site.path()));
        let listed: Vec<(&str, EntryType)> = entries
            .iter()
            .map(|(path, kind, _)| (path.as_str(), *kind))
            .collect();
        assert_eq!(
            listed,
            [
                ("files/css", EntryType::Directory),
                ("files/css/site.css", EntryType::Regular),
                ("files/escape", EntryType::Symlink),
                ("files/index.html", EntryType::Regular),
                ("files/shadow", EntryType::Symlink),
            ]
        );
        assert_eq!(entries[1].2, b"body {}");
        assert_eq!(entries[3].2, b"<h1>a</h1>");
        assert!(
            entries
                .iter()
                .all(|(_, _, content)| content.as_slice() != b"secret")
        );
    }

    #[test]
    fn an_unpacked_tree_replaces_files_restores_links_and_refuses_to_climb() {
        let site = tempfile::tempdir().unwrap();
        fs::create_dir(site.path().join("css")).unwrap();
        fs::write(site.path().join("css/site.css"), "body {}").unwrap();
        fs::write(site.path().join("index.html"), "<h1>a</h1>").unwrap();
        symlink("index.html", site.path().join("home.html")).unwrap();
        let dir = open_dir(site.path()).unwrap();
        let mut builder = Builder::new(Vec::new());
        pack(&mut builder, &dir, Path::new("files"), uid(site.path())).unwrap();
        let bytes = builder.into_inner().unwrap();

        fs::write(site.path().join("index.html"), "<h1>b</h1>").unwrap();
        fs::remove_dir_all(site.path().join("css")).unwrap();
        fs::remove_file(site.path().join("home.html")).unwrap();
        fs::write(site.path().join("home.html"), "not a link").unwrap();
        let placement = Placement::of(&dir, 0o755).unwrap();
        let mut archive = Archive::new(bytes.as_slice());
        let mut restored = 0;
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            let path = entry.path().unwrap().into_owned();
            let relative = path.strip_prefix("files").unwrap().to_owned();
            if unpack(&mut entry, &dir, &relative, placement).unwrap() {
                restored += 1;
            }
        }
        assert_eq!(restored, 4);
        assert_eq!(
            fs::read_to_string(site.path().join("index.html")).unwrap(),
            "<h1>a</h1>"
        );
        assert_eq!(
            fs::read_to_string(site.path().join("css/site.css")).unwrap(),
            "body {}"
        );
        assert_eq!(
            fs::read_link(site.path().join("home.html")).unwrap(),
            Path::new("index.html")
        );

        for climbing in ["../outside", "/etc/passwd", "./a"] {
            assert!(names(Path::new(climbing)).is_err(), "{climbing}");
        }
        assert!(
            descend(&dir, Path::new("home.html"), None).is_err(),
            "a link is never descended"
        );
        assert!(descend(&dir, Path::new("absent"), None).unwrap().is_none());
    }

    #[test]
    fn a_file_of_another_owner_is_never_packed_whatever_its_links() {
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("shadow"), "secret").unwrap();
        let site = tempfile::tempdir().unwrap();
        fs::write(site.path().join("index.html"), "<h1>a</h1>").unwrap();
        fs::hard_link(outside.path().join("shadow"), site.path().join("linked")).unwrap();
        let own = uid(site.path());

        let foreign = packed(site.path(), own.wrapping_add(1));
        assert!(foreign.is_empty(), "{foreign:?}");

        fs::remove_file(outside.path().join("shadow")).unwrap();
        let alone = packed(site.path(), own.wrapping_add(1));
        assert!(
            alone.iter().all(|(path, _, content)| path != "files/linked"
                && content.as_slice() != b"secret"),
            "{alone:?}"
        );

        let owned = packed(site.path(), own);
        assert!(owned.contains(&(
            "files/linked".to_owned(),
            EntryType::Regular,
            b"secret".to_vec()
        )));
        assert!(owned.iter().any(|(path, _, _)| path == "files/index.html"));
    }

    #[test]
    fn an_unpacked_entry_never_carries_the_other_write_bit() {
        let site = tempfile::tempdir().unwrap();
        fs::create_dir(site.path().join("shared")).unwrap();
        fs::write(site.path().join("shared/open.txt"), "open").unwrap();
        fs::set_permissions(
            site.path().join("shared/open.txt"),
            Permissions::from_mode(0o666),
        )
        .unwrap();
        fs::set_permissions(site.path().join("shared"), Permissions::from_mode(0o777)).unwrap();
        let dir = open_dir(site.path()).unwrap();
        let mut builder = Builder::new(Vec::new());
        pack(&mut builder, &dir, Path::new("files"), uid(site.path())).unwrap();
        let bytes = builder.into_inner().unwrap();

        let target = tempfile::tempdir().unwrap();
        let base = open_dir(target.path()).unwrap();
        let placement = Placement::of(&base, 0o755).unwrap();
        let mut archive = Archive::new(bytes.as_slice());
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            let path = entry.path().unwrap().into_owned();
            let relative = path.strip_prefix("files").unwrap().to_owned();
            assert!(unpack(&mut entry, &base, &relative, placement).unwrap());
        }
        let mode = |path: &str| fs::metadata(target.path().join(path)).unwrap().mode() & 0o7777;
        assert_eq!(mode("shared"), 0o775);
        assert_eq!(mode("shared/open.txt"), 0o664);
    }
}
