use std::fmt;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
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

    pub fn check_fresh(&self, now: u64) -> Result<(), ReleaseError> {
        match self.expires_at {
            None => Err(ReleaseError::NoExpiry {
                of: self.version.to_string(),
            }),
            Some(expires_at) if expires_at <= now => Err(ReleaseError::Expired {
                of: self.version.to_string(),
                expires_at,
                now,
            }),
            Some(_) => Ok(()),
        }
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

pub fn unix_now() -> Result<u64, ReleaseError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .map_err(|error| ReleaseError::Clock(error.to_string()))
}

pub(crate) fn utc(seconds: u64) -> String {
    let days = seconds / 86_400;
    let rest = seconds % 86_400;
    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted + 2) / 5 + 1;
    let month = if shifted < 10 {
        shifted + 3
    } else {
        shifted - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
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
    use crate::{KeyRing, SecretKey, Signed};

    fn manifest() -> Manifest {
        Manifest {
            version: Version::new(0, 2, 0),
            sha256: Digest::of(b"archive").hex(),
            min_upgrade_from: Version::new(0, 1, 0),
            contract: 1,
            notes: "second".to_owned(),
            expires_at: None,
        }
    }

    fn head(expires_at: u64) -> Manifest {
        Manifest {
            expires_at: Some(expires_at),
            ..manifest()
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
    fn an_expired_head_manifest_is_refused() {
        let now = 1_800_000_000;
        assert!(head(now + 1).check_fresh(now).is_ok());
        for expires_at in [now, now - 1, 1, 0] {
            let error = head(expires_at).check_fresh(now).unwrap_err();
            assert!(matches!(
                &error,
                ReleaseError::Expired { of, expires_at: at, now: seen }
                    if of == "0.2.0" && *at == expires_at && *seen == now
            ));
        }
        assert_eq!(
            head(1).check_fresh(now).unwrap_err().to_string(),
            "the head manifest of 0.2.0 expired at 1970-01-01 00:00:01 UTC (now 2027-01-15 08:00:00 UTC); an expired head is refused"
        );
    }

    #[test]
    fn a_head_manifest_without_an_expiry_is_refused() {
        let error = manifest().check_fresh(1_800_000_000).unwrap_err();
        assert!(matches!(&error, ReleaseError::NoExpiry { of } if of == "0.2.0"));
        assert!(error.to_string().contains("carries no expires_at"));
        assert!(manifest().check_fresh(0).is_err());
        let without: Manifest = serde_json::from_str(
            r#"{"version":"0.2.0","sha256":"00","min_upgrade_from":"0.1.0","contract":1,"notes":""}"#,
        )
        .unwrap();
        assert_eq!(without.expires_at, None);
        let null: Manifest = serde_json::from_str(
            r#"{"version":"0.2.0","sha256":"00","min_upgrade_from":"0.1.0","contract":1,"notes":"","expires_at":null}"#,
        )
        .unwrap();
        assert!(null.check_fresh(0).is_err());
        assert!(
            serde_json::from_str::<Manifest>(
                r#"{"version":"0.2.0","sha256":"00","min_upgrade_from":"0.1.0","contract":1,"notes":"","expires_at":-1}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<Manifest>(
                r#"{"version":"0.2.0","sha256":"00","min_upgrade_from":"0.1.0","contract":1,"notes":"","expires_at":"soon"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn a_manifest_without_expires_at_serialises_without_the_field_and_verifies_without_a_clock() {
        assert!(
            serde_json::to_value(manifest())
                .unwrap()
                .get("expires_at")
                .is_none()
        );
        let json = serde_json::to_value(head(1_800_000_000)).unwrap();
        assert_eq!(json["expires_at"], 1_800_000_000);
        let key = SecretKey::generate("release").unwrap();
        let ring = KeyRing::new(key.public(), Vec::new());
        let signed = Signed::sign(&manifest(), &key).unwrap();
        assert_eq!(signed.verify(&ring).unwrap(), manifest());
        let expired_head_read_as_a_version = Signed::sign(&head(1), &key).unwrap();
        assert_eq!(
            expired_head_read_as_a_version.verify(&ring).unwrap(),
            head(1)
        );
    }

    #[test]
    fn the_utc_text_follows_the_calendar() {
        assert_eq!(utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(utc(951_782_400), "2000-02-29 00:00:00 UTC");
        assert_eq!(utc(4_102_444_800), "2100-01-01 00:00:00 UTC");
        assert_eq!(utc(253_402_300_799), "9999-12-31 23:59:59 UTC");
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
