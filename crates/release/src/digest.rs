use std::fmt;

use sha2::{Digest as _, Sha256};

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Digest([u8; 32]);

#[derive(Default)]
pub struct Hasher(Sha256);

impl Digest {
    pub fn of(bytes: &[u8]) -> Self {
        let mut hasher = Hasher::default();
        hasher.update(bytes);
        hasher.finish()
    }

    pub fn hex(&self) -> String {
        self.to_string()
    }
}

impl Hasher {
    pub fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    pub fn finish(self) -> Digest {
        Digest(self.0.finalize().into())
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Digest({self})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_digest_is_the_lowercase_hex_of_sha256() {
        assert_eq!(
            Digest::of(b"abc").hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let mut hasher = Hasher::default();
        hasher.update(b"a");
        hasher.update(b"bc");
        assert_eq!(hasher.finish(), Digest::of(b"abc"));
        assert_eq!(
            format!("{:?}", Digest::of(b"")),
            "Digest(e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855)"
        );
    }
}
