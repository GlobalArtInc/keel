use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};

use crate::error::Error;

const ALPN: &[u8] = b"http/1.1";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Trust {
    #[default]
    System,
    Pinned(Vec<Pin>),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Pin([u8; 32]);

impl Pin {
    pub fn of(certificate_der: &[u8]) -> Self {
        Self(Sha256::digest(certificate_der).into())
    }

    pub fn from_pem(pem: &[u8]) -> Result<Vec<Self>, Error> {
        let certificates = CertificateDer::pem_slice_iter(pem)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| Error::invalid(format!("the PEM is not readable: {error}")))?;
        if certificates.is_empty() {
            return Err(Error::invalid("the PEM holds no CERTIFICATE block"));
        }
        Ok(certificates
            .iter()
            .map(|certificate| Self::of(certificate.as_ref()))
            .collect())
    }
}

impl FromStr for Pin {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self, Error> {
        let refused = || {
            Error::invalid(format!(
                "{text:?} is not the SHA-256 fingerprint of a certificate, 32 bytes in hex"
            ))
        };
        let digits: String = text.trim().split(':').collect();
        if digits.len() != 64 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(refused());
        }
        let mut bytes = [0u8; 32];
        for (index, byte) in bytes.iter_mut().enumerate() {
            let pair = digits.get(index * 2..index * 2 + 2).ok_or_else(refused)?;
            *byte = u8::from_str_radix(pair, 16).map_err(|_| refused())?;
        }
        Ok(Self(bytes))
    }
}

impl fmt::Display for Pin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, byte) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str(":")?;
            }
            write!(f, "{byte:02X}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Pin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Pin({self})")
    }
}

#[derive(Debug)]
struct Pinned {
    pins: Vec<Pin>,
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if self.pins.contains(&Pin::of(end_entity.as_ref())) {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

pub(crate) fn config(trust: Option<&Trust>) -> Result<ClientConfig, Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier: Arc<dyn ServerCertVerifier> = match trust {
        None => pinned(&provider, Vec::new()),
        Some(Trust::Pinned(pins)) if pins.is_empty() => {
            return Err(Error::invalid("no certificate is pinned"));
        }
        Some(Trust::Pinned(pins)) => pinned(&provider, pins.clone()),
        Some(Trust::System) => Arc::new(
            rustls_platform_verifier::Verifier::new(provider.clone()).map_err(|error| {
                Error::invalid(format!("the system roots are not available: {error}"))
            })?,
        ),
    };
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| Error::invalid(format!("TLS cannot be configured: {error}")))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    config.alpn_protocols = vec![ALPN.to_vec()];
    Ok(config)
}

fn pinned(provider: &CryptoProvider, pins: Vec<Pin>) -> Arc<dyn ServerCertVerifier> {
    Arc::new(Pinned {
        pins,
        algorithms: provider.signature_verification_algorithms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DER: &[u8] = b"not a real certificate, only bytes to digest";

    #[test]
    fn a_pin_reads_the_fingerprint_the_installer_prints_with_or_without_colons() {
        let pin = Pin::of(DER);
        let printed = pin.to_string();
        assert_eq!(printed.len(), 32 * 3 - 1);
        assert!(
            printed
                .bytes()
                .all(|byte| byte == b':' || byte.is_ascii_hexdigit())
        );
        assert_eq!(printed.parse::<Pin>().unwrap(), pin);
        assert_eq!(printed.to_lowercase().parse::<Pin>().unwrap(), pin);
        assert_eq!(printed.replace(':', "").parse::<Pin>().unwrap(), pin);
        assert_eq!(format!(" {printed} ").parse::<Pin>().unwrap(), pin);
        assert_eq!(format!("{pin:?}"), format!("Pin({printed})"));
        for refused in [
            String::new(),
            printed[..printed.len() - 3].to_owned(),
            format!("{printed}:00"),
            printed.replace('A', "G"),
            format!("sha256:{printed}"),
        ] {
            assert!(
                matches!(refused.parse::<Pin>(), Err(Error::Invalid(_))),
                "{refused}"
            );
        }
    }

    #[test]
    fn a_pem_pins_every_certificate_it_holds_and_one_without_a_certificate_is_refused() {
        let first = rcgen::generate_simple_self_signed(vec!["first.test".to_owned()])
            .unwrap()
            .cert;
        let second = rcgen::generate_simple_self_signed(vec!["second.test".to_owned()])
            .unwrap()
            .cert;
        let pem = format!("{}{}", first.pem(), second.pem());
        assert_eq!(
            Pin::from_pem(pem.as_bytes()).unwrap(),
            [Pin::of(first.der()), Pin::of(second.der())]
        );
        assert!(matches!(
            Pin::from_pem(b"no certificate here"),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            Pin::from_pem(b"-----BEGIN CERTIFICATE-----\n!!!!\n-----END CERTIFICATE-----\n"),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn an_empty_pin_list_is_refused_and_a_plain_base_gets_a_config_that_trusts_nothing() {
        assert!(matches!(
            config(Some(&Trust::Pinned(Vec::new()))),
            Err(Error::Invalid(_))
        ));
        let plain = config(None).unwrap();
        assert_eq!(plain.alpn_protocols, vec![ALPN.to_vec()]);
        let pinned = config(Some(&Trust::Pinned(vec![Pin::of(DER)]))).unwrap();
        assert_eq!(pinned.alpn_protocols, vec![ALPN.to_vec()]);
        let verifier = Pinned {
            pins: vec![Pin::of(DER)],
            algorithms: rustls::crypto::ring::default_provider().signature_verification_algorithms,
        };
        let name = ServerName::try_from("keel.test").unwrap();
        assert!(
            verifier
                .verify_server_cert(&CertificateDer::from(DER), &[], &name, &[], UnixTime::now())
                .is_ok()
        );
        assert!(
            verifier
                .verify_server_cert(
                    &CertificateDer::from(&b"another certificate"[..]),
                    &[],
                    &name,
                    &[],
                    UnixTime::now()
                )
                .is_err()
        );
    }
}
