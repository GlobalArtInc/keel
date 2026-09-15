use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

pub const HEADER: &str = "x-keel-signature";
pub const EVENT_ID_HEADER: &str = "x-keel-event-id";
pub const VERSION: &str = "v1";
pub const WINDOW_SECONDS: i64 = 300;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Rejection {
    #[error("the signature header is not t=<seconds>,v1=<hex>")]
    Format,
    #[error("the signature time {t} is more than {WINDOW_SECONDS} seconds from {now}")]
    Stale { t: i64, now: i64 },
    #[error("the signature does not match the body")]
    Mismatch,
    #[error("the secret cannot key the signature")]
    Key,
}

fn mac(secret: &[u8], t: i64, body: &[u8]) -> Result<Hmac<Sha256>, Rejection> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).map_err(|_| Rejection::Key)?;
    mac.update(t.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    Ok(mac)
}

pub fn sign(secret: &[u8], t: i64, body: &[u8]) -> Result<String, Rejection> {
    let digest = mac(secret, t, body)?.finalize().into_bytes();
    Ok(format!("t={t},{VERSION}={}", hex(&digest)))
}

pub fn verify(secret: &[u8], header: &str, body: &[u8], now: i64) -> Result<(), Rejection> {
    let (t, digest) = parse(header)?;
    if (now - t).abs() > WINDOW_SECONDS {
        return Err(Rejection::Stale { t, now });
    }
    mac(secret, t, body)?
        .verify_slice(&digest)
        .map_err(|_| Rejection::Mismatch)
}

fn parse(header: &str) -> Result<(i64, Vec<u8>), Rejection> {
    let mut t = None;
    let mut digest = None;
    for part in header.split(',') {
        let (name, value) = part.trim().split_once('=').ok_or(Rejection::Format)?;
        match name {
            "t" => t = Some(value.parse::<i64>().map_err(|_| Rejection::Format)?),
            VERSION => digest = Some(unhex(value).ok_or(Rejection::Format)?),
            _ => {}
        }
    }
    match (t, digest) {
        (Some(t), Some(digest)) => Ok((t, digest)),
        _ => Err(Rejection::Format),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"whsec_0123456789abcdef0123456789abcdef";
    const BODY: &[u8] = br#"{"id":"01J9EV5K00000000000000000A","type":"note.saved"}"#;

    #[test]
    fn a_signature_verifies_through_verify_slice_and_a_stale_or_tampered_one_is_refused() {
        let header = sign(SECRET, 1_757_671_200, BODY).unwrap();
        assert!(header.starts_with("t=1757671200,v1="));
        assert_eq!(header.len(), "t=1757671200,v1=".len() + 64);
        assert_eq!(verify(SECRET, &header, BODY, 1_757_671_200), Ok(()));
        assert_eq!(verify(SECRET, &header, BODY, 1_757_671_500), Ok(()));
        assert_eq!(verify(SECRET, &header, BODY, 1_757_670_900), Ok(()));
        assert_eq!(
            verify(SECRET, &header, BODY, 1_757_671_501),
            Err(Rejection::Stale {
                t: 1_757_671_200,
                now: 1_757_671_501
            })
        );
        assert_eq!(
            verify(SECRET, &header, BODY, 1_757_670_899),
            Err(Rejection::Stale {
                t: 1_757_671_200,
                now: 1_757_670_899
            })
        );
        let mut tampered = BODY.to_vec();
        tampered[10] ^= 1;
        assert_eq!(
            verify(SECRET, &header, &tampered, 1_757_671_200),
            Err(Rejection::Mismatch)
        );
        assert_eq!(
            verify(b"another secret", &header, BODY, 1_757_671_200),
            Err(Rejection::Mismatch)
        );
        let later = sign(SECRET, 1_757_671_260, BODY).unwrap();
        assert_ne!(later, header, "a new t gives a new v1");
        assert_eq!(verify(SECRET, &later, BODY, 1_757_671_260), Ok(()));
        let (digest, _) = header.split_once(",v1=").map(|(_, v)| (v, ())).unwrap();
        let mut flipped = digest.to_owned();
        flipped.replace_range(0..1, if digest.starts_with('0') { "1" } else { "0" });
        assert_eq!(
            verify(
                SECRET,
                &format!("t=1757671200,v1={flipped}"),
                BODY,
                1_757_671_200
            ),
            Err(Rejection::Mismatch)
        );
        for broken in [
            "",
            "t=abc,v1=00",
            "t=1757671200",
            "v1=00",
            "t=1757671200,v1=zz",
            "t=1757671200,v1=0",
            "nonsense",
        ] {
            assert_eq!(
                verify(SECRET, broken, BODY, 1_757_671_200),
                Err(Rejection::Format),
                "{broken}"
            );
        }
        assert_eq!(
            Rejection::Stale { t: 1, now: 400 }.to_string(),
            "the signature time 1 is more than 300 seconds from 400"
        );
    }
}
