use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use chrono::{DateTime, Days, SubsecRound, Utc};
use keel_release::{PublicKey, ReleaseError, SecretKey, Signed};
use keel_types::Id;
use serde::Serialize;

const SECRET_FILE: &str = "license.key";
const PUBLIC_FILE: &str = "license.pub";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Key(#[from] ReleaseError),
    #[error("could not create {path}: {source}")]
    CreateDir { path: PathBuf, source: io::Error },
    #[error("could not write {path}: {source}")]
    Write { path: PathBuf, source: io::Error },
    #[error("the product {0} is listed twice")]
    Twice(String),
    #[error("{days} days from {from} end beyond the calendar")]
    Calendar { days: u32, from: DateTime<Utc> },
    #[error("the lease could not be serialised: {0}")]
    Serialise(serde_json::Error),
}

#[derive(Debug, thiserror::Error)]
#[error("\"{0}\" is not <product>:<edition>")]
pub struct BadProduct(String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Product {
    id: String,
    edition: String,
}

impl FromStr for Product {
    type Err = BadProduct;

    fn from_str(text: &str) -> Result<Self, BadProduct> {
        let bad = || BadProduct(text.to_owned());
        let (id, edition) = text.split_once(':').ok_or_else(bad)?;
        let word =
            |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_graphic() && c != ':');
        if !word(id) || !word(edition) {
            return Err(bad());
        }
        Ok(Self {
            id: id.to_owned(),
            edition: edition.to_owned(),
        })
    }
}

impl fmt::Display for Product {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.id, self.edition)
    }
}

pub struct Terms {
    pub instance: Id,
    pub products: Vec<Product>,
    pub days: u32,
    pub grace: u32,
}

#[derive(Serialize)]
struct Lease<'a> {
    instance_id: Id,
    issued_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    grace_days: u32,
    products: &'a [Product],
}

pub struct Keys {
    secret: PathBuf,
    public: PathBuf,
    key: PublicKey,
}

impl fmt::Display for Keys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "secret key: {}", self.secret.display())?;
        writeln!(f, "public key: {}", self.public.display())?;
        write!(f, "{}", self.key)
    }
}

pub struct Issued {
    out: PathBuf,
    instance: Id,
    products: Vec<Product>,
    expires_at: DateTime<Utc>,
    grace: u32,
}

impl fmt::Display for Issued {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let products = self
            .products
            .iter()
            .map(Product::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        write!(
            f,
            "lease for {} ({products}) valid until {} with {} days of grace, written to {}",
            self.instance,
            self.expires_at.to_rfc3339(),
            self.grace,
            self.out.display()
        )
    }
}

pub fn keygen(out: &Path, comment: &str) -> Result<Keys, Error> {
    fs::create_dir_all(out).map_err(|source| Error::CreateDir {
        path: out.to_owned(),
        source,
    })?;
    let secret = SecretKey::generate(comment)?;
    let keys = Keys {
        secret: out.join(SECRET_FILE),
        public: out.join(PUBLIC_FILE),
        key: secret.public(),
    };
    secret.write(&keys.secret)?;
    keys.key.write(&keys.public)?;
    Ok(keys)
}

pub fn issue(key: &Path, terms: Terms, now: DateTime<Utc>, out: &Path) -> Result<Issued, Error> {
    let secret = SecretKey::read(key)?;
    let expires_at = expiry(now, terms.days)?;
    let signed = sign(&secret, &terms, now, expires_at)?;
    fs::write(out, signed.as_bytes()).map_err(|source| Error::Write {
        path: out.to_owned(),
        source,
    })?;
    Ok(Issued {
        out: out.to_owned(),
        instance: terms.instance,
        expires_at,
        grace: terms.grace,
        products: terms.products,
    })
}

fn expiry(now: DateTime<Utc>, days: u32) -> Result<DateTime<Utc>, Error> {
    now.trunc_subsecs(0)
        .checked_add_days(Days::new(u64::from(days)))
        .ok_or(Error::Calendar { days, from: now })
}

fn sign(
    secret: &SecretKey,
    terms: &Terms,
    now: DateTime<Utc>,
    expires_at: DateTime<Utc>,
) -> Result<String, Error> {
    for (index, product) in terms.products.iter().enumerate() {
        if terms.products[..index]
            .iter()
            .any(|earlier| earlier.id == product.id)
        {
            return Err(Error::Twice(product.id.clone()));
        }
    }
    let lease = Lease {
        instance_id: terms.instance,
        issued_at: now.trunc_subsecs(0),
        expires_at,
        grace_days: terms.grace,
        products: &terms.products,
    };
    let payload = serde_json::to_vec_pretty(&lease).map_err(Error::Serialise)?;
    Ok(Signed::seal(payload, secret).to_json()?)
}

#[cfg(test)]
mod tests {
    use keel_release::KeyRing;
    use serde_json::{Value, json};

    use super::*;

    fn terms(products: &[&str]) -> Terms {
        Terms {
            instance: "01J9ZQ3V6K8M2N4P5R7S9T1V3W".parse().unwrap(),
            products: products.iter().map(|text| text.parse().unwrap()).collect(),
            days: 365,
            grace: 30,
        }
    }

    #[test]
    fn a_product_is_written_as_id_and_edition() {
        let product: Product = "dns:pro".parse().unwrap();
        assert_eq!(product.to_string(), "dns:pro");
        for bad in ["dns", "dns:", ":pro", "dns:pro:x", "d ns:pro", ""] {
            assert!(bad.parse::<Product>().is_err(), "{bad} was accepted");
        }
    }

    #[test]
    fn the_signed_lease_carries_the_terms_and_opens_with_the_public_half() {
        let secret = SecretKey::generate("test").unwrap();
        let now: DateTime<Utc> = "2026-09-25T10:11:12.345Z".parse().unwrap();
        let signed = sign(
            &secret,
            &terms(&["dns:pro", "panel:lite"]),
            now,
            expiry(now, 365).unwrap(),
        )
        .unwrap();
        let ring = KeyRing::new(secret.public(), Vec::new());
        let payload = Signed::parse(signed.as_bytes())
            .unwrap()
            .open(&ring)
            .unwrap()
            .to_vec();
        let lease: Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(
            lease,
            json!({
                "instance_id": "01J9ZQ3V6K8M2N4P5R7S9T1V3W",
                "issued_at": "2026-09-25T10:11:12Z",
                "expires_at": "2027-09-25T10:11:12Z",
                "grace_days": 30,
                "products": [
                    { "id": "dns", "edition": "pro" },
                    { "id": "panel", "edition": "lite" },
                ],
            })
        );
        let other = KeyRing::new(SecretKey::generate("other").unwrap().public(), Vec::new());
        assert!(
            Signed::parse(signed.as_bytes())
                .unwrap()
                .open(&other)
                .is_err()
        );
    }

    #[test]
    fn a_product_listed_twice_is_refused() {
        let secret = SecretKey::generate("test").unwrap();
        let now = Utc::now();
        let error = sign(
            &secret,
            &terms(&["dns:pro", "dns:lite"]),
            now,
            expiry(now, 365).unwrap(),
        )
        .unwrap_err();
        assert!(
            matches!(error, Error::Twice(ref id) if id == "dns"),
            "{error}"
        );
    }

    #[test]
    fn keygen_never_overwrites_a_key() {
        let dir = tempfile::tempdir().unwrap();
        let keys = keygen(dir.path(), "test").unwrap();
        assert_eq!(
            PublicKey::read(&dir.path().join(PUBLIC_FILE)).unwrap(),
            keys.key
        );
        assert!(matches!(
            keygen(dir.path(), "test"),
            Err(Error::Key(ReleaseError::Exists { .. }))
        ));
    }
}
