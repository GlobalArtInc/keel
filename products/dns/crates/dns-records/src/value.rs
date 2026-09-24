use keel::prelude::*;
use keel::sdk::serde_json::Map;
use keel::types::{Check, ErrorCode, FieldError};

use crate::record;

const REQUIRED_CODE: &str = "schema.required";
const REQUIRED_KEY: &str = "keel.schema.required";
const PATTERN_CODE: &str = "schema.pattern";
const PATTERN_KEY: &str = "keel.schema.pattern";
const OWNER_MAX: usize = 253;
const LABEL_MAX: usize = 63;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
pub(crate) enum RecordType {
    A,
    #[serde(rename = "AAAA")]
    Aaaa,
    #[serde(rename = "CNAME")]
    Cname,
    #[serde(rename = "MX")]
    Mx,
    #[serde(rename = "TXT")]
    Txt,
    #[serde(rename = "NS")]
    Ns,
    #[serde(rename = "SRV")]
    Srv,
    #[serde(rename = "CAA")]
    Caa,
}

impl RecordType {
    pub(crate) const ALL: [RecordType; 8] = [
        RecordType::A,
        RecordType::Aaaa,
        RecordType::Cname,
        RecordType::Mx,
        RecordType::Txt,
        RecordType::Ns,
        RecordType::Srv,
        RecordType::Caa,
    ];

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            RecordType::A => "A",
            RecordType::Aaaa => "AAAA",
            RecordType::Cname => "CNAME",
            RecordType::Mx => "MX",
            RecordType::Txt => "TXT",
            RecordType::Ns => "NS",
            RecordType::Srv => "SRV",
            RecordType::Caa => "CAA",
        }
    }

    pub(crate) fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == text)
    }

    pub(crate) const fn check(self) -> Check {
        match self {
            RecordType::A => Check::Ipv4,
            RecordType::Aaaa => Check::Ipv6,
            RecordType::Cname | RecordType::Mx | RecordType::Ns | RecordType::Srv => Check::Domain,
            RecordType::Txt => Check::Quoted,
            RecordType::Caa => Check::Caa,
        }
    }

    const fn names_a_host(self) -> bool {
        matches!(
            self,
            RecordType::Cname | RecordType::Mx | RecordType::Ns | RecordType::Srv
        )
    }

    const fn has_priority(self) -> bool {
        matches!(self, RecordType::Mx | RecordType::Srv)
    }

    const fn has_target_port(self) -> bool {
        matches!(self, RecordType::Srv)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) kind: RecordType,
    pub(crate) name: String,
    pub(crate) value: String,
    pub(crate) priority: Option<i32>,
    pub(crate) weight: Option<i32>,
    pub(crate) port: Option<i32>,
}

impl Entry {
    pub(crate) fn checked(
        kind: RecordType,
        name: &str,
        value: &str,
        priority: Option<u16>,
        weight: Option<u16>,
        port: Option<u16>,
    ) -> Result<Self> {
        if !is_owner(name) {
            return Err(refused(
                "name",
                PATTERN_CODE.to_owned(),
                PATTERN_KEY.to_owned(),
                None,
            ));
        }
        let check = kind.check();
        if !check.matches(value) {
            return Err(refused(
                "value",
                check.code(),
                check.key(),
                Some(("value", value)),
            ));
        }
        let priority = kind
            .has_priority()
            .then(|| required("priority", priority))
            .transpose()?;
        let (weight, port) = if kind.has_target_port() {
            (
                Some(required("weight", weight)?),
                Some(required("port", port)?),
            )
        } else {
            (None, None)
        };
        let value = if kind.names_a_host() {
            value.to_ascii_lowercase()
        } else {
            value.to_owned()
        };
        Ok(Self {
            kind,
            name: name.to_ascii_lowercase(),
            value,
            priority,
            weight,
            port,
        })
    }
}

fn required(field: &str, value: Option<u16>) -> Result<i32> {
    value.map(i32::from).ok_or_else(|| {
        refused(
            field,
            REQUIRED_CODE.to_owned(),
            REQUIRED_KEY.to_owned(),
            Some(("property", field)),
        )
    })
}

fn is_owner(name: &str) -> bool {
    if name == "@" || name == "*" {
        return true;
    }
    let labels = name.strip_prefix("*.").unwrap_or(name);
    name.len() <= OWNER_MAX
        && labels.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= LABEL_MAX
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        })
}

fn refused(path: &str, code: String, key: String, param: Option<(&str, &str)>) -> KeelError {
    let mut params = Map::new();
    if let Some((name, value)) = param {
        params.insert(name.to_owned(), Value::String(value.to_owned()));
    }
    KeelError::new(ErrorCode::Validation).fields([FieldError {
        path: path.to_owned(),
        code: Some(code),
        key: Some(key),
        params,
    }])
}

pub(crate) fn data(row: &record::Model) -> String {
    let Some(kind) = RecordType::parse(&row.kind) else {
        return row.value.clone();
    };
    let target = if kind.names_a_host() {
        format!("{}.", row.value)
    } else {
        row.value.clone()
    };
    let number = |value: Option<i32>| value.unwrap_or_default().to_string();
    match kind {
        RecordType::Mx => format!("{} {target}", number(row.priority)),
        RecordType::Srv => format!(
            "{} {} {} {target}",
            number(row.priority),
            number(row.weight),
            number(row.port)
        ),
        _ => target,
    }
}

#[cfg(test)]
mod tests {
    use keel::sdk::chrono::Utc;

    use super::*;

    fn row(
        kind: RecordType,
        value: &str,
        priority: Option<i32>,
        weight: Option<i32>,
        port: Option<i32>,
    ) -> record::Model {
        record::Model {
            id: "01J9R0000000000000000000R1".to_owned(),
            zone_id: "01J9Q3W6B1R8N5X2K4T7M0V9Z3".to_owned(),
            account_id: "01J8A0000000000000000000A1".to_owned(),
            kind: kind.as_str().to_owned(),
            name: "@".to_owned(),
            value: value.to_owned(),
            ttl: 3600,
            priority,
            weight,
            port,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn every_type_names_its_wire_form_and_the_check_of_its_value() {
        for kind in RecordType::ALL {
            assert_eq!(RecordType::parse(kind.as_str()), Some(kind));
            assert_eq!(
                keel::sdk::serde_json::to_value(kind).expect("a type serialises"),
                json!(kind.as_str())
            );
        }
        assert_eq!(RecordType::parse("a"), None);
        assert_eq!(RecordType::Srv.check(), Check::Domain);
        assert_eq!(RecordType::Txt.check(), Check::Quoted);
    }

    #[test]
    fn an_entry_is_checked_by_its_type_and_keeps_the_numbers_its_type_uses() {
        let refused = Entry::checked(RecordType::A, "www", "2001:db8::1", None, None, None)
            .expect_err("an IPv6 address is not an A record")
            .to_call_error();
        assert_eq!(refused.fields[0].path, "value");
        assert_eq!(refused.fields[0].code.as_deref(), Some("check.ipv4"));
        let mx = Entry::checked(RecordType::Mx, "@", "Mail.Example.org", None, None, None)
            .expect_err("an MX record needs a priority")
            .to_call_error();
        assert_eq!(mx.fields[0].path, "priority");
        let srv = Entry::checked(
            RecordType::Srv,
            "_sip._tcp",
            "sip.example.org",
            Some(10),
            Some(5),
            Some(5060),
        )
        .expect("a complete SRV record");
        assert_eq!(
            (srv.priority, srv.weight, srv.port),
            (Some(10), Some(5), Some(5060))
        );
        let a = Entry::checked(
            RecordType::A,
            "WWW",
            "192.0.2.1",
            Some(10),
            Some(5),
            Some(80),
        )
        .expect("an A record");
        assert_eq!(a.name, "www");
        assert_eq!((a.priority, a.weight, a.port), (None, None, None));
        let mx = Entry::checked(
            RecordType::Mx,
            "@",
            "Mail.Example.org",
            Some(10),
            None,
            None,
        )
        .expect("an MX record");
        assert_eq!(mx.value, "mail.example.org");
    }

    #[test]
    fn an_owner_is_the_apex_a_wildcard_or_relative_labels() {
        for owner in [
            "@",
            "*",
            "www",
            "*.dev",
            "_sip._tcp",
            "a-b.c1",
            &"a".repeat(63),
        ] {
            assert!(is_owner(owner), "{owner}");
        }
        for owner in [
            "",
            "a..b",
            "-a",
            "a-",
            "www.",
            "a.*",
            "**",
            "a b",
            "a\nb",
            "\"a\"",
            &"a".repeat(64),
        ] {
            assert!(!is_owner(owner), "{owner:?}");
        }
        let refused = Entry::checked(
            RecordType::A,
            "a\n$INCLUDE /etc/passwd",
            "192.0.2.1",
            None,
            None,
            None,
        )
        .expect_err("a line break in the owner")
        .to_call_error();
        assert_eq!(refused.fields[0].path, "name");
        assert_eq!(refused.fields[0].code.as_deref(), Some("schema.pattern"));
    }

    #[test]
    fn the_data_of_a_record_ends_a_host_with_a_dot_and_leads_with_its_numbers() {
        assert_eq!(
            data(&row(RecordType::A, "192.0.2.1", None, None, None)),
            "192.0.2.1"
        );
        assert_eq!(
            data(&row(RecordType::Cname, "web.example.org", None, None, None)),
            "web.example.org."
        );
        assert_eq!(
            data(&row(
                RecordType::Mx,
                "mail.example.org",
                Some(10),
                None,
                None
            )),
            "10 mail.example.org."
        );
        assert_eq!(
            data(&row(
                RecordType::Srv,
                "sip.example.org",
                Some(10),
                Some(5),
                Some(5060)
            )),
            "10 5 5060 sip.example.org."
        );
        assert_eq!(
            data(&row(RecordType::Txt, "\"v=spf1 -all\"", None, None, None)),
            "\"v=spf1 -all\""
        );
        assert_eq!(
            data(&row(RecordType::Caa, "0 issue \"a.org\"", None, None, None)),
            "0 issue \"a.org\""
        );
    }
}
