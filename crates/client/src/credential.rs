use std::fmt;

use http::header::{AUTHORIZATION, COOKIE};
use http::{HeaderMap, HeaderValue};
use keel_types::{EXTERNAL_MAX_LEN, IDEMPOTENCY_KEY_MAX_LEN, Id};

use crate::error::Error;
use crate::{
    IDEMPOTENCY_KEY, ON_BEHALF_OF, PEER_PREFIX, SESSION_COOKIE, SESSION_PREFIX, TOKEN_PREFIX,
};

const ORIGIN_NAME_MAX_LEN: usize = 64;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct Credential(Kind);

#[derive(Clone, Default, PartialEq, Eq)]
enum Kind {
    #[default]
    Guest,
    Token(String),
    Peer(String),
    Session(String),
}

pub(crate) fn is_visible(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'"' | b',' | b';' | b'\\'))
}

fn prefixed(prefix: &str, what: &str, value: String) -> Result<String, Error> {
    match value.strip_prefix(prefix) {
        Some(rest) if is_visible(rest) => Ok(value),
        _ => Err(Error::invalid(format!(
            "{what} is not {prefix} followed by visible characters"
        ))),
    }
}

impl Credential {
    pub fn guest() -> Self {
        Self(Kind::Guest)
    }

    pub fn token(value: impl Into<String>) -> Result<Self, Error> {
        prefixed(TOKEN_PREFIX, "an API token", value.into()).map(|value| Self(Kind::Token(value)))
    }

    pub fn peer(value: impl Into<String>) -> Result<Self, Error> {
        prefixed(PEER_PREFIX, "a peer token", value.into()).map(|value| Self(Kind::Peer(value)))
    }

    pub fn session(value: impl Into<String>) -> Result<Self, Error> {
        prefixed(SESSION_PREFIX, "a session", value.into()).map(|value| Self(Kind::Session(value)))
    }

    pub fn is_guest(&self) -> bool {
        self.0 == Kind::Guest
    }

    pub(crate) fn apply(&self, headers: &mut HeaderMap) -> Result<(), Error> {
        let (name, value) = match &self.0 {
            Kind::Guest => return Ok(()),
            Kind::Token(token) | Kind::Peer(token) => (AUTHORIZATION, format!("Bearer {token}")),
            Kind::Session(session) => (COOKIE, format!("{SESSION_COOKIE}={session}")),
        };
        let mut value = HeaderValue::from_str(&value)
            .map_err(|_| Error::invalid("the credential is not a header value"))?;
        value.set_sensitive(true);
        headers.insert(name, value);
        Ok(())
    }
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Kind::Guest => f.write_str("Credential::Guest"),
            Kind::Token(_) => write!(f, "Credential::Token({TOKEN_PREFIX}***)"),
            Kind::Peer(_) => write!(f, "Credential::Peer({PEER_PREFIX}***)"),
            Kind::Session(_) => write!(f, "Credential::Session({SESSION_PREFIX}***)"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OnBehalf {
    origin: String,
    external_id: String,
}

fn is_origin(origin: &str) -> bool {
    let Some((name, instance)) = origin.split_once(':') else {
        return false;
    };
    let mut bytes = name.bytes();
    name.len() <= ORIGIN_NAME_MAX_LEN
        && bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        && instance.parse::<Id>().is_ok()
}

impl OnBehalf {
    pub fn new(origin: impl Into<String>, external_id: impl Into<String>) -> Result<Self, Error> {
        let origin = origin.into();
        let external_id = external_id.into();
        if !is_origin(&origin) {
            return Err(Error::invalid(format!(
                "the on-behalf origin is not a name of a lowercase letter and up to {} lowercase letters, digits and '_', a ':' and an instance id",
                ORIGIN_NAME_MAX_LEN - 1
            )));
        }
        if !is_visible(&external_id) || external_id.len() > EXTERNAL_MAX_LEN {
            return Err(Error::invalid(format!(
                "the on-behalf external_id is not 1 to {EXTERNAL_MAX_LEN} visible characters without '\"', ',', ';' and '\\'"
            )));
        }
        Ok(Self {
            origin,
            external_id,
        })
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn external_id(&self) -> &str {
        &self.external_id
    }

    pub fn header(&self) -> String {
        format!("origin={}; external_id={}", self.origin, self.external_id)
    }

    pub(crate) fn apply(&self, headers: &mut HeaderMap) -> Result<(), Error> {
        let value = HeaderValue::from_str(&self.header())
            .map_err(|_| Error::invalid("the on-behalf header is not a header value"))?;
        headers.insert(ON_BEHALF_OF, value);
        Ok(())
    }
}

pub(crate) fn apply_key(key: &str, headers: &mut HeaderMap) -> Result<(), Error> {
    let admitted = !key.is_empty()
        && key.len() <= IDEMPOTENCY_KEY_MAX_LEN
        && key.bytes().all(|byte| (b' '..=b'~').contains(&byte))
        && key.trim() == key;
    if !admitted {
        return Err(Error::invalid(format!(
            "the Idempotency-Key is not 1 to {IDEMPOTENCY_KEY_MAX_LEN} printable characters without surrounding spaces"
        )));
    }
    let value = HeaderValue::from_str(key)
        .map_err(|_| Error::invalid("the Idempotency-Key is not a header value"))?;
    headers.insert(IDEMPOTENCY_KEY, value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use keel_types::EXTERNAL_PATTERN;

    use super::*;

    fn pattern_admits(byte: u8) -> bool {
        let class = EXTERNAL_PATTERN
            .strip_prefix("^[")
            .and_then(|rest| rest.strip_suffix("]+$"))
            .expect("the pattern is one character class");
        let mut chars = class.chars().peekable();
        let mut admitted = false;
        let read = |chars: &mut std::iter::Peekable<std::str::Chars<'_>>| {
            let first = chars.next().expect("a character");
            if first == '\\' {
                chars.next().expect("an escaped character")
            } else {
                first
            }
        };
        while chars.peek().is_some() {
            let low = read(&mut chars);
            let high = if chars.peek() == Some(&'-') {
                chars.next();
                read(&mut chars)
            } else {
                low
            };
            admitted |= (low as u32..=high as u32).contains(&u32::from(byte));
        }
        admitted
    }

    #[test]
    fn the_visible_alphabet_is_the_one_the_contract_gives_origins_and_external_ids() {
        for byte in 0u8..=255 {
            let text = (byte as char).to_string();
            let visible = byte.is_ascii() && is_visible(&text);
            assert_eq!(visible, pattern_admits(byte), "byte {byte:#04x}");
        }
        assert!(!is_visible(""));
    }

    #[test]
    fn a_credential_carries_its_prefix_and_goes_as_bearer_or_session_cookie() {
        let token = Credential::token("kat_0a1b").unwrap();
        let peer = Credential::peer("kpt_0a1b").unwrap();
        let session = Credential::session("ks_0a1b").unwrap();
        for (credential, header, value) in [
            (&token, AUTHORIZATION, "Bearer kat_0a1b"),
            (&peer, AUTHORIZATION, "Bearer kpt_0a1b"),
            (&session, COOKIE, "__Host-keel_session=ks_0a1b"),
        ] {
            let mut headers = HeaderMap::new();
            credential.apply(&mut headers).unwrap();
            assert_eq!(headers.len(), 1);
            assert_eq!(headers[&header], value);
            assert!(headers[&header].is_sensitive());
        }
        let mut headers = HeaderMap::new();
        Credential::guest().apply(&mut headers).unwrap();
        assert!(headers.is_empty());
        assert!(Credential::default().is_guest());
        assert!(!token.is_guest());
    }

    #[test]
    fn a_credential_with_another_prefix_or_an_unsafe_character_is_refused_and_never_printed() {
        for refused in [
            Credential::token("kpt_0a1b"),
            Credential::token("kat_"),
            Credential::token("kat_0a 1b"),
            Credential::token("kat_0a1b\r\nX-Evil: 1"),
            Credential::peer("kat_0a1b"),
            Credential::peer("kpt_0a;1b"),
            Credential::session("kat_0a1b"),
            Credential::session("ks_0a1b; Path=/"),
            Credential::session("ks_\u{e9}"),
        ] {
            assert!(matches!(refused, Err(Error::Invalid(_))), "{refused:?}");
        }
        let printed = format!(
            "{:?} {:?} {:?} {:?}",
            Credential::token("kat_secret").unwrap(),
            Credential::peer("kpt_secret").unwrap(),
            Credential::session("ks_secret").unwrap(),
            Credential::guest()
        );
        assert!(!printed.contains("secret"), "{printed}");
        assert_eq!(
            printed,
            "Credential::Token(kat_***) Credential::Peer(kpt_***) Credential::Session(ks_***) Credential::Guest"
        );
    }

    const INSTANCE: &str = "01J9A00000000000000000000A";

    #[test]
    fn the_on_behalf_header_names_the_origin_and_the_external_id_in_the_contract_alphabet() {
        let origin = format!("bill:{INSTANCE}");
        let customer = OnBehalf::new(origin.as_str(), "acc=7/x@y.z").unwrap();
        assert_eq!(
            customer.header(),
            "origin=bill:01J9A00000000000000000000A; external_id=acc=7/x@y.z"
        );
        let mut headers = HeaderMap::new();
        customer.apply(&mut headers).unwrap();
        assert_eq!(headers[&ON_BEHALF_OF], customer.header().as_str());
        assert_eq!(customer.origin(), origin);
        assert_eq!(customer.external_id(), "acc=7/x@y.z");
        for admitted in [
            format!("crm_2:{INSTANCE}"),
            format!("b:{INSTANCE}"),
            format!("{}:{INSTANCE}", "b".repeat(ORIGIN_NAME_MAX_LEN)),
        ] {
            assert!(
                OnBehalf::new(admitted.as_str(), "client-42").is_ok(),
                "{admitted}"
            );
        }
        let long = "x".repeat(EXTERNAL_MAX_LEN + 1);
        for (origin, external_id) in [
            (String::new(), "client-42"),
            ("bill".to_owned(), "client-42"),
            ("bill:01J9A".to_owned(), "client-42"),
            (format!("Bill:{INSTANCE}"), "client-42"),
            (format!("1bill:{INSTANCE}"), "client-42"),
            (format!("_bill:{INSTANCE}"), "client-42"),
            (format!("bill-x:{INSTANCE}"), "client-42"),
            (format!(":{INSTANCE}"), "client-42"),
            (INSTANCE.to_owned(), "client-42"),
            (format!("bill:{}", INSTANCE.to_lowercase()), "client-42"),
            (format!("bill:{INSTANCE}:x"), "client-42"),
            (
                format!("bill:{INSTANCE}, origin=crm:{INSTANCE}"),
                "client-42",
            ),
            (
                format!("{}:{INSTANCE}", "b".repeat(ORIGIN_NAME_MAX_LEN + 1)),
                "client-42",
            ),
            (origin.clone(), ""),
            (origin.clone(), "client-42; external_id=admin"),
            (origin.clone(), "client 42"),
            (origin.clone(), "\"client\""),
            (origin.clone(), "client\\42"),
            (origin.clone(), long.as_str()),
        ] {
            assert!(
                matches!(
                    OnBehalf::new(origin.as_str(), external_id),
                    Err(Error::Invalid(_))
                ),
                "{origin} {external_id}"
            );
        }
        assert!(OnBehalf::new(origin, "x".repeat(EXTERNAL_MAX_LEN)).is_ok());
    }

    #[test]
    fn an_idempotency_key_is_one_to_255_printable_characters_without_surrounding_spaces() {
        let mut headers = HeaderMap::new();
        apply_key("bill-9912 open", &mut headers).unwrap();
        assert_eq!(headers[&IDEMPOTENCY_KEY], "bill-9912 open");
        apply_key(&"k".repeat(IDEMPOTENCY_KEY_MAX_LEN), &mut headers).unwrap();
        for refused in [
            String::new(),
            " bill".to_owned(),
            "bill ".to_owned(),
            "bill\topen".to_owned(),
            "bill\u{e9}".to_owned(),
            "k".repeat(IDEMPOTENCY_KEY_MAX_LEN + 1),
        ] {
            assert!(
                matches!(apply_key(&refused, &mut headers), Err(Error::Invalid(_))),
                "{refused:?}"
            );
        }
    }
}
