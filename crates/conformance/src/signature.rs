use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use crate::report::{Failure, ensure, fail};

pub const HEADER: &str = "x-keel-signature";
pub const WINDOW_SECONDS: i64 = 300;

pub fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

pub fn within(t: i64, now: i64) -> bool {
    now.abs_diff(t) <= WINDOW_SECONDS.unsigned_abs()
}

fn digest(secret: &[u8], t: i64, body: &[u8]) -> Result<Hmac<Sha256>, Failure> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret)
        .map_err(|_| fail("the secret cannot key the signature"))?;
    mac.update(t.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    Ok(mac)
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn sign(secret: &[u8], t: i64, body: &[u8]) -> Result<String, Failure> {
    let code = digest(secret, t, body)?.finalize().into_bytes();
    Ok(format!("t={t},v1={}", hex(&code)))
}

pub fn parse(header: &str) -> Result<(i64, Vec<u8>), Failure> {
    let mut t = None;
    let mut v1 = None;
    for part in header.split(',') {
        let (name, value) = part
            .trim()
            .split_once('=')
            .ok_or_else(|| fail(format!("the signature part {part} is not name=value")))?;
        match name {
            "t" => {
                t = Some(
                    value
                        .parse::<i64>()
                        .map_err(|_| fail(format!("t is not a number: {value}")))?,
                );
            }
            "v1" => v1 = Some(unhex(value).ok_or_else(|| fail("v1 is not hex"))?),
            _ => {}
        }
    }
    match (t, v1) {
        (Some(t), Some(v1)) => Ok((t, v1)),
        _ => Err(fail(format!("the signature lacks t or v1: {header}"))),
    }
}

pub fn verify(secret: &[u8], header: &str, body: &[u8], now: i64) -> Result<(), Failure> {
    let (t, v1) = parse(header)?;
    ensure(within(t, now), || {
        format!("the signature time {t} is more than {WINDOW_SECONDS} seconds from {now}")
    })?;
    digest(secret, t, body)?
        .verify_slice(&v1)
        .map_err(|_| fail("the signature does not match the body"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signature_the_suite_makes_verifies_within_the_window_and_nothing_else_does() {
        let body = br#"{"id":"01J9EV5K"}"#;
        let now = 1_757_671_200;
        let header = sign(b"whs_secret", now, body).unwrap();
        assert!(header.starts_with(&format!("t={now},v1=")), "{header}");
        assert_eq!(parse(&header).unwrap().1.len(), 32);
        verify(b"whs_secret", &header, body, now).unwrap();
        verify(b"whs_secret", &header, body, now + WINDOW_SECONDS).unwrap();
        assert!(verify(b"whs_secret", &header, body, now + WINDOW_SECONDS + 1).is_err());
        assert!(verify(b"whs_other", &header, body, now).is_err());
        assert!(verify(b"whs_secret", &header, br#"{"id":"01J9EV5L"}"#, now).is_err());
        assert!(parse("t=1").is_err());
        assert!(parse("t=x,v1=00").is_err());
        assert!(parse("t=1,v1=0").is_err());
        assert_eq!(hex(&[0x00, 0xab, 0x10]), "00ab10");
    }

    #[test]
    fn a_signature_time_at_the_ends_of_the_range_is_refused_without_overflow() {
        let now = 1_757_671_200;
        assert!(within(now, now));
        assert!(within(now - WINDOW_SECONDS, now));
        assert!(!within(now + WINDOW_SECONDS + 1, now));
        for t in [i64::MIN, i64::MIN + 1, i64::MAX] {
            assert!(!within(t, now), "{t}");
            let header = format!("t={t},v1={}", "00".repeat(32));
            assert!(verify(b"whs_secret", &header, b"{}", now).is_err(), "{t}");
        }
        assert!(within(i64::MAX, i64::MAX));
        assert!(!within(i64::MIN, i64::MAX));
    }
}
