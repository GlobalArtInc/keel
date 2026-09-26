use std::ffi::OsString;
use std::fs::{self, OpenOptions, Permissions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use nix::fcntl::OFlag;
use serde::Serialize;

pub(crate) const SUBCOMMAND: &str = "agent-home";
pub(crate) const HOME_FLAG: &str = "--home";
pub(crate) const LIST: &str = "list";
pub(crate) const MKDIR: &str = "mkdir";
pub(crate) const RENAME: &str = "rename";
pub(crate) const REMOVE: &str = "remove";
pub(crate) const CHMOD: &str = "chmod";
pub(crate) const READ: &str = "read";
pub(crate) const WRITE: &str = "write";

pub(crate) const EXIT_IO: u8 = 1;
pub(crate) const EXIT_USAGE: u8 = 2;
pub(crate) const EXIT_NOT_FOUND: u8 = 3;
pub(crate) const EXIT_EXISTS: u8 = 4;
pub(crate) const EXIT_OUTSIDE: u8 = 5;
pub(crate) const EXIT_KIND: u8 = 6;

const LIST_LIMIT: usize = 5000;
const DIR_MODE: u32 = 0o755;
const FILE_MODE: u32 = 0o644;
const MODE_LIMIT: u32 = 0o777;
const TEMPORARY_SUFFIX: &str = ".keel-upload";

#[derive(Debug, thiserror::Error)]
enum HomeError {
    #[error("{0}")]
    Usage(String),
    #[error("{} does not exist", .0.display())]
    NotFound(PathBuf),
    #[error("{} already exists", .0.display())]
    Exists(PathBuf),
    #[error("{} is outside the home {}", path.display(), home.display())]
    Outside { path: PathBuf, home: PathBuf },
    #[error("{} is not {expected}", path.display())]
    Kind {
        path: PathBuf,
        expected: &'static str,
    },
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl HomeError {
    fn code(&self) -> u8 {
        match self {
            Self::Usage(_) => EXIT_USAGE,
            Self::NotFound(_) => EXIT_NOT_FOUND,
            Self::Exists(_) => EXIT_EXISTS,
            Self::Outside { .. } => EXIT_OUTSIDE,
            Self::Kind { .. } => EXIT_KIND,
            Self::Io { .. } => EXIT_IO,
        }
    }
}

#[derive(Serialize)]
struct Entry {
    name: String,
    kind: &'static str,
    size: u64,
    mode: String,
    modified: i64,
}

#[derive(Serialize)]
struct Listing {
    entries: Vec<Entry>,
    truncated: bool,
}

pub fn serve(args: &[OsString]) -> u8 {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let done = run(args, &mut input, &mut output).and_then(|()| {
        output
            .flush()
            .map_err(|source| io_error(Path::new("stdout"), source))
    });
    match done {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            error.code()
        }
    }
}

fn run(args: &[OsString], input: &mut dyn Read, output: &mut dyn Write) -> Result<(), HomeError> {
    let args = args
        .iter()
        .map(|arg| {
            arg.to_str()
                .ok_or_else(|| HomeError::Usage(format!("{} is not UTF-8", arg.to_string_lossy())))
        })
        .collect::<Result<Vec<&str>, HomeError>>()?;
    let [flag, home, op, operands @ ..] = args.as_slice() else {
        return Err(HomeError::Usage(format!(
            "{SUBCOMMAND} takes {HOME_FLAG} <home> <operation> <operands>"
        )));
    };
    if *flag != HOME_FLAG || !Path::new(home).is_absolute() {
        return Err(HomeError::Usage(format!(
            "{SUBCOMMAND} takes {HOME_FLAG} and an absolute home first"
        )));
    }
    let home = Path::new(home)
        .canonicalize()
        .map_err(|source| io_error(Path::new(home), source))?;
    match (*op, operands) {
        (LIST, [dir]) => list(&home, dir, output),
        (MKDIR, [path]) => mkdir(&home, path),
        (RENAME, [from, to]) => rename(&home, from, to),
        (REMOVE, [path]) => remove(&home, path),
        (CHMOD, [path, mode]) => chmod(&home, path, mode),
        (READ, [path]) => read(&home, path, output),
        (WRITE, [path]) => write(&home, path, input),
        _ => Err(HomeError::Usage(format!(
            "{op} with {} operands is not an operation of {SUBCOMMAND}",
            operands.len()
        ))),
    }
}

fn io_error(path: &Path, source: io::Error) -> HomeError {
    match source.kind() {
        io::ErrorKind::NotFound => HomeError::NotFound(path.to_owned()),
        io::ErrorKind::AlreadyExists => HomeError::Exists(path.to_owned()),
        _ => HomeError::Io {
            path: path.to_owned(),
            source,
        },
    }
}

fn outside(home: &Path, path: &Path) -> HomeError {
    HomeError::Outside {
        path: path.to_owned(),
        home: home.to_owned(),
    }
}

fn plain(home: &Path, path: &Path) -> Result<(), HomeError> {
    let plain = path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)));
    if plain {
        Ok(())
    } else {
        Err(outside(home, path))
    }
}

fn entry(home: &Path, path: &str) -> Result<PathBuf, HomeError> {
    let path = Path::new(path);
    plain(home, path)?;
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(outside(home, path));
    };
    let parent = parent
        .canonicalize()
        .map_err(|source| io_error(parent, source))?;
    if parent.starts_with(home) {
        Ok(parent.join(name))
    } else {
        Err(outside(home, path))
    }
}

fn directory(home: &Path, path: &str) -> Result<PathBuf, HomeError> {
    let path = Path::new(path);
    plain(home, path)?;
    let dir = path
        .canonicalize()
        .map_err(|source| io_error(path, source))?;
    if dir.starts_with(home) {
        Ok(dir)
    } else {
        Err(outside(home, path))
    }
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

fn list(home: &Path, dir: &str, output: &mut dyn Write) -> Result<(), HomeError> {
    let dir = directory(home, dir)?;
    let mut entries = Vec::new();
    let mut truncated = false;
    let read = fs::read_dir(&dir).map_err(|source| io_error(&dir, source))?;
    for found in read {
        let found = found.map_err(|source| io_error(&dir, source))?;
        let Some(name) = found.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if entries.len() == LIST_LIMIT {
            truncated = true;
            break;
        }
        let meta = match fs::symlink_metadata(found.path()) {
            Ok(meta) => meta,
            Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => return Err(io_error(&found.path(), source)),
        };
        entries.push(Entry {
            name,
            kind: kind(&meta),
            size: meta.len(),
            mode: format!("{:04o}", meta.mode() & 0o7777),
            modified: meta.mtime(),
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    serde_json::to_writer(&mut *output, &Listing { entries, truncated }).map_err(|error| {
        HomeError::Io {
            path: dir,
            source: io::Error::other(error),
        }
    })
}

fn mkdir(home: &Path, path: &str) -> Result<(), HomeError> {
    let path = entry(home, path)?;
    fs::DirBuilder::new()
        .mode(DIR_MODE)
        .create(&path)
        .map_err(|source| io_error(&path, source))
}

fn rename(home: &Path, from: &str, to: &str) -> Result<(), HomeError> {
    let from = entry(home, from)?;
    let to = entry(home, to)?;
    fs::symlink_metadata(&from).map_err(|source| io_error(&from, source))?;
    if fs::symlink_metadata(&to).is_ok() {
        return Err(HomeError::Exists(to));
    }
    fs::rename(&from, &to).map_err(|source| io_error(&from, source))
}

fn remove(home: &Path, path: &str) -> Result<(), HomeError> {
    let path = entry(home, path)?;
    let meta = fs::symlink_metadata(&path).map_err(|source| io_error(&path, source))?;
    if meta.is_dir() {
        fs::remove_dir_all(&path)
    } else {
        fs::remove_file(&path)
    }
    .map_err(|source| io_error(&path, source))
}

pub(crate) fn parse_mode(text: &str) -> Option<u32> {
    if text.is_empty() || text.len() > 4 || !text.bytes().all(|byte| (b'0'..=b'7').contains(&byte))
    {
        return None;
    }
    u32::from_str_radix(text, 8)
        .ok()
        .filter(|mode| *mode <= MODE_LIMIT)
}

fn chmod(home: &Path, path: &str, mode: &str) -> Result<(), HomeError> {
    let mode = parse_mode(mode).ok_or_else(|| {
        HomeError::Usage(format!(
            "the mode {mode} is not octal within {MODE_LIMIT:04o}"
        ))
    })?;
    let path = entry(home, path)?;
    let meta = fs::symlink_metadata(&path).map_err(|source| io_error(&path, source))?;
    if meta.file_type().is_symlink() {
        return Err(HomeError::Kind {
            path,
            expected: "a file or a directory",
        });
    }
    fs::set_permissions(&path, Permissions::from_mode(mode))
        .map_err(|source| io_error(&path, source))
}

fn read(home: &Path, path: &str, output: &mut dyn Write) -> Result<(), HomeError> {
    let path = entry(home, path)?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(OFlag::O_NOFOLLOW.bits())
        .open(&path)
        .map_err(|source| match source.raw_os_error() {
            Some(code) if code == nix::errno::Errno::ELOOP as i32 => HomeError::Kind {
                path: path.clone(),
                expected: "a regular file",
            },
            _ => io_error(&path, source),
        })?;
    let meta = file.metadata().map_err(|source| io_error(&path, source))?;
    if !meta.is_file() {
        return Err(HomeError::Kind {
            path,
            expected: "a regular file",
        });
    }
    io::copy(&mut file, output).map_err(|source| io_error(&path, source))?;
    Ok(())
}

fn write(home: &Path, path: &str, input: &mut dyn Read) -> Result<(), HomeError> {
    let path = entry(home, path)?;
    if fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_dir()) {
        return Err(HomeError::Kind {
            path,
            expected: "a file",
        });
    }
    let Some(name) = path.file_name() else {
        return Err(outside(home, &path));
    };
    let mut temporary_name = OsString::from(".");
    temporary_name.push(name);
    temporary_name.push(TEMPORARY_SUFFIX);
    let temporary = path.with_file_name(temporary_name);
    match fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(source) if source.kind() == io::ErrorKind::NotFound => {}
        Err(source) => return Err(io_error(&temporary, source)),
    }
    let placed = place(&temporary, &path, input);
    if placed.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    placed
}

fn place(temporary: &Path, path: &Path, input: &mut dyn Read) -> Result<(), HomeError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(FILE_MODE)
        .custom_flags(OFlag::O_NOFOLLOW.bits())
        .open(temporary)
        .map_err(|source| io_error(temporary, source))?;
    io::copy(input, &mut file).map_err(|source| io_error(temporary, source))?;
    file.sync_all()
        .map_err(|source| io_error(temporary, source))?;
    fs::rename(temporary, path).map_err(|source| io_error(path, source))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use serde_json::Value;

    use super::*;

    struct Home {
        dir: tempfile::TempDir,
    }

    impl Home {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            fs::create_dir(dir.path().join("home")).unwrap();
            Self { dir }
        }

        fn home(&self) -> PathBuf {
            self.dir.path().join("home").canonicalize().unwrap()
        }

        fn at(&self, name: &str) -> String {
            self.home().join(name).display().to_string()
        }

        fn run(&self, op: &str, operands: &[&str], input: &[u8]) -> Result<Vec<u8>, HomeError> {
            let mut args = vec![
                OsString::from(HOME_FLAG),
                self.home().into_os_string(),
                OsString::from(op),
            ];
            args.extend(operands.iter().map(OsString::from));
            let mut output = Vec::new();
            run(&args, &mut &input[..], &mut output).map(|()| output)
        }

        fn names(&self, dir: &str) -> Vec<(String, String)> {
            let listed: Value =
                serde_json::from_slice(&self.run(LIST, &[&self.at(dir)], b"").unwrap()).unwrap();
            assert_eq!(listed["truncated"], false);
            listed["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| {
                    (
                        entry["name"].as_str().unwrap().to_owned(),
                        entry["kind"].as_str().unwrap().to_owned(),
                    )
                })
                .collect()
        }
    }

    #[test]
    fn the_worker_lists_creates_renames_changes_and_removes_inside_the_home() {
        let home = Home::new();
        home.run(MKDIR, &[&home.at("site")], b"").unwrap();
        home.run(WRITE, &[&home.at("site/index.html")], b"<p>hello</p>")
            .unwrap();
        assert_eq!(
            home.names(""),
            [("site".to_owned(), "dir".to_owned())],
            "the listing names the directory the worker made"
        );
        assert_eq!(
            home.names("site"),
            [("index.html".to_owned(), "file".to_owned())]
        );
        home.run(
            RENAME,
            &[&home.at("site/index.html"), &home.at("site/main.html")],
            b"",
        )
        .unwrap();
        home.run(CHMOD, &[&home.at("site/main.html"), "600"], b"")
            .unwrap();
        let mode = fs::metadata(home.home().join("site/main.html"))
            .unwrap()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(
            home.run(READ, &[&home.at("site/main.html")], b"").unwrap(),
            b"<p>hello</p>"
        );
        home.run(WRITE, &[&home.at("site/other.html")], b"x")
            .unwrap();
        let taken = home
            .run(
                RENAME,
                &[&home.at("site/other.html"), &home.at("site/main.html")],
                b"",
            )
            .unwrap_err();
        assert_eq!(taken.code(), EXIT_EXISTS, "{taken}");
        home.run(REMOVE, &[&home.at("site")], b"").unwrap();
        assert!(home.names("").is_empty());
        let gone = home.run(REMOVE, &[&home.at("site")], b"").unwrap_err();
        assert_eq!(gone.code(), EXIT_NOT_FOUND, "{gone}");
        let mode = home
            .run(CHMOD, &[&home.at("site"), "4755"], b"")
            .unwrap_err();
        assert_eq!(mode.code(), EXIT_USAGE, "{mode}");
    }

    #[test]
    fn a_path_outside_the_home_or_through_a_symlink_leaving_it_is_refused() {
        let home = Home::new();
        let beside = home.dir.path().join("beside");
        fs::create_dir(&beside).unwrap();
        fs::write(beside.join("secret"), "outside").unwrap();
        symlink(&beside, home.home().join("escape")).unwrap();
        for (op, operands) in [
            (LIST, vec![home.at("escape")]),
            (READ, vec![home.at("escape/secret")]),
            (WRITE, vec![home.at("escape/planted")]),
            (MKDIR, vec![home.at("../beside/made")]),
            (REMOVE, vec![home.home().display().to_string()]),
            (REMOVE, vec![beside.join("secret").display().to_string()]),
            (LIST, vec!["relative".to_owned()]),
        ] {
            let operands: Vec<&str> = operands.iter().map(String::as_str).collect();
            let refused = home.run(op, &operands, b"planted").unwrap_err();
            assert_eq!(refused.code(), EXIT_OUTSIDE, "{op} {operands:?}: {refused}");
        }
        assert_eq!(
            fs::read_to_string(beside.join("secret")).unwrap(),
            "outside"
        );
        assert!(!beside.join("planted").exists() && !beside.join("made").exists());
        let link = home.run(READ, &[&home.at("escape")], b"").unwrap_err();
        assert_eq!(link.code(), EXIT_KIND, "{link}");
        home.run(REMOVE, &[&home.at("escape")], b"").unwrap();
        assert!(
            beside.join("secret").exists(),
            "removing a link leaves its target"
        );
    }

    #[test]
    fn a_written_file_replaces_the_previous_one_through_a_temporary_beside_it() {
        let home = Home::new();
        home.run(WRITE, &[&home.at("notes.txt")], b"first").unwrap();
        home.run(WRITE, &[&home.at("notes.txt")], b"second")
            .unwrap();
        assert_eq!(
            fs::read_to_string(home.home().join("notes.txt")).unwrap(),
            "second"
        );
        assert_eq!(
            home.names(""),
            [("notes.txt".to_owned(), "file".to_owned())]
        );
        let mode = fs::metadata(home.home().join("notes.txt")).unwrap().mode();
        assert_eq!(mode & 0o022, 0, "an upload is not writable by others");
        home.run(MKDIR, &[&home.at("dir")], b"").unwrap();
        let onto = home.run(WRITE, &[&home.at("dir")], b"x").unwrap_err();
        assert_eq!(onto.code(), EXIT_KIND, "{onto}");
    }

    #[test]
    fn a_call_without_the_home_or_with_wrong_operands_is_a_usage_error() {
        let home = Home::new();
        let mut output = Vec::new();
        for args in [
            vec![],
            vec!["--root", "/tmp", LIST, "/tmp"],
            vec![HOME_FLAG, "relative", LIST, "/tmp"],
        ] {
            let args: Vec<OsString> = args.into_iter().map(OsString::from).collect();
            let refused = run(&args, &mut &b""[..], &mut output).unwrap_err();
            assert_eq!(refused.code(), EXIT_USAGE, "{refused}");
        }
        let refused = home.run(RENAME, &[&home.at("a")], b"").unwrap_err();
        assert_eq!(refused.code(), EXIT_USAGE, "{refused}");
        let refused = home.run("chown", &[&home.at("a")], b"").unwrap_err();
        assert_eq!(refused.code(), EXIT_USAGE, "{refused}");
    }
}
