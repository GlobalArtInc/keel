use keel_sdk::{KeelError, Result, Secret};
use rand::TryRng;
use rand::rngs::SysRng;

use crate::auth::api::token_hash;

pub const CODE_PREFIX: &str = "PAIR-";
pub const CODE_SYMBOLS: usize = 8;
pub const ERROR_ENTROPY: &str = "peer.error.entropy";
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const GROUP: usize = 4;

pub fn generate() -> Result<Secret> {
    let mut bytes = [0u8; CODE_SYMBOLS];
    SysRng.try_fill_bytes(&mut bytes).map_err(|error| {
        KeelError::internal(ERROR_ENTROPY).source(std::io::Error::other(error.to_string()))
    })?;
    let symbols: Vec<char> = bytes
        .iter()
        .map(|byte| char::from(ALPHABET[usize::from(byte % 32)]))
        .collect();
    let groups: Vec<String> = symbols
        .chunks(GROUP)
        .map(|group| group.iter().collect())
        .collect();
    Ok(Secret::new(format!("{CODE_PREFIX}{}", groups.join("-"))))
}

pub fn hash(code: &str) -> String {
    token_hash(&code.trim().to_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_is_pair_and_two_groups_of_four_unambiguous_symbols_hashed_without_case() {
        let code = generate().expect("entropy");
        let text = code.expose();
        assert_eq!(text.len(), CODE_PREFIX.len() + CODE_SYMBOLS + 1);
        let rest = text.strip_prefix(CODE_PREFIX).expect("the prefix");
        let (first, second) = rest.split_once('-').expect("two groups");
        assert_eq!((first.len(), second.len()), (GROUP, GROUP));
        for symbol in first.bytes().chain(second.bytes()) {
            assert!(ALPHABET.contains(&symbol), "{text}");
            assert!(!b"ILOU".contains(&symbol), "{text}");
        }
        assert_ne!(generate().expect("entropy").expose(), text);
        assert_eq!(
            hash(text),
            hash(&format!("  {}\n", text.to_ascii_lowercase()))
        );
        assert_eq!(hash(text).len(), 64);
        assert_ne!(hash(text), hash("PAIR-0000-0000"));
        assert_eq!(ALPHABET.len(), 32);
    }
}
