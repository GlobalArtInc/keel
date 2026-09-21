use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeZone, Utc};
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use sha2::{Digest, Sha256};

use super::{Check, Findings, TLS};
use crate::config::Tls;

const DIR: &str = "tls";
const CERT_FILE: &str = "cert.pem";
const KEY_FILE: &str = "key.pem";
const KEY_OTHERS: u32 = 0o077;
const EXPIRY_WARNING_DAYS: i64 = 30;

const TAG_SEQUENCE: u8 = 0x30;
const TAG_INTEGER: u8 = 0x02;
const TAG_CONTEXT_0: u8 = 0xa0;
const TAG_UTC_TIME: u8 = 0x17;
const TAG_GENERALIZED_TIME: u8 = 0x18;

#[derive(Debug, PartialEq, Eq)]
struct Validity {
    not_before: DateTime<Utc>,
    not_after: DateTime<Utc>,
}

struct Der<'a> {
    bytes: &'a [u8],
}

impl<'a> Der<'a> {
    fn element(&mut self) -> Result<(u8, &'a [u8]), &'static str> {
        let (&tag, rest) = self.bytes.split_first().ok_or("truncated certificate")?;
        let (&first, rest) = rest.split_first().ok_or("truncated length")?;
        let (length, rest) = if first < 0x80 {
            (usize::from(first), rest)
        } else {
            let count = usize::from(first & 0x7f);
            if count == 0 || count > 4 || rest.len() < count {
                return Err("unsupported length encoding");
            }
            let (digits, rest) = rest.split_at(count);
            let length = digits
                .iter()
                .fold(0usize, |value, digit| (value << 8) | usize::from(*digit));
            (length, rest)
        };
        if rest.len() < length {
            return Err("an element runs past the end of the certificate");
        }
        let (content, rest) = rest.split_at(length);
        self.bytes = rest;
        Ok((tag, content))
    }

    fn tagged(&mut self, tag: u8, what: &'static str) -> Result<&'a [u8], &'static str> {
        let (found, content) = self.element()?;
        if found != tag {
            return Err(what);
        }
        Ok(content)
    }
}

fn digits(text: &[u8], range: std::ops::Range<usize>) -> Result<u32, &'static str> {
    let slice = text.get(range).ok_or("a time field is too short")?;
    let mut value = 0u32;
    for digit in slice {
        if !digit.is_ascii_digit() {
            return Err("a time field is not numeric");
        }
        value = value * 10 + u32::from(digit - b'0');
    }
    Ok(value)
}

fn time(tag: u8, text: &[u8]) -> Result<DateTime<Utc>, &'static str> {
    let (year, rest) = match tag {
        TAG_UTC_TIME => {
            let two = digits(text, 0..2)?;
            let year = if two >= 50 { 1900 + two } else { 2000 + two };
            (year, &text[2..])
        }
        TAG_GENERALIZED_TIME => (digits(text, 0..4)?, &text[4..]),
        _ => return Err("a validity field is not a time"),
    };
    let month = digits(rest, 0..2)?;
    let day = digits(rest, 2..4)?;
    let hour = digits(rest, 4..6)?;
    let minute = digits(rest, 6..8)?;
    let second = digits(rest, 8..10)?;
    if rest.last() != Some(&b'Z') {
        return Err("a validity field is not in UTC");
    }
    Utc.with_ymd_and_hms(
        i32::try_from(year).map_err(|_| "a year is out of range")?,
        month,
        day,
        hour,
        minute,
        second,
    )
    .single()
    .ok_or("a validity field is not a calendar time")
}

fn validity(der: &[u8]) -> Result<Validity, &'static str> {
    let mut outer = Der { bytes: der };
    let certificate = outer.tagged(TAG_SEQUENCE, "not a certificate sequence")?;
    let mut certificate = Der { bytes: certificate };
    let tbs = certificate.tagged(TAG_SEQUENCE, "no tbsCertificate")?;
    let mut tbs = Der { bytes: tbs };
    let (tag, _) = tbs.element()?;
    if tag == TAG_CONTEXT_0 {
        tbs.tagged(TAG_INTEGER, "no serial number")?;
    } else if tag != TAG_INTEGER {
        return Err("no serial number");
    }
    tbs.tagged(TAG_SEQUENCE, "no signature algorithm")?;
    tbs.tagged(TAG_SEQUENCE, "no issuer")?;
    let validity = tbs.tagged(TAG_SEQUENCE, "no validity")?;
    let mut validity = Der { bytes: validity };
    let (before_tag, before) = validity.element()?;
    let (after_tag, after) = validity.element()?;
    Ok(Validity {
        not_before: time(before_tag, before)?,
        not_after: time(after_tag, after)?,
    })
}

fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn pair(tls: &Tls, data_dir: &Path) -> (PathBuf, PathBuf, &'static str) {
    match (tls.cert.trim(), tls.key.trim()) {
        ("", _) | (_, "") => {
            let dir = data_dir.join(DIR);
            (dir.join(CERT_FILE), dir.join(KEY_FILE), "generated")
        }
        (cert, key) => (PathBuf::from(cert), PathBuf::from(key), "configured"),
    }
}

pub(super) fn check(tls: &Tls, data_dir: &Path, now: DateTime<Utc>) -> Check {
    let mut findings = Findings::new(TLS);
    let (cert_path, key_path, origin) = pair(tls, data_dir);
    let pem = match fs::read(&cert_path) {
        Ok(pem) => pem,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            findings.fail(format!(
                "{} is absent; the first start of the service generates the pair",
                cert_path.display()
            ));
            return findings.finish("");
        }
        Err(error) => {
            findings.fail(format!("{} cannot be read: {error}", cert_path.display()));
            return findings.finish("");
        }
    };
    let der = match CertificateDer::from_pem_slice(&pem) {
        Ok(der) => der,
        Err(error) => {
            findings.fail(format!(
                "{} is not a PEM certificate: {error}",
                cert_path.display()
            ));
            return findings.finish("");
        }
    };
    let mut summary = String::new();
    match validity(&der) {
        Ok(validity) => {
            let expiry = validity.not_after.format("%Y-%m-%d").to_string();
            if validity.not_after <= now {
                findings.fail(format!("{} expired on {expiry}", cert_path.display()));
            } else if validity.not_after - now < chrono::Duration::days(EXPIRY_WARNING_DAYS) {
                findings.warn(format!(
                    "{} expires on {expiry}, within {EXPIRY_WARNING_DAYS} days",
                    cert_path.display()
                ));
            }
            if validity.not_before > now {
                findings.fail(format!(
                    "{} is not valid before {}",
                    cert_path.display(),
                    validity.not_before.format("%Y-%m-%d")
                ));
            }
            summary.push_str(&format!(
                "the {origin} certificate {} is valid until {expiry}, fingerprint {}",
                cert_path.display(),
                fingerprint(&der)
            ));
        }
        Err(reason) => findings.fail(format!(
            "the validity of {} cannot be read: {reason}",
            cert_path.display()
        )),
    }
    match fs::metadata(&key_path) {
        Ok(meta) => {
            let mode = meta.mode() & 0o7777;
            if mode & KEY_OTHERS != 0 {
                findings.fail(format!(
                    "{} has mode {mode:04o}; the key must be readable by the service user alone (0600)",
                    key_path.display()
                ));
            }
            summary.push_str(&format!("; {} has mode {mode:04o}", key_path.display()));
        }
        Err(error) => findings.fail(format!("{} cannot be read: {error}", key_path.display())),
    }
    findings.finish(summary)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::super::Status;
    use super::*;

    fn certificate(not_before: (i32, u8, u8), not_after: (i32, u8, u8)) -> (String, Vec<u8>) {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut params = rcgen::CertificateParams::new(vec!["localhost".to_owned()]).unwrap();
        params.not_before = rcgen::date_time_ymd(not_before.0, not_before.1, not_before.2);
        params.not_after = rcgen::date_time_ymd(not_after.0, not_after.1, not_after.2);
        let cert = params.self_signed(&key).unwrap();
        (cert.pem(), cert.der().to_vec())
    }

    #[test]
    fn the_validity_of_a_certificate_is_read_from_its_der_encoding() {
        let (_, der) = certificate((2026, 9, 20), (2036, 9, 20));
        let read = validity(&der).unwrap();
        assert_eq!(
            read.not_before,
            Utc.with_ymd_and_hms(2026, 9, 20, 0, 0, 0).unwrap()
        );
        assert_eq!(
            read.not_after,
            Utc.with_ymd_and_hms(2036, 9, 20, 0, 0, 0).unwrap()
        );
        let (_, far) = certificate((2026, 1, 1), (2055, 6, 30));
        assert_eq!(
            validity(&far).unwrap().not_after,
            Utc.with_ymd_and_hms(2055, 6, 30, 0, 0, 0).unwrap()
        );
        assert_eq!(
            validity(&der[..20]).unwrap_err(),
            "an element runs past the end of the certificate"
        );
        assert_eq!(
            validity(&[0x02, 0x01, 0x00]).unwrap_err(),
            "not a certificate sequence"
        );
        assert_eq!(validity(&[]).unwrap_err(), "truncated certificate");
        assert_eq!(
            time(TAG_UTC_TIME, b"990101000000Z").unwrap(),
            Utc.with_ymd_and_hms(1999, 1, 1, 0, 0, 0).unwrap()
        );
        assert_eq!(
            time(TAG_UTC_TIME, b"26010100000").unwrap_err(),
            "a time field is too short"
        );
        assert_eq!(
            time(TAG_UTC_TIME, b"2601010000x0Z").unwrap_err(),
            "a time field is not numeric"
        );
        assert_eq!(
            time(TAG_GENERALIZED_TIME, b"20260101000000+0100").unwrap_err(),
            "a validity field is not in UTC"
        );
        assert_eq!(
            time(TAG_INTEGER, b"1").unwrap_err(),
            "a validity field is not a time"
        );
        assert_eq!(fingerprint(b"abc").len(), 95);
    }

    #[test]
    fn the_check_reads_the_generated_pair_and_names_an_expiry_and_an_open_key() {
        let dir = tempfile::tempdir().unwrap();
        let tls_dir = dir.path().join(DIR);
        fs::create_dir_all(&tls_dir).unwrap();
        let (pem, _) = certificate((2026, 9, 20), (2036, 9, 20));
        fs::write(tls_dir.join(CERT_FILE), &pem).unwrap();
        fs::write(tls_dir.join(KEY_FILE), "key").unwrap();
        fs::set_permissions(tls_dir.join(KEY_FILE), fs::Permissions::from_mode(0o600)).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap();
        let green = check(&Tls::default(), dir.path(), now);
        assert_eq!(green.status, Status::Ok, "{}", green.message);
        assert!(
            green.message.starts_with("the generated certificate"),
            "{}",
            green.message
        );
        assert!(
            green
                .message
                .contains("is valid until 2036-09-20, fingerprint")
        );
        assert!(
            green.message.ends_with("key.pem has mode 0600"),
            "{}",
            green.message
        );
        let soon = Utc.with_ymd_and_hms(2036, 9, 1, 0, 0, 0).unwrap();
        let expiring = check(&Tls::default(), dir.path(), soon);
        assert_eq!(expiring.status, Status::Warn);
        assert!(
            expiring
                .message
                .contains("expires on 2036-09-20, within 30 days")
        );
        let late = Utc.with_ymd_and_hms(2036, 9, 21, 0, 0, 0).unwrap();
        let expired = check(&Tls::default(), dir.path(), late);
        assert_eq!(expired.status, Status::Fail);
        assert!(expired.message.contains("expired on 2036-09-20"));
        let early = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        let future = check(&Tls::default(), dir.path(), early);
        assert_eq!(future.status, Status::Fail);
        assert!(future.message.contains("is not valid before 2026-09-20"));
        fs::set_permissions(tls_dir.join(KEY_FILE), fs::Permissions::from_mode(0o644)).unwrap();
        let open = check(&Tls::default(), dir.path(), now);
        assert_eq!(open.status, Status::Fail);
        assert!(
            open.message.contains("key.pem has mode 0644"),
            "{}",
            open.message
        );
    }

    #[test]
    fn a_configured_pair_is_read_from_its_paths_and_a_missing_or_broken_file_is_named() {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap();
        let absent = check(&Tls::default(), dir.path(), now);
        assert_eq!(absent.status, Status::Fail);
        assert!(
            absent
                .message
                .contains("cert.pem is absent; the first start")
        );
        let cert = dir.path().join("own.crt");
        let key = dir.path().join("own.key");
        fs::write(&cert, "not a certificate").unwrap();
        fs::write(&key, "k").unwrap();
        fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
        let configured = Tls {
            cert: cert.display().to_string(),
            key: key.display().to_string(),
            acme: String::new(),
        };
        let broken = check(&configured, dir.path(), now);
        assert_eq!(broken.status, Status::Fail);
        assert!(
            broken.message.contains("own.crt is not a PEM certificate"),
            "{}",
            broken.message
        );
        let (pem, _) = certificate((2026, 9, 20), (2036, 9, 20));
        fs::write(&cert, pem).unwrap();
        let green = check(&configured, dir.path(), now);
        assert_eq!(green.status, Status::Ok, "{}", green.message);
        assert!(green.message.starts_with("the configured certificate"));
        fs::remove_file(&key).unwrap();
        let no_key = check(&configured, dir.path(), now);
        assert_eq!(no_key.status, Status::Fail);
        assert!(no_key.message.contains("own.key cannot be read"));
    }
}
