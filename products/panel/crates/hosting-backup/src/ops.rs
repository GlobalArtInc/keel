use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use keel::prelude::*;
use keel::sdk::serde_json;
use tar::{Archive, Builder};

use crate::seal::{self, Hashing};
use crate::tree::{self, Placement};

pub(crate) const PACK: &str = "hosting.backup.pack";
pub(crate) const UNPACK: &str = "hosting.backup.unpack";

const CLASS: &str = "backup";
const ARCHIVES: &str = "archives";
const HOMES: &str = "homes";
const MAILDIRS: &str = "maildirs";
const DUMPS: &str = "dumps";
const FILES_DIR: &str = "files";
const DATABASES_DIR: &str = "databases";
const MAIL_DIR: &str = "mail";
const MANIFEST: &str = "manifest.json";
const DUMP_SUFFIX: &str = ".sql";
const LEVEL: i32 = 3;
const DOCUMENT_MODE: u32 = 0o755;
const MAILDIR_MODE: u32 = 0o700;
const NAME_MAX: usize = 255;
const ERROR_POLICY: &str = "host.error.policy";
const ERROR_ARGS: &str = "host.error.args";
const ERROR_IO: &str = "host.error.io";
const ERROR_NOT_FOUND: &str = "host.error.not_found";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", deny_unknown_fields)]
#[schemars(crate = "keel::sdk::schemars")]
pub(crate) struct DatabaseDump {
    pub(crate) database: String,
    pub(crate) dump: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", deny_unknown_fields)]
#[schemars(crate = "keel::sdk::schemars")]
pub(crate) struct MailboxDir {
    pub(crate) address: String,
    pub(crate) maildir: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", deny_unknown_fields)]
#[schemars(crate = "keel::sdk::schemars")]
pub(crate) struct SiteArchive {
    pub(crate) archive: String,
    pub(crate) domain: String,
    pub(crate) home: String,
    pub(crate) root: String,
    #[serde(default)]
    pub(crate) databases: Vec<DatabaseDump>,
    #[serde(default)]
    pub(crate) mailboxes: Vec<MailboxDir>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
pub(crate) struct Contents {
    pub(crate) bytes: u64,
    pub(crate) files: u64,
    pub(crate) databases: Vec<String>,
    pub(crate) mailboxes: Vec<String>,
}

#[derive(Serialize)]
#[serde(crate = "keel::sdk::serde")]
struct Manifest<'a> {
    domain: &'a str,
    databases: Vec<&'a str>,
    mailboxes: Vec<&'a str>,
}

struct Resolved {
    archive: PathBuf,
    home: PathBuf,
    root: PathBuf,
    dumps: Vec<(String, PathBuf)>,
    maildirs: Vec<(String, PathBuf)>,
    owner: u32,
}

pub(crate) fn pack_op() -> AgentOp {
    AgentOp::new::<SiteArchive, Contents>(PACK, pack).policy(CLASS)
}

pub(crate) fn unpack_op() -> AgentOp {
    AgentOp::new::<SiteArchive, Contents>(UNPACK, unpack).policy(CLASS)
}

fn pack(scope: &dyn AgentScope, args: SiteArchive) -> Result<Contents, AgentOpError> {
    let resolved = resolve(scope, &args)?;
    let temporary = resolved
        .archive
        .with_file_name(format!(".{}.tmp", args.archive));
    discard(&temporary).map_err(|error| failed(&temporary, &error))?;
    let file = tree::create_file(&temporary, resolved.owner)
        .map_err(|error| failed(&temporary, &error))?;
    let written = write(file, &args, &resolved).and_then(|(contents, digest)| {
        seal::record(&resolved.archive, &digest)?;
        std::fs::rename(&temporary, &resolved.archive)?;
        Ok(contents)
    });
    match written {
        Ok(mut contents) => {
            contents.bytes = std::fs::symlink_metadata(&resolved.archive)
                .map_err(|error| failed(&resolved.archive, &error))?
                .len();
            Ok(contents)
        }
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            let _ = seal::remove(&resolved.archive);
            Err(failed(&resolved.archive, &error))
        }
    }
}

fn write(file: File, args: &SiteArchive, resolved: &Resolved) -> io::Result<(Contents, String)> {
    let mut builder = Builder::new(zstd::Encoder::new(Hashing::new(file), LEVEL)?);
    let manifest = Manifest {
        domain: &args.domain,
        databases: resolved
            .dumps
            .iter()
            .map(|(name, _)| name.as_str())
            .collect(),
        mailboxes: resolved
            .maildirs
            .iter()
            .map(|(address, _)| address.as_str())
            .collect(),
    };
    let manifest = serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?;
    tree::append_bytes(&mut builder, Path::new(MANIFEST), &manifest, 0)?;
    let home = tree::open_dir(&resolved.home)?;
    let files = match tree::descend(&home, &resolved.root, None)? {
        Some(root) => tree::pack(
            &mut builder,
            &root,
            Path::new(FILES_DIR),
            tree::owner(&home)?,
        )?,
        None => 0,
    };
    let mut databases = Vec::new();
    for (name, path) in &resolved.dumps {
        let entry = Path::new(DATABASES_DIR).join(format!("{name}{DUMP_SUFFIX}"));
        let sealed = seal::expected(path)?;
        let digest = tree::append_file(&mut builder, &entry, tree::open_file(path)?)?;
        sealed.confirm(path, &digest)?;
        databases.push(name.clone());
    }
    let mut mailboxes = Vec::new();
    for (address, path) in &resolved.maildirs {
        let (parent, name) = split(path)?;
        let parent = tree::open_dir(parent)?;
        let Some(maildir) = tree::descend(&parent, Path::new(name), None)? else {
            continue;
        };
        tree::pack(
            &mut builder,
            &maildir,
            &Path::new(MAIL_DIR).join(address),
            tree::owner(&parent)?,
        )?;
        mailboxes.push(address.clone());
    }
    let (file, digest) = builder.into_inner()?.finish()?.finish();
    file.sync_all()?;
    Ok((
        Contents {
            bytes: 0,
            files,
            databases,
            mailboxes,
        },
        digest,
    ))
}

fn unpack(scope: &dyn AgentScope, args: SiteArchive) -> Result<Contents, AgentOpError> {
    let resolved = resolve(scope, &args)?;
    let file = tree::open_file(&resolved.archive).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            AgentOpError::new(ERROR_NOT_FOUND, format!("{}: {error}", args.archive))
        } else {
            failed(&resolved.archive, &error)
        }
    })?;
    let copy = verified(file, &resolved.archive, &args.archive)
        .map_err(|error| failed(&resolved.archive, &error))?;
    read(copy, &resolved).map_err(|error| failed(&resolved.archive, &error))
}

fn verified(file: File, archive: &Path, name: &str) -> io::Result<File> {
    let sealed = seal::expected(archive)?;
    let scratch = archive.with_file_name(format!(".{name}.restore"));
    discard(&scratch)?;
    let mut copy = tree::scratch(&scratch)?;
    let mut reading = Hashing::new(file);
    io::copy(&mut reading, &mut copy)?;
    sealed.confirm(archive, &reading.finish().1)?;
    copy.seek(SeekFrom::Start(0))?;
    Ok(copy)
}

fn read(file: File, resolved: &Resolved) -> io::Result<Contents> {
    let home = tree::open_dir(&resolved.home)?;
    let placement = Placement::of(&home, DOCUMENT_MODE)?;
    let root = tree::descend(&home, &resolved.root, Some(placement))?
        .ok_or_else(|| io::Error::other("the document root could not be created"))?;
    let dumps: BTreeMap<&str, &Path> = resolved
        .dumps
        .iter()
        .map(|(name, path)| (name.as_str(), path.as_path()))
        .collect();
    let maildirs: BTreeMap<&str, &Path> = resolved
        .maildirs
        .iter()
        .map(|(address, path)| (address.as_str(), path.as_path()))
        .collect();
    let mut opened = BTreeMap::new();
    let mut contents = Contents {
        bytes: 0,
        files: 0,
        databases: Vec::new(),
        mailboxes: Vec::new(),
    };
    let mut archive = Archive::new(zstd::Decoder::new(file)?);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let parts = tree::names(&path)?;
        match parts.as_slice() {
            [section, rest @ ..] if *section == FILES_DIR && !rest.is_empty() => {
                let relative: PathBuf = rest.iter().collect();
                if tree::unpack(&mut entry, &root, &relative, placement)? {
                    contents.files += 1;
                }
            }
            [section, file] if *section == DATABASES_DIR => {
                let Some(name) = file
                    .to_str()
                    .and_then(|file| file.strip_suffix(DUMP_SUFFIX))
                else {
                    continue;
                };
                let Some(dump) = dumps.get(name) else {
                    continue;
                };
                discard(dump)?;
                let mut target = Hashing::new(tree::create_private(dump)?);
                io::copy(&mut entry, &mut target)?;
                let (target, digest) = target.finish();
                target.sync_all()?;
                seal::record(dump, &digest)?;
                contents.databases.push(name.to_owned());
            }
            [section, address, rest @ ..] if *section == MAIL_DIR && !rest.is_empty() => {
                let Some(address) = address.to_str() else {
                    continue;
                };
                let Some(path) = maildirs.get(address) else {
                    continue;
                };
                if !opened.contains_key(address) {
                    let (parent, name) = split(path)?;
                    let parent = tree::open_dir(parent)?;
                    let placement = Placement::of(&parent, MAILDIR_MODE)?;
                    let maildir = tree::descend(&parent, Path::new(name), Some(placement))?
                        .ok_or_else(|| io::Error::other("the maildir could not be created"))?;
                    opened.insert(address.to_owned(), (maildir, placement));
                    contents.mailboxes.push(address.to_owned());
                }
                if let Some((maildir, placement)) = opened.get(address) {
                    let relative: PathBuf = rest.iter().collect();
                    tree::unpack(&mut entry, maildir, &relative, *placement)?;
                }
            }
            _ => {}
        }
    }
    Ok(contents)
}

fn resolve(scope: &dyn AgentScope, args: &SiteArchive) -> Result<Resolved, AgentOpError> {
    let root = |key: &str| {
        scope
            .policy()
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AgentOpError::new(
                    ERROR_POLICY,
                    format!("the policy class {CLASS} names no {key} root"),
                )
            })
    };
    let relative = PathBuf::from(&args.root);
    if args.root.is_empty() || args.root.len() > NAME_MAX || tree::names(&relative).is_err() {
        return Err(AgentOpError::new(
            ERROR_ARGS,
            format!("{} is not a document root under the home", args.root),
        ));
    }
    let mut dumps = Vec::with_capacity(args.databases.len());
    for dump in &args.databases {
        if !database_name(&dump.database) {
            return Err(AgentOpError::new(
                ERROR_ARGS,
                format!("{} is not a database name", dump.database),
            ));
        }
        dumps.push((
            dump.database.clone(),
            scope.resolve(root(DUMPS)?, &dump.dump)?,
        ));
    }
    let mut maildirs = Vec::with_capacity(args.mailboxes.len());
    for mailbox in &args.mailboxes {
        if !address(&mailbox.address) {
            return Err(AgentOpError::new(
                ERROR_ARGS,
                format!("{} is not a mail address", mailbox.address),
            ));
        }
        maildirs.push((
            mailbox.address.clone(),
            scope.resolve(root(MAILDIRS)?, &mailbox.maildir)?,
        ));
    }
    Ok(Resolved {
        archive: scope.resolve(root(ARCHIVES)?, &args.archive)?,
        home: scope.resolve(root(HOMES)?, &args.home)?,
        root: relative,
        dumps,
        maildirs,
        owner: scope.caller_uid(),
    })
}

fn database_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

fn address(value: &str) -> bool {
    value.len() <= NAME_MAX
        && value.contains('@')
        && !value.starts_with('.')
        && !value.contains('/')
        && !value.contains('\0')
}

fn split(path: &Path) -> io::Result<(&Path, &std::ffi::OsStr)> {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => Ok((parent, name)),
        _ => Err(io::Error::other(format!(
            "{} has no parent directory",
            path.display()
        ))),
    }
}

fn discard(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

fn failed(path: &Path, error: &io::Error) -> AgentOpError {
    let key = if seal::refused(error) {
        ERROR_POLICY
    } else {
        ERROR_IO
    };
    AgentOpError::new(key, format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::MetadataExt;

    use keel::sdk::serde_json::json;

    use super::*;

    struct Scope {
        base: tempfile::TempDir,
        policy: Value,
    }

    impl Scope {
        fn new() -> Self {
            let base = tempfile::tempdir().unwrap();
            for root in ["backups", "www", "maildirs", "dumps"] {
                fs::create_dir(base.path().join(root)).unwrap();
            }
            Self {
                base,
                policy: json!({ "archives": "backups", "homes": "www", "maildirs": "maildirs", "dumps": "dumps" }),
            }
        }

        fn path(&self, relative: &str) -> PathBuf {
            self.base.path().join(relative)
        }
    }

    impl AgentScope for Scope {
        fn product(&self) -> &str {
            "panel"
        }

        fn policy(&self) -> &Value {
            &self.policy
        }

        fn resolve(&self, root: &str, name: &str) -> Result<PathBuf, AgentOpError> {
            if name.starts_with("keel-panel-") && !name.contains('/') {
                Ok(self.path(root).join(name))
            } else {
                Err(AgentOpError::new(ERROR_POLICY, name))
            }
        }

        fn caller_uid(&self) -> u32 {
            fs::metadata(self.base.path()).unwrap().uid()
        }
    }

    fn sealed(path: &Path, content: &str) {
        fs::write(path, content).unwrap();
        let mut hashing = Hashing::new(io::sink());
        io::Write::write_all(&mut hashing, content.as_bytes()).unwrap();
        seal::record(path, &hashing.finish().1).unwrap();
    }

    fn site(scope: &Scope) {
        fs::create_dir_all(scope.path("www/keel-panel-alice/public_html")).unwrap();
        fs::write(
            scope.path("www/keel-panel-alice/public_html/index.php"),
            "<?php echo 1;",
        )
        .unwrap();
        sealed(&scope.path("dumps/keel-panel-b1-0.sql"), "-- dump\n");
        fs::create_dir_all(scope.path("maildirs/keel-panel-m1/Maildir/cur")).unwrap();
        fs::write(
            scope.path("maildirs/keel-panel-m1/Maildir/cur/1.eml"),
            "Subject: hi\n",
        )
        .unwrap();
    }

    fn args() -> SiteArchive {
        SiteArchive {
            archive: "keel-panel-b1.tar.zst".to_owned(),
            domain: "a.org".to_owned(),
            home: "keel-panel-alice".to_owned(),
            root: "public_html".to_owned(),
            databases: vec![DatabaseDump {
                database: "hp_alice_wp".to_owned(),
                dump: "keel-panel-b1-0.sql".to_owned(),
            }],
            mailboxes: vec![
                MailboxDir {
                    address: "info@a.org".to_owned(),
                    maildir: "keel-panel-m1".to_owned(),
                },
                MailboxDir {
                    address: "empty@a.org".to_owned(),
                    maildir: "keel-panel-m2".to_owned(),
                },
            ],
        }
    }

    #[test]
    fn a_site_is_packed_with_its_dumps_and_maildirs_and_unpacked_over_what_changed() {
        let scope = Scope::new();
        site(&scope);

        let packed = pack(&scope, args()).unwrap();
        assert_eq!(packed.files, 1);
        assert_eq!(packed.databases, ["hp_alice_wp"]);
        assert_eq!(packed.mailboxes, ["info@a.org"]);
        let archive = scope.path("backups/keel-panel-b1.tar.zst");
        assert_eq!(packed.bytes, fs::metadata(&archive).unwrap().len());
        assert_eq!(fs::metadata(&archive).unwrap().mode() & 0o777, 0o600);
        assert!(!scope.path("backups/.keel-panel-b1.tar.zst.tmp").exists());
        let archive_seal = scope.path("backups/.keel-panel-b1.tar.zst.sha256");
        let meta = fs::metadata(&archive_seal).unwrap();
        assert_eq!(meta.mode() & 0o777, 0o600);
        assert_eq!(meta.uid(), rustix::process::geteuid().as_raw());
        assert_eq!(fs::read_to_string(&archive_seal).unwrap().len(), 64);

        fs::remove_dir_all(scope.path("www/keel-panel-alice/public_html")).unwrap();
        fs::remove_file(scope.path("dumps/keel-panel-b1-0.sql")).unwrap();
        fs::remove_dir_all(scope.path("maildirs/keel-panel-m1")).unwrap();
        let unpacked = unpack(&scope, args()).unwrap();
        assert_eq!(unpacked.files, 1);
        assert_eq!(unpacked.databases, ["hp_alice_wp"]);
        assert_eq!(unpacked.mailboxes, ["info@a.org"]);
        assert_eq!(
            fs::read_to_string(scope.path("www/keel-panel-alice/public_html/index.php")).unwrap(),
            "<?php echo 1;"
        );
        let dump = scope.path("dumps/keel-panel-b1-0.sql");
        assert_eq!(fs::read_to_string(&dump).unwrap(), "-- dump\n");
        let meta = fs::metadata(&dump).unwrap();
        assert_eq!(meta.mode() & 0o777, 0o600);
        assert_eq!(meta.uid(), rustix::process::geteuid().as_raw());
        let mut hashing = Hashing::new(tree::open_file(&dump).unwrap());
        io::copy(&mut hashing, &mut io::sink()).unwrap();
        seal::expected(&dump)
            .unwrap()
            .confirm(&dump, &hashing.finish().1)
            .unwrap();
        assert!(
            !scope
                .path("backups/.keel-panel-b1.tar.zst.restore")
                .exists()
        );
        assert_eq!(
            fs::read_to_string(scope.path("maildirs/keel-panel-m1/Maildir/cur/1.eml")).unwrap(),
            "Subject: hi\n"
        );
    }

    #[test]
    fn a_changed_byte_of_the_archive_is_refused_before_any_file_of_the_home_changes() {
        let scope = Scope::new();
        site(&scope);
        pack(&scope, args()).unwrap();
        let archive = scope.path("backups/keel-panel-b1.tar.zst");
        let index = scope.path("www/keel-panel-alice/public_html/index.php");
        let dump = scope.path("dumps/keel-panel-b1-0.sql");
        let message = scope.path("maildirs/keel-panel-m1/Maildir/cur/1.eml");
        let original = fs::read(&archive).unwrap();
        let mut changed = original.clone();
        let middle = changed.len() / 2;
        changed[middle] ^= 0x01;
        fs::write(&archive, &changed).unwrap();
        fs::write(&index, "defaced").unwrap();
        fs::remove_file(&dump).unwrap();
        seal::remove(&dump).unwrap();
        fs::remove_file(&message).unwrap();

        let refused = unpack(&scope, args()).unwrap_err();
        assert_eq!(refused.key(), ERROR_POLICY, "{}", refused.detail());
        assert!(refused.detail().contains("differs"), "{}", refused.detail());
        assert_eq!(fs::read_to_string(&index).unwrap(), "defaced");
        assert!(!dump.exists());
        assert!(!message.exists());
        assert!(
            !scope
                .path("backups/.keel-panel-b1.tar.zst.restore")
                .exists()
        );

        fs::write(&archive, &original).unwrap();
        seal::remove(&archive).unwrap();
        let unsealed = unpack(&scope, args()).unwrap_err();
        assert_eq!(unsealed.key(), ERROR_POLICY, "{}", unsealed.detail());
        assert_eq!(fs::read_to_string(&index).unwrap(), "defaced");
        assert!(!dump.exists());
    }

    #[test]
    fn a_dump_changed_after_the_agent_wrote_it_or_without_its_digest_is_not_packed() {
        let scope = Scope::new();
        site(&scope);
        let dump = scope.path("dumps/keel-panel-b1-0.sql");
        let archive = scope.path("backups/keel-panel-b1.tar.zst");
        fs::write(&dump, "GRANT ALL ON *.* TO 'hp_alice'@'localhost';\n").unwrap();
        let changed = pack(&scope, args()).unwrap_err();
        assert_eq!(changed.key(), ERROR_POLICY, "{}", changed.detail());
        assert!(!archive.exists());
        assert!(!scope.path("backups/.keel-panel-b1.tar.zst.tmp").exists());
        assert!(!scope.path("backups/.keel-panel-b1.tar.zst.sha256").exists());

        seal::remove(&dump).unwrap();
        let unsealed = pack(&scope, args()).unwrap_err();
        assert_eq!(unsealed.key(), ERROR_POLICY, "{}", unsealed.detail());
        assert!(
            unsealed.detail().contains("no digest"),
            "{}",
            unsealed.detail()
        );
        assert!(!archive.exists());
    }

    #[test]
    fn names_outside_the_policy_and_paths_that_climb_are_refused() {
        let scope = Scope::new();
        for (change, key) in [
            (json!({ "root": "../other" }), ERROR_ARGS),
            (json!({ "root": "/etc" }), ERROR_ARGS),
            (json!({ "archive": "../keel-panel-x" }), ERROR_POLICY),
            (json!({ "home": "hp_alice" }), ERROR_POLICY),
            (
                json!({ "databases": [{ "database": "a`b", "dump": "keel-panel-d" }] }),
                ERROR_ARGS,
            ),
            (
                json!({ "mailboxes": [{ "address": "../x", "maildir": "keel-panel-m" }] }),
                ERROR_ARGS,
            ),
        ] {
            let mut value = serde_json::to_value(args()).unwrap();
            for (key, field) in change.as_object().unwrap() {
                value[key] = field.clone();
            }
            let refused = pack_op().run(&scope, value).unwrap_err();
            assert_eq!(refused.key(), key, "{change}");
        }
        let refused = Scope {
            policy: json!({}),
            ..Scope::new()
        };
        assert_eq!(pack(&refused, args()).unwrap_err().key(), ERROR_POLICY);
        let missing = unpack(&scope, args()).unwrap_err();
        assert_eq!(missing.key(), ERROR_NOT_FOUND);
    }
}
