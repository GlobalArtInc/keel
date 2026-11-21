use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use keel_release::{
    Digest, KeyRing, Manifest, PublicKey, ReleaseError, SecretKey, Signed, Version, unix_now,
};

#[derive(Debug, Parser)]
#[command(
    name = "keel-release",
    version,
    about = "The release key pair, the signing of a release manifest and its verification"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(about = "Write a new release key pair as release.key (mode 0600) and release.pub")]
    Keygen {
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
        #[arg(long, default_value = "keel release key", value_name = "TEXT")]
        comment: String,
    },
    #[command(about = "Sign a release manifest over an archive and write the signed envelope")]
    Sign(SignArgs),
    #[command(
        about = "Sign the head of a channel again with a new expires_at, the rest of the manifest unchanged"
    )]
    Refresh(RefreshArgs),
    #[command(
        about = "Verify a signed manifest against the compiled release keys, or against --key, and print it"
    )]
    Verify {
        #[arg(value_name = "MANIFEST")]
        manifest: PathBuf,
        #[arg(long, value_name = "PUBLIC_KEY")]
        key: Option<PathBuf>,
        #[arg(
            long,
            help = "The manifest is the head of a channel: refuse it unless it carries an expires_at after now"
        )]
        head: bool,
    },
}

#[derive(Debug, Args)]
struct SignArgs {
    #[arg(long, value_name = "SECRET_KEY")]
    key: PathBuf,
    #[arg(long)]
    version: Version,
    #[arg(long, value_name = "FILE")]
    archive: PathBuf,
    #[arg(long, value_name = "VERSION")]
    min_upgrade_from: Version,
    #[arg(long)]
    contract: u32,
    #[arg(long, value_name = "TEXT", conflicts_with = "notes_file")]
    notes: Option<String>,
    #[arg(long, value_name = "FILE")]
    notes_file: Option<PathBuf>,
    #[arg(
        long,
        value_name = "SECONDS",
        help = "Make the manifest a channel head that expires at this instant, in whole seconds since the Unix epoch"
    )]
    expires_at: Option<u64>,
    #[arg(long, value_name = "FILE")]
    out: PathBuf,
}

#[derive(Debug, Args)]
struct RefreshArgs {
    #[arg(long, value_name = "SECRET_KEY")]
    key: PathBuf,
    #[arg(long, value_name = "HEAD")]
    head: PathBuf,
    #[arg(
        long,
        value_name = "PUBLIC_KEY",
        help = "Verify the head against this key instead of the compiled release keys"
    )]
    verify_key: Option<PathBuf>,
    #[arg(long, value_name = "SECONDS")]
    expires_at: u64,
    #[arg(long, value_name = "FILE")]
    out: PathBuf,
}

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error(transparent)]
    Release(#[from] ReleaseError),
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not create {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the verified payload is not UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error(
        "{path} verifies, but its payload is not what this build writes for the manifest it holds; signing it again would change more than expires_at"
    )]
    NotReproduced { path: PathBuf },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("keel-release: {error}");
            let mut source = std::error::Error::source(&error);
            while let Some(cause) = source {
                eprintln!("  caused by: {cause}");
                source = cause.source();
            }
            ExitCode::FAILURE
        }
    }
}

fn run(command: Command) -> Result<(), CliError> {
    match command {
        Command::Keygen { out, comment } => keygen(&out, &comment),
        Command::Sign(args) => sign(args),
        Command::Refresh(args) => refresh(&args),
        Command::Verify {
            manifest,
            key,
            head,
        } => verify(&manifest, key.as_deref(), head),
    }
}

fn read(path: &Path) -> Result<Vec<u8>, CliError> {
    fs::read(path).map_err(|source| CliError::Read {
        path: path.to_owned(),
        source,
    })
}

fn keygen(out: &Path, comment: &str) -> Result<(), CliError> {
    fs::create_dir_all(out).map_err(|source| CliError::CreateDir {
        path: out.to_owned(),
        source,
    })?;
    let secret = SecretKey::generate(comment)?;
    let secret_path = out.join("release.key");
    let public_path = out.join("release.pub");
    secret.write(&secret_path)?;
    secret.public().write(&public_path)?;
    println!("secret key: {}", secret_path.display());
    println!("public key: {}", public_path.display());
    println!("{}", secret.public());
    Ok(())
}

fn sign(args: SignArgs) -> Result<(), CliError> {
    let key = SecretKey::read(&args.key)?;
    let notes = match (args.notes, args.notes_file) {
        (Some(text), _) => text,
        (None, Some(path)) => String::from_utf8(read(&path)?)?,
        (None, None) => String::new(),
    };
    let manifest = Manifest {
        version: args.version,
        sha256: Digest::of(&read(&args.archive)?).hex(),
        min_upgrade_from: args.min_upgrade_from,
        contract: args.contract,
        notes,
        expires_at: args.expires_at,
    };
    let signed = Signed::sign(&manifest, &key)?;
    fs::write(&args.out, signed.to_json()?).map_err(|source| CliError::Write {
        path: args.out.clone(),
        source,
    })?;
    println!(
        "signed {} for {} (sha256 {}) with {}",
        args.out.display(),
        manifest.version,
        manifest.sha256,
        key.public()
    );
    Ok(())
}

fn ring_of(key: Option<&Path>) -> Result<(KeyRing, &'static str), CliError> {
    Ok(match key {
        Some(path) => (
            KeyRing::new(PublicKey::read(path)?, Vec::new()),
            "the given key",
        ),
        None => (KeyRing::compiled()?, "the compiled release keys"),
    })
}

fn refresh(args: &RefreshArgs) -> Result<(), CliError> {
    let key = SecretKey::read(&args.key)?;
    let (ring, named) = ring_of(args.verify_key.as_deref())?;
    let signed = Signed::parse(&read(&args.head)?)?;
    let manifest = signed.verify(&ring)?;
    if serde_json::to_vec_pretty(&manifest).map_err(ReleaseError::Serialise)? != signed.payload() {
        return Err(CliError::NotReproduced {
            path: args.head.clone(),
        });
    }
    let refreshed = Manifest {
        expires_at: Some(args.expires_at),
        ..manifest
    };
    let renewed = Signed::sign(&refreshed, &key)?;
    fs::write(&args.out, renewed.to_json()?).map_err(|source| CliError::Write {
        path: args.out.clone(),
        source,
    })?;
    println!(
        "signed {} for {} again with {}; {} verified under {named}, expires_at now {}",
        args.out.display(),
        refreshed.version,
        key.public(),
        args.head.display(),
        args.expires_at
    );
    Ok(())
}

fn verify(manifest: &Path, key: Option<&Path>, head: bool) -> Result<(), CliError> {
    let (ring, named) = ring_of(key)?;
    let signed = Signed::parse(&read(manifest)?)?;
    let verified = if head {
        signed.verify_head(&ring, unix_now()?)?
    } else {
        signed.verify(&ring)?
    };
    print!("{}", String::from_utf8(signed.payload().to_vec())?);
    println!();
    println!(
        "verified {} for {} with {named}",
        manifest.display(),
        verified.version
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;

    use super::*;

    #[test]
    fn the_four_subcommands_parse() {
        let cli = Cli::try_parse_from(["keel-release", "keygen", "--out", "/tmp/k"]).unwrap();
        assert!(matches!(cli.command, Command::Keygen { .. }));
        let cli = Cli::try_parse_from([
            "keel-release",
            "sign",
            "--key",
            "k",
            "--version",
            "0.2.0",
            "--archive",
            "a.tar.zst",
            "--min-upgrade-from",
            "0.1.0",
            "--contract",
            "1",
            "--notes",
            "n",
            "--expires-at",
            "1800000000",
            "--out",
            "m.json",
        ])
        .unwrap();
        let Command::Sign(args) = cli.command else {
            panic!("expected sign");
        };
        assert_eq!(args.version, Version::new(0, 2, 0));
        assert_eq!(args.contract, 1);
        assert_eq!(args.notes.as_deref(), Some("n"));
        assert_eq!(args.expires_at, Some(1_800_000_000));
        assert!(
            Cli::try_parse_from([
                "keel-release",
                "sign",
                "--key",
                "k",
                "--version",
                "not-a-version",
                "--archive",
                "a",
                "--min-upgrade-from",
                "0.1.0",
                "--contract",
                "1",
                "--out",
                "m"
            ])
            .is_err()
        );
        let cli =
            Cli::try_parse_from(["keel-release", "verify", "m.json", "--key", "k.pub"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Verify {
                key: Some(_),
                head: false,
                ..
            }
        ));
        let cli = Cli::try_parse_from(["keel-release", "verify", "m.json", "--head"]).unwrap();
        assert!(matches!(cli.command, Command::Verify { head: true, .. }));
        let cli = Cli::try_parse_from([
            "keel-release",
            "refresh",
            "--key",
            "k",
            "--head",
            "h.json",
            "--expires-at",
            "1800000000",
            "--out",
            "o.json",
        ])
        .unwrap();
        let Command::Refresh(args) = cli.command else {
            panic!("expected refresh");
        };
        assert_eq!(args.expires_at, 1_800_000_000);
        assert!(args.verify_key.is_none());
        assert!(
            Cli::try_parse_from([
                "keel-release",
                "refresh",
                "--key",
                "k",
                "--head",
                "h",
                "--out",
                "o"
            ])
            .is_err()
        );
    }

    #[test]
    fn keygen_sign_and_verify_run_end_to_end_in_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let keys = dir.path().join("keys");
        keygen(&keys, "test").unwrap();
        assert!(matches!(
            keygen(&keys, "test"),
            Err(CliError::Release(ReleaseError::Exists { .. }))
        ));
        let archive = dir.path().join("keel-0.2.0-x86_64-linux-musl.tar.zst");
        fs::write(&archive, b"archive bytes").unwrap();
        let notes = dir.path().join("notes.txt");
        fs::write(&notes, "Notes from the tag.\n").unwrap();
        let out = dir.path().join("manifest.json");
        sign(SignArgs {
            key: keys.join("release.key"),
            version: Version::new(0, 2, 0),
            archive: archive.clone(),
            min_upgrade_from: Version::new(0, 1, 0),
            contract: 1,
            notes: None,
            notes_file: Some(notes),
            expires_at: None,
            out: out.clone(),
        })
        .unwrap();
        verify(&out, Some(&keys.join("release.pub")), false).unwrap();
        let ring = KeyRing::new(
            PublicKey::read(&keys.join("release.pub")).unwrap(),
            Vec::new(),
        );
        let manifest = Signed::parse(&fs::read(&out).unwrap())
            .unwrap()
            .verify(&ring)
            .unwrap();
        assert_eq!(manifest.sha256, Digest::of(b"archive bytes").hex());
        assert_eq!(manifest.notes, "Notes from the tag.\n");
        assert!(matches!(
            verify(&out, None, false),
            Err(CliError::Release(ReleaseError::Signature))
        ));
        assert!(matches!(
            verify(&dir.path().join("missing.json"), None, false),
            Err(CliError::Read { .. })
        ));
    }

    struct Rehearsal {
        dir: tempfile::TempDir,
    }

    impl Rehearsal {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            keygen(&dir.path().join("keys"), "test").unwrap();
            fs::write(dir.path().join("archive"), b"archive bytes").unwrap();
            Self { dir }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.dir.path().join(name)
        }

        fn secret(&self) -> PathBuf {
            self.path("keys/release.key")
        }

        fn public(&self) -> PathBuf {
            self.path("keys/release.pub")
        }

        fn sign(&self, out: &str, expires_at: Option<u64>) -> PathBuf {
            let out = self.path(out);
            sign(SignArgs {
                key: self.secret(),
                version: Version::new(0, 2, 0),
                archive: self.path("archive"),
                min_upgrade_from: Version::new(0, 1, 0),
                contract: 1,
                notes: Some("Notes.\n".to_owned()),
                notes_file: None,
                expires_at,
                out: out.clone(),
            })
            .unwrap();
            out
        }

        fn refresh(&self, head: &Path, out: &str, expires_at: u64) -> Result<PathBuf, CliError> {
            let out = self.path(out);
            refresh(&RefreshArgs {
                key: self.secret(),
                head: head.to_owned(),
                verify_key: Some(self.public()),
                expires_at,
                out: out.clone(),
            })
            .map(|()| out)
        }

        fn payload(&self, signed: &Path) -> String {
            let ring = KeyRing::new(PublicKey::read(&self.public()).unwrap(), Vec::new());
            let signed = Signed::parse(&fs::read(signed).unwrap()).unwrap();
            signed.verify(&ring).unwrap();
            String::from_utf8(signed.payload().to_vec()).unwrap()
        }
    }

    #[test]
    fn verify_head_refuses_a_manifest_without_an_expiry_or_past_it() {
        let rehearsal = Rehearsal::new();
        let public = rehearsal.public();
        let version_manifest = rehearsal.sign("version.json", None);
        verify(&version_manifest, Some(&public), false).unwrap();
        assert!(matches!(
            verify(&version_manifest, Some(&public), true),
            Err(CliError::Release(ReleaseError::NoExpiry { .. }))
        ));
        let expired = rehearsal.sign("expired.json", Some(1));
        verify(&expired, Some(&public), false).unwrap();
        assert!(matches!(
            verify(&expired, Some(&public), true),
            Err(CliError::Release(ReleaseError::Expired { .. }))
        ));
        let now = unix_now().unwrap();
        let fresh = rehearsal.sign("fresh.json", Some(now + 3_600));
        verify(&fresh, Some(&public), true).unwrap();
    }

    #[test]
    fn a_refreshed_head_differs_from_its_source_in_expires_at_alone() {
        let rehearsal = Rehearsal::new();
        let source = rehearsal.sign("head.json", Some(1_800_000_000));
        let refreshed = rehearsal
            .refresh(&source, "refreshed.json", 1_900_000_000)
            .unwrap();
        let before = rehearsal.payload(&source);
        let after = rehearsal.payload(&refreshed);
        assert_eq!(
            before.replace("1800000000", "1900000000"),
            after,
            "the payloads differ in the one number"
        );
        let legacy = rehearsal.sign("legacy.json", None);
        let adopted = rehearsal
            .refresh(&legacy, "adopted.json", 1_900_000_000)
            .unwrap();
        let legacy_payload = rehearsal.payload(&legacy);
        assert_eq!(
            format!(
                "{},\n  \"expires_at\": 1900000000\n}}",
                legacy_payload.strip_suffix("\n}").unwrap()
            ),
            rehearsal.payload(&adopted),
            "a head published before the expiry gains the one field"
        );
    }

    #[test]
    fn a_head_that_does_not_verify_or_is_not_reproduced_is_not_refreshed() {
        let rehearsal = Rehearsal::new();
        let source = rehearsal.sign("head.json", Some(1_800_000_000));
        let mut envelope: serde_json::Value =
            serde_json::from_slice(&fs::read(&source).unwrap()).unwrap();
        let mut payload = STANDARD
            .decode(envelope["payload"].as_str().unwrap())
            .unwrap();
        let position = payload.iter().position(|byte| *byte == b'2').unwrap();
        payload[position] = b'3';
        envelope["payload"] = serde_json::Value::String(STANDARD.encode(&payload));
        let tampered = rehearsal.path("tampered.json");
        fs::write(&tampered, envelope.to_string()).unwrap();
        assert!(matches!(
            rehearsal.refresh(&tampered, "out.json", 1_900_000_000),
            Err(CliError::Release(ReleaseError::Signature))
        ));
        assert!(!rehearsal.path("out.json").exists());

        let key = SecretKey::read(&rehearsal.secret()).unwrap();
        let compact = Signed::seal(
            br#"{"version":"0.2.0","sha256":"00","min_upgrade_from":"0.1.0","contract":1,"notes":"","later":true}"#.to_vec(),
            &key,
        );
        let unreproduced = rehearsal.path("unreproduced.json");
        fs::write(&unreproduced, compact.to_json().unwrap()).unwrap();
        assert!(matches!(
            rehearsal.refresh(&unreproduced, "out.json", 1_900_000_000),
            Err(CliError::NotReproduced { .. })
        ));
        assert!(!rehearsal.path("out.json").exists());
    }
}
