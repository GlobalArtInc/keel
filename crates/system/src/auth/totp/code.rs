use hmac::{Hmac, KeyInit, Mac};
use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::{KeelError, Result};
use sha1::Sha1;
use subtle::ConstantTimeEq;

use super::super::password::random_bytes;

pub const STEP_SECONDS: i64 = 30;
pub const DIGITS: u32 = 6;
pub const WINDOW: i64 = 1;
pub const SECRET_LEN: usize = 20;
pub const RECOVERY_CODES: usize = 10;
pub const RECOVERY_CODE_LEN: usize = 10;
pub const ISSUER: &str = "keel";

const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

pub fn step_at(at: DateTime<Utc>) -> i64 {
    at.timestamp().div_euclid(STEP_SECONDS)
}

// https://www.rfc-editor.org/rfc/rfc4226#section-5.3
fn hotp(secret: &[u8], counter: u64, digits: u32) -> Result<String> {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret)
        .map_err(|_| KeelError::internal("auth.error.totp_secret"))?;
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = usize::from(digest[digest.len() - 1] & 0x0f);
    let binary = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    let width = digits as usize;
    Ok(format!("{:0width$}", binary % 10u32.pow(digits)))
}

// https://www.rfc-editor.org/rfc/rfc6238#section-4
pub fn code(secret: &[u8], step: i64) -> Result<String> {
    hotp(secret, u64::try_from(step).unwrap_or_default(), DIGITS)
}

pub fn matching_step(secret: &[u8], submitted: &str, current: i64) -> Result<Option<i64>> {
    let mut found = None;
    for step in (current - WINDOW)..=(current + WINDOW) {
        let expected = code(secret, step)?;
        let same: bool = expected.as_bytes().ct_eq(submitted.as_bytes()).into();
        if same && (found.is_none() || step == current) {
            found = Some(step);
        }
    }
    Ok(found)
}

pub fn is_code(value: &str) -> bool {
    value.len() == DIGITS as usize && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub fn generate_secret() -> Result<Vec<u8>> {
    random_bytes(SECRET_LEN)
}

pub fn encode_secret(secret: &[u8]) -> String {
    base32::encode(base32::Alphabet::Rfc4648 { padding: false }, secret)
}

pub fn decode_secret(text: &str) -> Option<Vec<u8>> {
    base32::decode(base32::Alphabet::Rfc4648 { padding: false }, text)
}

pub fn uri(login: &str, secret: &str) -> String {
    format!(
        "otpauth://totp/{ISSUER}:{}?secret={secret}&issuer={ISSUER}&algorithm=SHA1&digits={DIGITS}&period={STEP_SECONDS}",
        percent_encode(login)
    )
}

fn percent_encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

pub fn generate_recovery_codes() -> Result<Vec<String>> {
    let mut codes = Vec::with_capacity(RECOVERY_CODES);
    for _ in 0..RECOVERY_CODES {
        let bytes = random_bytes(RECOVERY_CODE_LEN)?;
        codes.push(
            bytes
                .iter()
                .map(|byte| char::from(ALPHABET[usize::from(byte & 0x1f)]))
                .collect(),
        );
    }
    Ok(codes)
}

pub fn normalise_recovery_code(submitted: &str) -> String {
    submitted.trim().to_ascii_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RFC_SECRET: &[u8] = b"12345678901234567890";

    #[test]
    fn the_rfc_4226_and_rfc_6238_test_vectors_pass_against_the_generator() {
        let hotp_vectors = [
            "755224", "287082", "359152", "969429", "338314", "254676", "287922", "162583",
            "399871", "520489",
        ];
        for (counter, expected) in hotp_vectors.iter().enumerate() {
            assert_eq!(
                hotp(RFC_SECRET, counter as u64, DIGITS).expect("a code"),
                *expected
            );
        }
        let totp_vectors: [(i64, &str); 6] = [
            (59, "94287082"),
            (1_111_111_109, "07081804"),
            (1_111_111_111, "14050471"),
            (1_234_567_890, "89005924"),
            (2_000_000_000, "69279037"),
            (20_000_000_000, "65353130"),
        ];
        for (seconds, expected) in totp_vectors {
            let at = DateTime::from_timestamp(seconds, 0).expect("an instant");
            let step = step_at(at);
            assert_eq!(step, seconds / STEP_SECONDS);
            assert_eq!(hotp(RFC_SECRET, step as u64, 8).expect("a code"), expected);
            assert_eq!(code(RFC_SECRET, step).expect("a code"), &expected[2..]);
        }
    }

    #[test]
    fn a_code_matches_its_own_step_first_then_a_neighbour_and_nothing_further() {
        let current = 1_111_111_111 / STEP_SECONDS;
        let now = code(RFC_SECRET, current).expect("a code");
        let before = code(RFC_SECRET, current - 1).expect("a code");
        let after = code(RFC_SECRET, current + 1).expect("a code");
        let far = code(RFC_SECRET, current + 2).expect("a code");
        assert_eq!(
            matching_step(RFC_SECRET, &now, current).expect("checked"),
            Some(current)
        );
        assert_eq!(
            matching_step(RFC_SECRET, &before, current).expect("checked"),
            Some(current - 1)
        );
        assert_eq!(
            matching_step(RFC_SECRET, &after, current).expect("checked"),
            Some(current + 1)
        );
        assert_eq!(
            matching_step(RFC_SECRET, &far, current).expect("checked"),
            None
        );
        assert_eq!(
            matching_step(RFC_SECRET, "000000", current).expect("checked"),
            None
        );
        assert!(is_code("123456"));
        assert!(!is_code("12345"));
        assert!(!is_code("12345a"));
        assert!(!is_code("ABCDEFGHIJ"));
    }

    #[test]
    fn a_secret_is_twenty_random_bytes_shown_as_base32_inside_an_otpauth_uri() {
        let secret = generate_secret().expect("entropy");
        assert_eq!(secret.len(), SECRET_LEN);
        assert_ne!(secret, generate_secret().expect("entropy"));
        let encoded = encode_secret(&secret);
        assert_eq!(encoded.len(), 32);
        assert!(encoded.bytes().all(|byte| ALPHABET.contains(&byte)));
        assert_eq!(decode_secret(&encoded).expect("decodes"), secret);
        assert_eq!(
            uri("a l/ice", "JBSWY3DPEHPK3PXP"),
            "otpauth://totp/keel:a%20l%2Fice?secret=JBSWY3DPEHPK3PXP&issuer=keel&algorithm=SHA1&digits=6&period=30"
        );
    }

    #[test]
    fn recovery_codes_are_ten_codes_of_ten_base32_characters_normalised_to_upper_case() {
        let codes = generate_recovery_codes().expect("entropy");
        assert_eq!(codes.len(), RECOVERY_CODES);
        for code in &codes {
            assert_eq!(code.len(), RECOVERY_CODE_LEN);
            assert!(code.bytes().all(|byte| ALPHABET.contains(&byte)), "{code}");
            assert!(!is_code(code));
        }
        let unique: std::collections::BTreeSet<&String> = codes.iter().collect();
        assert_eq!(unique.len(), RECOVERY_CODES);
        assert_eq!(
            normalise_recovery_code(&format!(" {} ", codes[0].to_ascii_lowercase())),
            codes[0]
        );
    }
}
