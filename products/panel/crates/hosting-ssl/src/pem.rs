use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use keel::prelude::*;
use keel::sdk::chrono::{DateTime, Utc};
use rustls::pki_types::{
    CertificateDer, PrivateKeyDer, PrivatePkcs1KeyDer, PrivatePkcs8KeyDer, PrivateSec1KeyDer,
};
use x509_parser::extensions::GeneralName;
use x509_parser::prelude::{FromDer, X509Certificate};

const CERTIFICATE: &str = "CERTIFICATE";
const PKCS8: &str = "PRIVATE KEY";
const PKCS1: &str = "RSA PRIVATE KEY";
const SEC1: &str = "EC PRIVATE KEY";
const BEGIN: &str = "-----BEGIN ";
const DASHES: &str = "-----";
const LINE: usize = 64;
const PEM: &str = "hosting.certificate.error.pem";
const UNREADABLE: &str = "hosting.certificate.error.certificate";
const NAMES: &str = "hosting.certificate.error.names";
const EXPIRED: &str = "hosting.certificate.error.expired";
const KEY: &str = "hosting.certificate.error.key";
const PAIR: &str = "hosting.certificate.error.pair";

pub(crate) struct Material {
    pub(crate) chain: String,
    pub(crate) key: Secret,
    pub(crate) not_after: DateTime<Utc>,
}

impl Material {
    pub(crate) fn issued(chain: String, key: String) -> Result<Self> {
        let leaf = certificates(&chain, "certificate")?
            .into_iter()
            .next()
            .ok_or_else(|| KeelError::internal(UNREADABLE).param("detail", "no certificate"))
            .and_then(|der| {
                parse(&der)
                    .map_err(|detail| KeelError::internal(UNREADABLE).param("detail", detail))
            })?;
        Ok(Self {
            chain,
            key: Secret::new(key),
            not_after: leaf.not_after,
        })
    }

    pub(crate) fn uploaded(
        certificate: &str,
        key: &str,
        names: &[String],
        now: DateTime<Utc>,
    ) -> Result<Self> {
        let chain = certificates(certificate, "certificate")?;
        let leaf = chain
            .first()
            .ok_or_else(|| {
                KeelError::validation(PEM)
                    .field("certificate")
                    .param("detail", "no CERTIFICATE block")
            })
            .and_then(|der| {
                parse(der).map_err(|detail| {
                    KeelError::validation(UNREADABLE)
                        .field("certificate")
                        .param("detail", detail)
                })
            })?;
        if let Some(missing) = names.iter().find(|name| !covers(&leaf.names, name)) {
            return Err(KeelError::validation(NAMES)
                .field("certificate")
                .param("name", missing.as_str()));
        }
        if leaf.not_after <= now {
            return Err(KeelError::validation(EXPIRED)
                .field("certificate")
                .param("notAfter", leaf.not_after.to_rfc3339()));
        }
        let (label, der) = private_key(key)?;
        let private = match label.as_str() {
            PKCS8 => PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(der.clone())),
            PKCS1 => PrivateKeyDer::Pkcs1(PrivatePkcs1KeyDer::from(der.clone())),
            _ => PrivateKeyDer::Sec1(PrivateSec1KeyDer::from(der.clone())),
        };
        check_pair(
            chain.iter().cloned().map(CertificateDer::from).collect(),
            private,
        )?;
        Ok(Self {
            chain: chain
                .iter()
                .map(|der| encode(CERTIFICATE, der))
                .collect::<String>(),
            key: Secret::new(encode(&label, &der)),
            not_after: leaf.not_after,
        })
    }
}

struct Leaf {
    not_after: DateTime<Utc>,
    names: Vec<String>,
}

struct Block {
    label: String,
    der: Vec<u8>,
}

fn certificates(text: &str, field: &str) -> Result<Vec<Vec<u8>>> {
    blocks(text, field)?
        .into_iter()
        .map(|block| {
            if block.label == CERTIFICATE {
                Ok(block.der)
            } else {
                Err(KeelError::validation(PEM)
                    .field(field)
                    .param("detail", format!("a {} block", block.label)))
            }
        })
        .collect()
}

fn private_key(text: &str) -> Result<(String, Vec<u8>)> {
    let mut blocks = blocks(text, "key")?.into_iter();
    match (blocks.next(), blocks.next()) {
        (Some(block), None) if [PKCS8, PKCS1, SEC1].contains(&block.label.as_str()) => {
            Ok((block.label, block.der))
        }
        _ => Err(KeelError::validation(KEY).field("key")),
    }
}

fn blocks(text: &str, field: &str) -> Result<Vec<Block>> {
    let refused = |detail: String| {
        KeelError::validation(PEM)
            .field(field)
            .param("detail", detail)
    };
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(BEGIN) {
        let after = &rest[start + BEGIN.len()..];
        let end = after
            .find(DASHES)
            .ok_or_else(|| refused("an unterminated BEGIN line".to_owned()))?;
        let label = &after[..end];
        if label.is_empty()
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte == b' ')
        {
            return Err(refused("a block without a plain label".to_owned()));
        }
        let body = &after[end + DASHES.len()..];
        let footer = format!("-----END {label}-----");
        let stop = body
            .find(&footer)
            .ok_or_else(|| refused(format!("no END line for {label}")))?;
        let text: String = body[..stop]
            .chars()
            .filter(|character| !character.is_ascii_whitespace())
            .collect();
        let der = STANDARD
            .decode(text.as_bytes())
            .map_err(|error| refused(format!("{label}: {error}")))?;
        found.push(Block {
            label: label.to_owned(),
            der,
        });
        rest = &body[stop + footer.len()..];
    }
    Ok(found)
}

fn encode(label: &str, der: &[u8]) -> String {
    let body = STANDARD.encode(der);
    let mut text = format!("-----BEGIN {label}-----\n");
    for line in body.as_bytes().chunks(LINE) {
        text.push_str(&String::from_utf8_lossy(line));
        text.push('\n');
    }
    text.push_str(&format!("-----END {label}-----\n"));
    text
}

fn parse(der: &[u8]) -> std::result::Result<Leaf, String> {
    let (_, certificate) = X509Certificate::from_der(der).map_err(|error| error.to_string())?;
    let stamp = certificate.validity().not_after.timestamp();
    let not_after = DateTime::from_timestamp(stamp, 0)
        .ok_or_else(|| format!("the expiry {stamp} is out of range"))?;
    let names = match certificate.subject_alternative_name() {
        Ok(Some(extension)) => extension
            .value
            .general_names
            .iter()
            .filter_map(|name| match name {
                GeneralName::DNSName(name) => Some(name.to_ascii_lowercase()),
                _ => None,
            })
            .collect(),
        Ok(None) => Vec::new(),
        Err(error) => return Err(error.to_string()),
    };
    Ok(Leaf { not_after, names })
}

fn covers(names: &[String], name: &str) -> bool {
    names.iter().any(|pattern| {
        pattern == name
            || pattern.strip_prefix("*.").is_some_and(|parent| {
                name.split_once('.')
                    .is_some_and(|(label, rest)| !label.is_empty() && rest == parent)
            })
    })
}

fn check_pair(chain: Vec<CertificateDer<'static>>, key: PrivateKeyDer<'static>) -> Result<()> {
    let refused = |detail: String| {
        KeelError::validation(PAIR)
            .field("key")
            .param("detail", detail)
    };
    rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|error| refused(error.to_string()))?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .map(drop)
        .map_err(|error| refused(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use keel::sdk::chrono::Duration;

    fn pair(names: &[&str]) -> (String, String) {
        let names: Vec<String> = names.iter().map(|name| (*name).to_owned()).collect();
        let certified = rcgen::generate_simple_self_signed(names).unwrap();
        (certified.cert.pem(), certified.signing_key.serialize_pem())
    }

    fn flattened(pem: &str) -> String {
        pem.replace('\n', "")
    }

    #[test]
    fn an_uploaded_pair_is_normalised_even_when_its_line_breaks_were_lost() {
        let (certificate, key) = pair(&["example.com", "*.example.com"]);
        let names = ["example.com".to_owned(), "www.example.com".to_owned()];
        let material =
            Material::uploaded(&certificate, &flattened(&key), &names, Utc::now()).unwrap();
        assert_eq!(material.chain, certificate.replace("\r\n", "\n"));
        assert!(
            material
                .key
                .expose()
                .starts_with("-----BEGIN PRIVATE KEY-----\n")
        );
        assert!(material.key.expose().lines().all(|line| line.len() <= LINE));
        assert!(material.not_after > Utc::now());
    }

    #[test]
    fn an_uploaded_pair_that_misses_a_name_is_expired_or_mismatched_is_refused_on_its_field() {
        let (certificate, key) = pair(&["example.com"]);
        let (_, other) = pair(&["example.com"]);
        let missing = Material::uploaded(
            &certificate,
            &key,
            &["example.com".to_owned(), "www.example.com".to_owned()],
            Utc::now(),
        )
        .err()
        .unwrap();
        assert_eq!(
            (missing.key(), missing.field_path()),
            (NAMES, Some("certificate"))
        );
        let names = ["example.com".to_owned()];
        let expired = Material::uploaded(
            &certificate,
            &key,
            &names,
            Utc::now() + Duration::days(365 * 5000),
        )
        .err()
        .unwrap();
        assert_eq!(expired.key(), EXPIRED);
        let mismatched = Material::uploaded(&certificate, &other, &names, Utc::now())
            .err()
            .unwrap();
        assert_eq!(
            (mismatched.key(), mismatched.field_path()),
            (PAIR, Some("key"))
        );
        let no_key = Material::uploaded(&certificate, &certificate, &names, Utc::now())
            .err()
            .unwrap();
        assert_eq!((no_key.key(), no_key.field_path()), (KEY, Some("key")));
        let garbage = Material::uploaded(
            "-----BEGIN CERTIFICATE-----@@-----END CERTIFICATE-----",
            &key,
            &names,
            Utc::now(),
        )
        .err()
        .unwrap();
        assert_eq!(
            (garbage.key(), garbage.field_path()),
            (PEM, Some("certificate"))
        );
    }

    #[test]
    fn a_wildcard_covers_one_label_and_nothing_else() {
        let names = ["*.example.com".to_owned()];
        assert!(covers(&names, "www.example.com"));
        assert!(!covers(&names, "example.com"));
        assert!(!covers(&names, "a.b.example.com"));
        assert!(!covers(&names, ".example.com"));
    }

    #[test]
    fn an_issued_chain_answers_the_expiry_of_its_leaf() {
        let (certificate, key) = pair(&["example.com"]);
        let material = Material::issued(certificate.clone(), key).unwrap();
        assert_eq!(material.chain, certificate);
        assert!(material.not_after > Utc::now());
        assert_eq!(
            Material::issued(String::new(), String::new())
                .err()
                .unwrap()
                .key(),
            UNREADABLE
        );
    }
}
