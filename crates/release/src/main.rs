use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use keel_release::{
    Digest, KeyRing, Manifest, PublicKey, ReleaseError, SecretKey, Signed, Version,
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
        about = "Verify a signed manifest against the compiled release keys, or against --key, and print it"
    )]
    Verify {
        #[arg(value_name = "MANIFEST")]
        manifest: PathBuf,
        #[arg(long, value_name = "PUBLIC_KEY")]
        key: Option<PathBuf>,
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
        Command::Verify { manifest, key } => verify(&manifest, key.as_deref()),
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

fn verify(manifest: &Path, key: Option<&Path>) -> Result<(), CliError> {
    let (ring, named) = match key {
        Some(path) => (
            KeyRing::new(PublicKey::read(path)?, Vec::new()),
            "the given key",
        ),
        None => (KeyRing::compiled()?, "the compiled release keys"),
    };
    let signed = Signed::parse(&read(manifest)?)?;
    let verified = signed.verify(&ring)?;
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
    use super::*;

    #[test]
    fn the_three_subcommands_parse() {
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
        assert!(matches!(cli.command, Command::Verify { key: Some(_), .. }));
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
            out: out.clone(),
        })
        .unwrap();
        verify(&out, Some(&keys.join("release.pub"))).unwrap();
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
            verify(&out, None),
            Err(CliError::Release(ReleaseError::Signature))
        ));
        assert!(matches!(
            verify(&dir.path().join("missing.json"), None),
            Err(CliError::Read { .. })
        ));
    }
}
