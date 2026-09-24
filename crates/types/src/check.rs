use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const CHARACTER_STRING: usize = 255;
const CAA_TAG: usize = 15;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Check {
    Domain,
    Ip,
    Ipv4,
    Ipv6,
    Cidr,
    Email,
    Int,
    Name,
    Port,
    Path,
    Quoted,
    Caa,
}

impl Check {
    pub const ALL: [Check; 12] = [
        Check::Domain,
        Check::Ip,
        Check::Ipv4,
        Check::Ipv6,
        Check::Cidr,
        Check::Email,
        Check::Int,
        Check::Name,
        Check::Port,
        Check::Path,
        Check::Quoted,
        Check::Caa,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Check::Domain => "domain",
            Check::Ip => "ip",
            Check::Ipv4 => "ipv4",
            Check::Ipv6 => "ipv6",
            Check::Cidr => "cidr",
            Check::Email => "email",
            Check::Int => "int",
            Check::Name => "name",
            Check::Port => "port",
            Check::Path => "path",
            Check::Quoted => "quoted",
            Check::Caa => "caa",
        }
    }

    pub fn code(self) -> String {
        format!("check.{}", self.as_str())
    }

    pub fn key(self) -> String {
        format!("keel.check.{}", self.as_str())
    }

    pub fn matches(self, value: &str) -> bool {
        match self {
            Check::Domain => is_domain(value),
            Check::Ip => value.parse::<IpAddr>().is_ok(),
            Check::Ipv4 => value.parse::<Ipv4Addr>().is_ok(),
            Check::Ipv6 => value.parse::<Ipv6Addr>().is_ok(),
            Check::Cidr => is_cidr(value),
            Check::Email => is_email(value),
            Check::Int => is_int(value),
            Check::Name => is_name(value),
            Check::Port => is_port(value),
            Check::Path => is_path(value),
            Check::Quoted => is_quoted(value),
            Check::Caa => is_caa(value),
        }
    }
}

fn is_domain(value: &str) -> bool {
    if value.is_empty() || value.len() > 253 {
        return false;
    }
    let labels: Vec<&str> = value.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|label| is_label(label))
        && labels
            .last()
            .is_some_and(|tld| tld.bytes().any(|byte| byte.is_ascii_alphabetic()))
}

fn is_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 63
        && label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && !label.starts_with('-')
        && !label.ends_with('-')
}

fn is_cidr(value: &str) -> bool {
    let Some((address, prefix)) = value.split_once('/') else {
        return false;
    };
    if !is_canonical_digits(prefix) || prefix.len() > 3 {
        return false;
    }
    let Ok(bits) = prefix.parse::<u16>() else {
        return false;
    };
    match address.parse::<IpAddr>() {
        Ok(IpAddr::V4(_)) => bits <= 32,
        Ok(IpAddr::V6(_)) => bits <= 128,
        Err(_) => false,
    }
}

fn is_email(value: &str) -> bool {
    if value.len() > 254 {
        return false;
    }
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    local.len() <= 64
        && !local.is_empty()
        && !local.starts_with('.')
        && !local.ends_with('.')
        && !local.contains("..")
        && local.bytes().all(is_local_part_byte)
        && is_domain(domain)
}

fn is_local_part_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b".!#$%&'*+/=?^_`{|}~-".contains(&byte)
}

fn is_int(value: &str) -> bool {
    let digits = value.strip_prefix('-').unwrap_or(value);
    is_canonical_digits(digits) && value.parse::<i64>().is_ok()
}

fn is_canonical_digits(text: &str) -> bool {
    !text.is_empty()
        && text.bytes().all(|byte| byte.is_ascii_digit())
        && (text.len() == 1 || !text.starts_with('0'))
}

fn is_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && value.len() <= 64
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn is_port(value: &str) -> bool {
    is_canonical_digits(value) && value != "0" && value.len() <= 5 && value.parse::<u16>().is_ok()
}

fn is_path(value: &str) -> bool {
    if value == "/" {
        return true;
    }
    let Some(rest) = value.strip_prefix('/') else {
        return false;
    };
    !value.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
        && rest
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

// https://www.rfc-editor.org/rfc/rfc1035#section-5.1
fn is_quoted(value: &str) -> bool {
    let mut rest = value;
    loop {
        let Some(after) = character_string(rest) else {
            return false;
        };
        if after.is_empty() {
            return true;
        }
        let Some(next) = after.strip_prefix(' ') else {
            return false;
        };
        rest = next.trim_start_matches(' ');
    }
}

fn character_string(text: &str) -> Option<&str> {
    let body = text.strip_prefix('"')?;
    let mut length = 0;
    let mut escaped = false;
    for (index, byte) in body.bytes().enumerate() {
        if !(0x20..0x7f).contains(&byte) {
            return None;
        }
        if escaped {
            if byte != b'"' && byte != b'\\' {
                return None;
            }
            escaped = false;
            length += 1;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            return (length <= CHARACTER_STRING).then(|| &body[index + 1..]);
        } else {
            length += 1;
        }
    }
    None
}

// https://www.rfc-editor.org/rfc/rfc8659#section-4.1.1
fn is_caa(value: &str) -> bool {
    let mut parts = value.splitn(3, ' ');
    let (Some(flags), Some(tag), Some(text)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    is_canonical_digits(flags)
        && flags.parse::<u8>().is_ok()
        && !tag.is_empty()
        && tag.len() <= CAA_TAG
        && tag.bytes().all(|byte| byte.is_ascii_alphanumeric())
        && character_string(text) == Some("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_check_fails_to_deserialise() {
        assert!(serde_json::from_value::<Check>(serde_json::json!("phone")).is_err());
        assert!(serde_json::from_value::<Check>(serde_json::json!("IP")).is_err());
    }

    #[test]
    fn a_failed_check_names_its_error_code_and_key() {
        assert_eq!(Check::Ip.code(), "check.ip");
        assert_eq!(Check::Ip.key(), "keel.check.ip");
        for check in Check::ALL {
            assert_eq!(
                serde_json::to_value(check).unwrap(),
                serde_json::json!(check.as_str())
            );
        }
    }

    #[test]
    fn an_address_family_check_admits_only_its_own_family() {
        assert!(Check::Ipv4.matches("192.0.2.1"));
        assert!(!Check::Ipv4.matches("2001:db8::1"));
        assert!(!Check::Ipv4.matches("192.0.2.01"));
        assert!(Check::Ipv6.matches("2001:db8::1"));
        assert!(Check::Ipv6.matches("::ffff:192.0.2.1"));
        assert!(!Check::Ipv6.matches("192.0.2.1"));
    }

    #[test]
    fn quoted_text_is_one_or_more_printable_character_strings_of_at_most_255() {
        assert!(Check::Quoted.matches("\"v=spf1 -all\""));
        assert!(Check::Quoted.matches("\"a\" \"b\""));
        assert!(Check::Quoted.matches("\"\""));
        assert!(Check::Quoted.matches("\"say \\\"hi\\\" \\\\ ok; (fine)\""));
        assert!(Check::Quoted.matches(&format!("\"{}\"", "a".repeat(255))));
        assert!(!Check::Quoted.matches(&format!("\"{}\"", "a".repeat(256))));
        assert!(!Check::Quoted.matches("v=spf1"));
        assert!(!Check::Quoted.matches("\"open"));
        assert!(!Check::Quoted.matches("\"a\"\"b\""));
        assert!(!Check::Quoted.matches("\"a\" "));
        assert!(!Check::Quoted.matches(" \"a\""));
        assert!(!Check::Quoted.matches("\"line\nbreak\""));
        assert!(!Check::Quoted.matches("\"\\065\""));
        assert!(!Check::Quoted.matches("\"caf\u{e9}\""));
    }

    #[test]
    fn caa_is_flags_a_tag_and_one_quoted_value() {
        assert!(Check::Caa.matches("0 issue \"letsencrypt.org\""));
        assert!(Check::Caa.matches("128 iodef \"mailto:security@example.org\""));
        assert!(Check::Caa.matches("0 issuewild \";\""));
        assert!(!Check::Caa.matches("256 issue \"a.org\""));
        assert!(!Check::Caa.matches("00 issue \"a.org\""));
        assert!(!Check::Caa.matches("0 is-sue \"a.org\""));
        assert!(!Check::Caa.matches("0 abcdefghijklmnop \"a.org\""));
        assert!(!Check::Caa.matches("0 issue a.org"));
        assert!(!Check::Caa.matches("0 issue \"a\" \"b\""));
        assert!(!Check::Caa.matches("0 issue"));
    }

    #[test]
    fn domain_requires_two_valid_labels_and_a_non_numeric_top_level() {
        assert!(Check::Domain.matches("a.org"));
        assert!(Check::Domain.matches("Sub.Example.COM"));
        assert!(Check::Domain.matches("xn--80ak6aa92e.com"));
        assert!(!Check::Domain.matches("org"));
        assert!(!Check::Domain.matches("a..org"));
        assert!(!Check::Domain.matches("-a.org"));
        assert!(!Check::Domain.matches("a-.org"));
        assert!(!Check::Domain.matches("a.org."));
        assert!(!Check::Domain.matches("a_b.org"));
        assert!(!Check::Domain.matches("10.0.0.1"));
        assert!(!Check::Domain.matches(""));
        assert!(!Check::Domain.matches(&format!("{}.org", "a".repeat(64))));
    }

    #[test]
    fn cidr_bounds_the_prefix_by_the_address_family() {
        assert!(Check::Cidr.matches("10.0.0.0/8"));
        assert!(Check::Cidr.matches("10.0.0.1/32"));
        assert!(Check::Cidr.matches("2001:db8::/32"));
        assert!(Check::Cidr.matches("::/0"));
        assert!(!Check::Cidr.matches("10.0.0.0/33"));
        assert!(!Check::Cidr.matches("2001:db8::/129"));
        assert!(!Check::Cidr.matches("10.0.0.0"));
        assert!(!Check::Cidr.matches("10.0.0.0/"));
        assert!(!Check::Cidr.matches("10.0.0.0/08"));
        assert!(!Check::Cidr.matches("10.0.0.0/+8"));
    }

    #[test]
    fn int_port_name_and_path_are_canonical() {
        assert!(Check::Int.matches("0"));
        assert!(Check::Int.matches("-42"));
        assert!(!Check::Int.matches("+42"));
        assert!(!Check::Int.matches("007"));
        assert!(!Check::Int.matches("1.0"));
        assert!(!Check::Int.matches("9223372036854775808"));
        assert!(Check::Port.matches("1"));
        assert!(Check::Port.matches("65535"));
        assert!(!Check::Port.matches("0"));
        assert!(!Check::Port.matches("65536"));
        assert!(!Check::Port.matches("08080"));
        assert!(Check::Name.matches("web_1-a"));
        assert!(!Check::Name.matches("1web"));
        assert!(!Check::Name.matches("web.1"));
        assert!(!Check::Name.matches(""));
        assert!(!Check::Name.matches(&"a".repeat(65)));
        assert!(Check::Path.matches("/"));
        assert!(Check::Path.matches("/var/www/a.org"));
        assert!(!Check::Path.matches("var/www"));
        assert!(!Check::Path.matches("/var//www"));
        assert!(!Check::Path.matches("/var/../etc"));
        assert!(!Check::Path.matches("/var/./www"));
        assert!(!Check::Path.matches("/var/www/"));
        assert!(!Check::Path.matches("/var/w\u{0}ww"));
    }
}
