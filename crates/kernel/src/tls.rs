use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rustls::ServerConfig;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use sha2::{Digest, Sha256};

use crate::config::{FILE_NAME, Tls};

const DIR: &str = "tls";
const CERT_FILE: &str = "cert.pem";
const KEY_FILE: &str = "key.pem";
const DIR_MODE: u32 = 0o700;
const CERT_MODE: u32 = 0o644;
const KEY_MODE: u32 = 0o600;
const VALIDITY: Duration = Duration::from_secs(10 * 365 * 24 * 60 * 60 + 3 * 24 * 60 * 60);
const LOCALHOST: &str = "localhost";
const TEMPORARY_SUFFIX: &str = ".tmp";
const ALPN: [&[u8]; 2] = [b"h2", b"http/1.1"];

#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error(
        "\"tls.{present}\" is set in {FILE_NAME} but \"tls.{missing}\" is not; a certificate and its key are configured together"
    )]
    PairIncomplete {
        present: &'static str,
        missing: &'static str,
    },
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path} is not a usable {what}: {reason}")]
    Parse {
        path: PathBuf,
        what: &'static str,
        reason: String,
    },
    #[error("could not create {path}: {source}")]
    Create {
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
    #[error("could not generate the self-signed certificate: {0}")]
    Generate(#[source] rcgen::Error),
    #[error("the certificate and the key do not form a usable pair: {0}")]
    Pair(#[source] rustls::Error),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Configured { cert: PathBuf, key: PathBuf },
    Stored { cert: PathBuf, key: PathBuf },
    Generated { cert: PathBuf, key: PathBuf },
}

impl Source {
    pub fn cert_path(&self) -> &Path {
        match self {
            Self::Configured { cert, .. }
            | Self::Stored { cert, .. }
            | Self::Generated { cert, .. } => cert,
        }
    }
}

pub struct TlsMaterial {
    certs: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
    cert_pem: String,
    key_pem: String,
    fingerprint: [u8; 32],
    source: Option<Source>,
}

impl Clone for TlsMaterial {
    fn clone(&self) -> Self {
        Self {
            certs: self.certs.clone(),
            key: self.key.clone_key(),
            cert_pem: self.cert_pem.clone(),
            key_pem: self.key_pem.clone(),
            fingerprint: self.fingerprint,
            source: self.source.clone(),
        }
    }
}

impl fmt::Debug for TlsMaterial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TlsMaterial")
            .field("fingerprint", &self.fingerprint())
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Names {
    entries: BTreeSet<String>,
}

impl Names {
    pub fn for_host(host_name: Option<&str>, listen: SocketAddr) -> Self {
        let mut names = Self::default();
        names.add(LOCALHOST);
        names.add_ip(IpAddr::from([127, 0, 0, 1]));
        names.add_ip(IpAddr::from([0, 0, 0, 0, 0, 0, 0, 1]));
        if let Some(host) = host_name.map(str::trim).filter(|host| !host.is_empty()) {
            names.add(host);
            if let Ok(resolved) = (host, 0).to_socket_addrs() {
                for addr in resolved {
                    names.add_ip(addr.ip());
                }
            }
        }
        let ip = listen.ip();
        if !ip.is_unspecified() && !ip.is_loopback() {
            names.add_ip(ip);
        }
        names
    }

    fn add(&mut self, name: &str) {
        let name = name.trim().to_ascii_lowercase();
        if let Ok(ip) = name.parse::<IpAddr>() {
            self.add_ip(ip);
        } else if is_dns_name(&name) {
            self.entries.insert(name);
        }
    }

    fn add_ip(&mut self, ip: IpAddr) {
        self.entries.insert(ip.to_string());
    }

    fn iter(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(String::as_str)
    }

    fn common_name(&self) -> String {
        self.entries
            .iter()
            .find(|entry| entry.parse::<IpAddr>().is_err() && entry.as_str() != LOCALHOST)
            .cloned()
            .unwrap_or_else(|| LOCALHOST.to_owned())
    }
}

fn is_dns_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn host_name() -> Option<String> {
    hostname::get()
        .ok()
        .and_then(|name| name.into_string().ok())
        .map(|name| name.trim().to_ascii_lowercase())
        .filter(|name| !name.is_empty())
}

pub fn install_provider() -> bool {
    rustls::crypto::ring::default_provider()
        .install_default()
        .is_ok()
}

impl TlsMaterial {
    pub fn load(tls: &Tls, data_dir: &Path, listen: SocketAddr) -> Result<Self, TlsError> {
        match (tls.cert.trim(), tls.key.trim()) {
            ("", "") => Self::stored_or_generated(data_dir, listen),
            (_, "") => Err(TlsError::PairIncomplete {
                present: "cert",
                missing: "key",
            }),
            ("", _) => Err(TlsError::PairIncomplete {
                present: "key",
                missing: "cert",
            }),
            (cert, key) => {
                let cert = PathBuf::from(cert);
                let key = PathBuf::from(key);
                let mut material = Self::read_pair(&cert, &key)?;
                material.source = Some(Source::Configured { cert, key });
                Ok(material)
            }
        }
    }

    fn stored_or_generated(data_dir: &Path, listen: SocketAddr) -> Result<Self, TlsError> {
        let dir = data_dir.join(DIR);
        let cert = dir.join(CERT_FILE);
        let key = dir.join(KEY_FILE);
        match Self::read_pair(&cert, &key) {
            Ok(mut material) => {
                material.source = Some(Source::Stored {
                    cert: cert.clone(),
                    key,
                });
                return Ok(material);
            }
            Err(TlsError::Read { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
                tracing::info!(cert = %cert.display(), "no certificate under the data directory; generating one");
            }
            Err(error) => {
                tracing::warn!(error = %error, "the stored certificate pair is unusable; generating a new one");
            }
        }
        let names = Names::for_host(host_name().as_deref(), listen);
        let mut material = Self::generate(&names)?;
        material.store(data_dir)?;
        Ok(material)
    }

    pub fn generate(names: &Names) -> Result<Self, TlsError> {
        let key_pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
            .map_err(TlsError::Generate)?;
        let san: Vec<String> = names.iter().map(str::to_owned).collect();
        let mut params = rcgen::CertificateParams::new(san).map_err(TlsError::Generate)?;
        let today = chrono::Utc::now().date_naive() - chrono::Duration::days(1);
        params.not_before = rcgen::date_time_ymd(
            chrono::Datelike::year(&today),
            u8::try_from(chrono::Datelike::month(&today)).unwrap_or(1),
            u8::try_from(chrono::Datelike::day(&today)).unwrap_or(1),
        );
        params.not_after = params.not_before + VALIDITY;
        params.distinguished_name = rcgen::DistinguishedName::new();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, names.common_name());
        let certificate = params.self_signed(&key_pair).map_err(TlsError::Generate)?;
        Self::from_pem(
            certificate.pem().as_bytes(),
            key_pair.serialize_pem().as_bytes(),
        )
    }

    fn from_pem(cert_pem: &[u8], key_pem: &[u8]) -> Result<Self, TlsError> {
        let cert_path = PathBuf::from(CERT_FILE);
        let key_path = PathBuf::from(KEY_FILE);
        Self::parse(&cert_path, cert_pem, &key_path, key_pem)
    }

    fn read_pair(cert: &Path, key: &Path) -> Result<Self, TlsError> {
        let read = |path: &Path| {
            fs::read(path).map_err(|source| TlsError::Read {
                path: path.to_owned(),
                source,
            })
        };
        let cert_pem = read(cert)?;
        let key_pem = read(key)?;
        Self::parse(cert, &cert_pem, key, &key_pem)
    }

    fn parse(
        cert_path: &Path,
        cert_pem: &[u8],
        key_path: &Path,
        key_pem: &[u8],
    ) -> Result<Self, TlsError> {
        let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert_pem)
            .collect::<Result<_, _>>()
            .map_err(|error| TlsError::Parse {
                path: cert_path.to_owned(),
                what: "certificate",
                reason: error.to_string(),
            })?;
        let Some(leaf) = certs.first() else {
            return Err(TlsError::Parse {
                path: cert_path.to_owned(),
                what: "certificate",
                reason: "the file holds no CERTIFICATE block".to_owned(),
            });
        };
        let key = PrivateKeyDer::from_pem_slice(key_pem).map_err(|error| TlsError::Parse {
            path: key_path.to_owned(),
            what: "private key",
            reason: error.to_string(),
        })?;
        let fingerprint: [u8; 32] = Sha256::digest(leaf.as_ref()).into();
        let material = Self {
            certs,
            key,
            cert_pem: String::from_utf8_lossy(cert_pem).into_owned(),
            key_pem: String::from_utf8_lossy(key_pem).into_owned(),
            fingerprint,
            source: None,
        };
        material.server_config()?;
        Ok(material)
    }

    fn store(&mut self, data_dir: &Path) -> Result<(), TlsError> {
        let dir = data_dir.join(DIR);
        fs::create_dir_all(data_dir).map_err(|source| TlsError::Create {
            path: data_dir.to_owned(),
            source,
        })?;
        if !dir.is_dir() {
            DirBuilder::new()
                .mode(DIR_MODE)
                .create(&dir)
                .map_err(|source| TlsError::Create {
                    path: dir.clone(),
                    source,
                })?;
        }
        let cert = dir.join(CERT_FILE);
        let key = dir.join(KEY_FILE);
        write_whole(&key, self.key_pem().as_bytes(), KEY_MODE)?;
        write_whole(&cert, self.cert_pem.as_bytes(), CERT_MODE)?;
        self.source = Some(Source::Generated { cert, key });
        Ok(())
    }

    fn key_pem(&self) -> &str {
        &self.key_pem
    }

    pub fn certificate_pem(&self) -> &str {
        &self.cert_pem
    }

    #[cfg(test)]
    fn certificate_der(&self) -> &[u8] {
        self.certs.first().map(AsRef::as_ref).unwrap_or_default()
    }

    pub fn fingerprint(&self) -> String {
        self.fingerprint
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(":")
    }

    pub fn source(&self) -> Option<&Source> {
        self.source.as_ref()
    }

    pub fn server_config(&self) -> Result<Arc<ServerConfig>, TlsError> {
        let mut config =
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(TlsError::Pair)?
                .with_no_client_auth()
                .with_single_cert(self.certs.clone(), self.key.clone_key())
                .map_err(TlsError::Pair)?;
        config.alpn_protocols = ALPN.iter().map(|protocol| protocol.to_vec()).collect();
        Ok(Arc::new(config))
    }
}

fn write_whole(path: &Path, content: &[u8], mode: u32) -> Result<(), TlsError> {
    let failed = |source| TlsError::Write {
        path: path.to_owned(),
        source,
    };
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(TEMPORARY_SUFFIX);
    let temporary = PathBuf::from(temporary);
    let _ = fs::remove_file(&temporary);
    let written = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&temporary)?;
        file.write_all(content)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if let Err(source) = written {
        let _ = fs::remove_file(&temporary);
        return Err(failed(source));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn listen(text: &str) -> SocketAddr {
        text.parse().expect("an address")
    }

    fn mode_of(path: &Path) -> u32 {
        fs::metadata(path)
            .expect("the file exists")
            .permissions()
            .mode()
            & 0o777
    }

    fn holds(der: &[u8], needle: &[u8]) -> bool {
        der.windows(needle.len()).any(|window| window == needle)
    }

    fn dns_san(name: &str) -> Vec<u8> {
        let mut entry = vec![0x82, u8::try_from(name.len()).expect("a short name")];
        entry.extend_from_slice(name.as_bytes());
        entry
    }

    fn ip_san(ip: IpAddr) -> Vec<u8> {
        let octets: Vec<u8> = match ip {
            IpAddr::V4(v4) => v4.octets().to_vec(),
            IpAddr::V6(v6) => v6.octets().to_vec(),
        };
        let mut entry = vec![0x87, u8::try_from(octets.len()).expect("a short address")];
        entry.extend_from_slice(&octets);
        entry
    }

    #[test]
    fn a_missing_pair_is_generated_for_host_listen_address_and_loopback_under_0700_with_a_0600_key()
    {
        let dir = tempfile::tempdir().expect("a directory");
        let data_dir = dir.path().join("lib");
        let material = TlsMaterial::load(&Tls::default(), &data_dir, listen("10.20.30.40:1500"))
            .expect("the pair is generated");
        let tls_dir = data_dir.join(DIR);
        assert_eq!(mode_of(&tls_dir), DIR_MODE);
        assert_eq!(mode_of(&tls_dir.join(KEY_FILE)), KEY_MODE);
        assert_eq!(mode_of(&tls_dir.join(CERT_FILE)), CERT_MODE);
        assert_eq!(
            material.source(),
            Some(&Source::Generated {
                cert: tls_dir.join(CERT_FILE),
                key: tls_dir.join(KEY_FILE),
            })
        );
        let der = material.certificate_der();
        assert!(holds(der, &dns_san(LOCALHOST)), "localhost is a SAN");
        assert!(
            holds(der, &ip_san(IpAddr::from([127, 0, 0, 1]))),
            "127.0.0.1 is a SAN"
        );
        assert!(
            holds(der, &ip_san(IpAddr::from([0, 0, 0, 0, 0, 0, 0, 1]))),
            "::1 is a SAN"
        );
        assert!(
            holds(der, &ip_san(IpAddr::from([10, 20, 30, 40]))),
            "the listen address is a SAN"
        );
        if let Some(host) = host_name() {
            assert!(holds(der, &dns_san(&host)), "the host name {host} is a SAN");
        }
        assert_eq!(material.fingerprint().len(), 32 * 3 - 1);
        assert!(
            material
                .certificate_pem()
                .starts_with("-----BEGIN CERTIFICATE-----")
        );
        let key_pem = fs::read_to_string(tls_dir.join(KEY_FILE)).expect("the key is readable");
        assert!(
            key_pem.starts_with("-----BEGIN PRIVATE KEY-----"),
            "{key_pem}"
        );
        let reloaded = TlsMaterial::load(&Tls::default(), &data_dir, listen("0.0.0.0:1500"))
            .expect("the stored pair is read back");
        assert_eq!(reloaded.fingerprint(), material.fingerprint());
        assert_eq!(
            reloaded.source(),
            Some(&Source::Stored {
                cert: tls_dir.join(CERT_FILE),
                key: tls_dir.join(KEY_FILE),
            })
        );
        let names = Names::for_host(Some("Example.Test"), listen("[::]:8443"));
        let listed: Vec<&str> = names.iter().collect();
        assert!(listed.contains(&"example.test"), "{listed:?}");
        assert!(listed.contains(&"localhost"), "{listed:?}");
        assert!(listed.contains(&"127.0.0.1"), "{listed:?}");
        assert!(listed.contains(&"::1"), "{listed:?}");
        assert!(!listed.contains(&"::"), "{listed:?}");
        let loopback = Names::for_host(None, listen("127.0.0.1:1500"));
        assert_eq!(loopback.iter().count(), 3);
        let odd = Names::for_host(Some("my_host"), listen("127.0.0.1:1500"));
        assert_eq!(
            odd.iter().count(),
            3,
            "a name that is not a DNS name is left out"
        );
        let mut typed = Names::default();
        typed.add("192.0.2.7");
        typed.add("-bad.example");
        typed.add("good.example.");
        assert_eq!(typed.iter().collect::<Vec<_>>(), ["192.0.2.7"]);
        TlsMaterial::generate(&odd).expect("a pair without the odd host name");
    }

    #[test]
    fn a_configured_cert_without_its_key_fails_the_start_naming_tls_key() {
        let dir = tempfile::tempdir().expect("a directory");
        let tls = Tls {
            cert: "/etc/keel/cert.pem".to_owned(),
            key: String::new(),
            acme: String::new(),
        };
        let error = TlsMaterial::load(&tls, dir.path(), listen("127.0.0.1:1500")).unwrap_err();
        assert!(
            matches!(
                error,
                TlsError::PairIncomplete {
                    present: "cert",
                    missing: "key"
                }
            ),
            "{error}"
        );
        assert!(error.to_string().contains("\"tls.key\""), "{error}");
        let other = Tls {
            cert: String::new(),
            key: "/etc/keel/key.pem".to_owned(),
            acme: String::new(),
        };
        let error = TlsMaterial::load(&other, dir.path(), listen("127.0.0.1:1500")).unwrap_err();
        assert!(error.to_string().contains("\"tls.cert\""), "{error}");
        assert!(!dir.path().join(DIR).exists(), "nothing is generated");
    }

    #[test]
    fn a_configured_pair_is_read_from_its_files_and_a_missing_file_names_its_path() {
        let dir = tempfile::tempdir().expect("a directory");
        let names = Names::for_host(Some("panel.example"), listen("127.0.0.1:1500"));
        let generated = TlsMaterial::generate(&names).expect("a pair");
        let cert = dir.path().join("operator-cert.pem");
        let key = dir.path().join("operator-key.pem");
        fs::write(&cert, generated.certificate_pem()).expect("the cert is written");
        fs::write(&key, generated.key_pem()).expect("the key is written");
        let tls = Tls {
            cert: cert.display().to_string(),
            key: key.display().to_string(),
            acme: String::new(),
        };
        let loaded = TlsMaterial::load(&tls, dir.path(), listen("127.0.0.1:1500"))
            .expect("the configured pair loads");
        assert_eq!(loaded.fingerprint(), generated.fingerprint());
        assert_eq!(
            loaded.source(),
            Some(&Source::Configured {
                cert: cert.clone(),
                key: key.clone()
            })
        );
        assert!(!dir.path().join(DIR).exists(), "nothing is generated");
        fs::remove_file(&key).expect("the key is removed");
        let error = TlsMaterial::load(&tls, dir.path(), listen("127.0.0.1:1500")).unwrap_err();
        assert!(matches!(error, TlsError::Read { .. }), "{error}");
        assert!(
            error.to_string().contains(&key.display().to_string()),
            "{error}"
        );
    }

    #[test]
    fn an_unreadable_pair_under_data_dir_is_regenerated() {
        let dir = tempfile::tempdir().expect("a directory");
        let data_dir = dir.path().join("lib");
        let tls_dir = data_dir.join(DIR);
        fs::create_dir_all(&tls_dir).expect("the directory is created");
        fs::write(tls_dir.join(CERT_FILE), "not a certificate").expect("garbage is written");
        fs::write(tls_dir.join(KEY_FILE), "not a key").expect("garbage is written");
        let material = TlsMaterial::load(&Tls::default(), &data_dir, listen("127.0.0.1:1500"))
            .expect("the pair is regenerated");
        assert!(matches!(material.source(), Some(Source::Generated { .. })));
        let stored = TlsMaterial::from_pem(
            &fs::read(tls_dir.join(CERT_FILE)).expect("the cert"),
            &fs::read(tls_dir.join(KEY_FILE)).expect("the key"),
        )
        .expect("the stored files parse");
        assert_eq!(stored.fingerprint(), material.fingerprint());
        assert_eq!(mode_of(&tls_dir.join(KEY_FILE)), KEY_MODE);
        fs::remove_file(tls_dir.join(KEY_FILE)).expect("the key is removed");
        let again = TlsMaterial::load(&Tls::default(), &data_dir, listen("127.0.0.1:1500"))
            .expect("a half pair is regenerated");
        assert!(matches!(again.source(), Some(Source::Generated { .. })));
        assert_ne!(again.fingerprint(), material.fingerprint());
    }

    #[test]
    fn a_pem_is_written_whole_or_not_at_all() {
        let dir = tempfile::tempdir().expect("a directory");
        let data_dir = dir.path().join("lib");
        let tls_dir = data_dir.join(DIR);
        fs::create_dir_all(tls_dir.join(CERT_FILE)).expect("a directory in the way");
        let error =
            TlsMaterial::load(&Tls::default(), &data_dir, listen("127.0.0.1:1500")).unwrap_err();
        assert!(matches!(error, TlsError::Write { .. }), "{error}");
        assert!(
            error
                .to_string()
                .contains(&tls_dir.join(CERT_FILE).display().to_string()),
            "{error}"
        );
        let left: Vec<String> = fs::read_dir(&tls_dir)
            .expect("the directory is listed")
            .map(|entry| {
                entry
                    .expect("an entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .filter(|name| name.ends_with(TEMPORARY_SUFFIX))
            .collect();
        assert!(left.is_empty(), "no temporary file remains: {left:?}");
        fs::remove_dir(tls_dir.join(CERT_FILE)).expect("the directory is removed");
        let material = TlsMaterial::load(&Tls::default(), &data_dir, listen("127.0.0.1:1500"))
            .expect("the pair is written");
        for name in [CERT_FILE, KEY_FILE] {
            let path = tls_dir.join(name);
            assert!(path.is_file(), "{name} is in place");
            assert!(!tls_dir.join(format!("{name}{TEMPORARY_SUFFIX}")).exists());
        }
        let stored = TlsMaterial::from_pem(
            &fs::read(tls_dir.join(CERT_FILE)).expect("the cert"),
            &fs::read(tls_dir.join(KEY_FILE)).expect("the key"),
        )
        .expect("the stored files parse as a pair");
        assert_eq!(stored.fingerprint(), material.fingerprint());
        assert_eq!(
            stored.fingerprint(),
            Sha256::digest(stored.certificate_der())
                .iter()
                .map(|byte| format!("{byte:02X}"))
                .collect::<Vec<_>>()
                .join(":")
        );
    }

    #[test]
    fn the_server_config_offers_h2_and_http11_and_a_mismatched_pair_is_refused() {
        let first = TlsMaterial::generate(&Names::for_host(None, listen("127.0.0.1:1500")))
            .expect("a pair");
        let second = TlsMaterial::generate(&Names::for_host(None, listen("127.0.0.1:1500")))
            .expect("another pair");
        let config = first.server_config().expect("a server config");
        assert_eq!(
            config.alpn_protocols,
            vec![b"h2".to_vec(), b"http/1.1".to_vec()]
        );
        let error = TlsMaterial::from_pem(
            first.certificate_pem().as_bytes(),
            second.key_pem().as_bytes(),
        )
        .unwrap_err();
        assert!(matches!(error, TlsError::Pair(_)), "{error}");
        assert!(install_provider() || rustls::crypto::CryptoProvider::get_default().is_some());
    }
}
