use std::fmt;
use std::str::FromStr;

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::{Digest, ReleaseError};

const ARCHIVE_TARGET: &str = "x86_64-linux-musl";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: Version,
    pub sha256: String,
    pub min_upgrade_from: Version,
    pub contract: u32,
    pub notes: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Stable,
    Beta,
}

impl Manifest {
    pub fn archive_name(&self) -> String {
        archive_name(&self.version)
    }

    pub fn admits(&self, version: &Version) -> Result<(), ReleaseError> {
        if *version < self.min_upgrade_from {
            return Err(ReleaseError::BelowMinimum {
                version: version.to_string(),
                min_upgrade_from: self.min_upgrade_from.to_string(),
                of: self.version.to_string(),
            });
        }
        Ok(())
    }

    pub fn check_digest(&self, digest: &Digest) -> Result<(), ReleaseError> {
        let actual = digest.hex();
        if self.sha256.eq_ignore_ascii_case(&actual) {
            Ok(())
        } else {
            Err(ReleaseError::Digest {
                expected: self.sha256.clone(),
                actual,
            })
        }
    }
}

impl Channel {
    fn as_str(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Beta => "beta",
        }
    }

    pub fn head_path(self) -> String {
        format!("{}/manifest.json", self.as_str())
    }
}

impl FromStr for Channel {
    type Err = ReleaseError;

    fn from_str(text: &str) -> Result<Self, ReleaseError> {
        match text {
            "stable" => Ok(Channel::Stable),
            "beta" => Ok(Channel::Beta),
            other => Err(ReleaseError::Channel(other.to_owned())),
        }
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

fn archive_name(version: &Version) -> String {
    format!("keel-{version}-{ARCHIVE_TARGET}.tar.zst")
}

pub fn manifest_path(version: &Version) -> String {
    format!("versions/{version}/manifest.json")
}

pub fn archive_path(version: &Version) -> String {
    format!("versions/{version}/{}", archive_name(version))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        Manifest {
            version: Version::new(0, 2, 0),
            sha256: Digest::of(b"archive").hex(),
            min_upgrade_from: Version::new(0, 1, 0),
            contract: 1,
            notes: "second".to_owned(),
        }
    }

    #[test]
    fn a_version_below_min_upgrade_from_is_refused() {
        let head = manifest();
        assert!(head.admits(&Version::new(0, 1, 0)).is_ok());
        assert!(head.admits(&Version::new(0, 2, 0)).is_ok());
        assert!(head.admits(&Version::new(3, 0, 0)).is_ok());
        let error = head.admits(&Version::new(0, 0, 9)).unwrap_err();
        assert!(matches!(
            &error,
            ReleaseError::BelowMinimum { version, min_upgrade_from, of }
                if version == "0.0.9" && min_upgrade_from == "0.1.0" && of == "0.2.0"
        ));
        assert_eq!(
            error.to_string(),
            "0.0.9 is below min_upgrade_from 0.1.0 of the manifest of 0.2.0"
        );
        assert!(
            head.admits(&Version::parse("0.1.0-beta.1").unwrap())
                .is_err()
        );
    }

    #[test]
    fn an_archive_whose_sha256_differs_from_the_manifest_is_refused() {
        let manifest = manifest();
        assert!(manifest.check_digest(&Digest::of(b"archive")).is_ok());
        let upper = Manifest {
            sha256: manifest.sha256.to_uppercase(),
            ..manifest.clone()
        };
        assert!(upper.check_digest(&Digest::of(b"archive")).is_ok());
        let error = manifest.check_digest(&Digest::of(b"archivf")).unwrap_err();
        assert!(matches!(
            &error,
            ReleaseError::Digest { expected, actual }
                if *expected == manifest.sha256 && *actual == Digest::of(b"archivf").hex()
        ));
    }

    #[test]
    fn the_channel_layout_follows_the_design() {
        assert_eq!(Channel::Stable.head_path(), "stable/manifest.json");
        assert_eq!(Channel::Beta.head_path(), "beta/manifest.json");
        assert_eq!("stable".parse::<Channel>().unwrap(), Channel::Stable);
        assert_eq!("beta".parse::<Channel>().unwrap(), Channel::Beta);
        assert!(matches!(
            "nightly".parse::<Channel>(),
            Err(ReleaseError::Channel(name)) if name == "nightly"
        ));
        assert_eq!(Channel::Beta.to_string(), "beta");
        assert_eq!(
            serde_json::from_str::<Channel>("\"stable\"").unwrap(),
            Channel::Stable
        );
        assert!(serde_json::from_str::<Channel>("\"Stable\"").is_err());
        let version = Version::new(1, 3, 0);
        assert_eq!(manifest_path(&version), "versions/1.3.0/manifest.json");
        assert_eq!(
            archive_path(&version),
            "versions/1.3.0/keel-1.3.0-x86_64-linux-musl.tar.zst"
        );
        assert_eq!(
            manifest().archive_name(),
            "keel-0.2.0-x86_64-linux-musl.tar.zst"
        );
    }

    #[test]
    fn a_manifest_keeps_unknown_fields_out_and_its_five_fields_in() {
        let json = serde_json::to_value(manifest()).unwrap();
        assert_eq!(
            json.as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            ["version", "sha256", "min_upgrade_from", "contract", "notes"]
        );
        let extended = r#"{"version":"0.2.0","sha256":"00","min_upgrade_from":"0.1.0","contract":1,"notes":"","later":true}"#;
        assert_eq!(
            serde_json::from_str::<Manifest>(extended).unwrap().version,
            Version::new(0, 2, 0)
        );
    }
}
