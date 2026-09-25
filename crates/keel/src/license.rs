use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use keel_release::{KeyRing, Signed};
use keel_sdk::system::LicenseKey;

pub(crate) const INSTALL: &str = "license.install";

const LEASE_LIMIT: u64 = 65_536;

pub(crate) struct CompiledLicenseKey;

impl LicenseKey for CompiledLicenseKey {
    fn open(&self, signed: &[u8]) -> Result<Vec<u8>, String> {
        KeyRing::license()
            .and_then(|ring| Signed::parse(signed)?.open(&ring).map(<[u8]>::to_vec))
            .map_err(|error| error.to_string())
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum LeaseFileError {
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path} is over {LEASE_LIMIT} bytes; a lease is a small signed text")]
    TooLarge { path: PathBuf },
    #[error("{path} is not UTF-8 text")]
    Text { path: PathBuf },
}

pub(crate) fn read_lease(path: &Path) -> Result<String, LeaseFileError> {
    let failed = |source| LeaseFileError::Read {
        path: path.to_owned(),
        source,
    };
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(failed)?
        .take(LEASE_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(failed)?;
    if bytes.len() as u64 > LEASE_LIMIT {
        return Err(LeaseFileError::TooLarge {
            path: path.to_owned(),
        });
    }
    String::from_utf8(bytes).map_err(|_| LeaseFileError::Text {
        path: path.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use keel_release::SecretKey;

    use super::*;

    #[test]
    fn the_compiled_key_refuses_a_lease_signed_with_another_key() {
        let other = SecretKey::generate("other").unwrap();
        let signed = Signed::seal(b"{}".to_vec(), &other).to_json().unwrap();
        let refused = CompiledLicenseKey.open(signed.as_bytes()).unwrap_err();
        assert!(refused.contains("signature"), "{refused}");
        assert!(CompiledLicenseKey.open(b"not an envelope").is_err());
    }

    #[test]
    fn a_lease_file_is_read_as_text_up_to_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("license.lease");
        std::fs::write(&path, "{\"payload\":\"\",\"signature\":\"\"}\n").unwrap();
        assert!(read_lease(&path).unwrap().starts_with("{\"payload\""));
        std::fs::write(&path, vec![b'x'; LEASE_LIMIT as usize + 1]).unwrap();
        assert!(matches!(
            read_lease(&path),
            Err(LeaseFileError::TooLarge { .. })
        ));
        std::fs::write(&path, [0xff, 0xfe]).unwrap();
        assert!(matches!(
            read_lease(&path),
            Err(LeaseFileError::Text { .. })
        ));
        assert!(matches!(
            read_lease(&dir.path().join("missing")),
            Err(LeaseFileError::Read { .. })
        ));
    }
}
