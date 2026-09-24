use keel::prelude::*;
use keel::types::{CallError, FieldError};
use keel_dns_records::api::{self, RecordType};
use keel_dns_zones::api::{Expansion, RecordSeed};

const ZONE: &str = "{zone}";
const IP: &str = "{ip}";
const IP6: &str = "{ip6}";
const SAMPLE_ZONE: &str = "example.com";
const SAMPLE_IP: &str = "192.0.2.1";
const SAMPLE_IP6: &str = "2001:db8::1";
const DEFAULT_TTL: u32 = 3_600;

fn ttl() -> u32 {
    DEFAULT_TTL
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
pub(crate) struct TemplateRecord {
    #[serde(rename = "type")]
    pub(crate) kind: RecordType,
    #[schemars(length(min = 1, max = 253))]
    pub(crate) name: String,
    #[schemars(length(min = 1, max = 4000))]
    pub(crate) value: String,
    #[serde(default = "ttl")]
    #[schemars(range(min = 60, max = 604_800))]
    pub(crate) ttl: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) priority: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) weight: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) port: Option<u16>,
}

impl TemplateRecord {
    pub(crate) fn expand(&self, expansion: &Expansion) -> Option<RecordSeed> {
        let value = substitute(
            &self.value,
            &expansion.zone,
            expansion.ip.as_deref(),
            expansion.ip6.as_deref(),
        )?;
        Some(RecordSeed {
            kind: self.kind.as_str().to_owned(),
            name: self.name.clone(),
            value,
            ttl: self.ttl,
            priority: self.priority,
            weight: self.weight,
            port: self.port,
        })
    }

    fn check(&self) -> Result<()> {
        let sample = substitute(&self.value, SAMPLE_ZONE, Some(SAMPLE_IP), Some(SAMPLE_IP6))
            .unwrap_or_default();
        api::check(
            self.kind,
            &self.name,
            &sample,
            self.priority,
            self.weight,
            self.port,
        )
    }
}

pub(crate) fn checked(records: &[TemplateRecord]) -> Result<()> {
    for (index, record) in records.iter().enumerate() {
        record
            .check()
            .map_err(|error| at(&error.to_call_error(), index, &record.value))?;
    }
    Ok(())
}

fn at(error: &CallError, index: usize, value: &str) -> KeelError {
    let fields = error.fields.iter().map(|field| {
        let mut params = field.params.clone();
        if params.contains_key("value") {
            params.insert("value".to_owned(), Value::String(value.to_owned()));
        }
        FieldError {
            path: format!("records[{index}].{}", field.path),
            code: field.code.clone(),
            key: field.key.clone(),
            params,
        }
    });
    KeelError::new(error.code).fields(fields)
}

fn substitute(text: &str, zone: &str, ip: Option<&str>, ip6: Option<&str>) -> Option<String> {
    let mut expanded = text.replace(ZONE, zone);
    for (placeholder, address) in [(IP, ip), (IP6, ip6)] {
        if expanded.contains(placeholder) {
            expanded = expanded.replace(placeholder, address?);
        }
    }
    Some(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(kind: RecordType, name: &str, value: &str) -> TemplateRecord {
        TemplateRecord {
            kind,
            name: name.to_owned(),
            value: value.to_owned(),
            ttl: DEFAULT_TTL,
            priority: None,
            weight: None,
            port: None,
        }
    }

    fn expansion(ip: Option<&str>, ip6: Option<&str>) -> Expansion {
        Expansion {
            zone: "example.org".to_owned(),
            ip: ip.map(str::to_owned),
            ip6: ip6.map(str::to_owned),
        }
    }

    #[test]
    fn placeholders_are_replaced_and_a_record_needing_a_missing_address_is_left_out() {
        let both = expansion(Some("192.0.2.7"), Some("2001:db8::7"));
        let txt = record(
            RecordType::Txt,
            "@",
            "\"v=spf1 ip4:{ip} ip6:{ip6} a:{zone} -all\"",
        );
        assert_eq!(
            txt.expand(&both).map(|seed| seed.value),
            Some("\"v=spf1 ip4:192.0.2.7 ip6:2001:db8::7 a:example.org -all\"".to_owned())
        );
        let cname = record(RecordType::Cname, "www", "{zone}");
        let seed = cname
            .expand(&expansion(None, None))
            .expect("no address is needed");
        assert_eq!(seed.kind, "CNAME");
        assert_eq!(seed.name, "www");
        assert_eq!(seed.value, "example.org");
        assert_eq!(seed.ttl, DEFAULT_TTL);
        let aaaa = record(RecordType::Aaaa, "@", "{ip6}");
        assert_eq!(aaaa.expand(&expansion(Some("192.0.2.7"), None)), None);
        assert_eq!(txt.expand(&expansion(Some("192.0.2.7"), None)), None);
    }

    #[test]
    fn a_record_is_checked_with_sample_values_and_its_error_names_its_row() {
        let good = [
            record(RecordType::A, "@", "{ip}"),
            record(RecordType::Aaaa, "@", "{ip6}"),
            record(RecordType::Cname, "www", "{zone}"),
            TemplateRecord {
                priority: Some(10),
                ..record(RecordType::Mx, "@", "mail.{zone}")
            },
        ];
        checked(&good).expect("a valid record set");

        let bad = [
            record(RecordType::A, "@", "{ip}"),
            record(RecordType::A, "www", "{ip6}"),
        ];
        let error = checked(&bad)
            .expect_err("an AAAA address in an A record")
            .to_call_error();
        assert_eq!(error.code, keel::types::ErrorCode::Validation);
        assert_eq!(error.fields.len(), 1);
        assert_eq!(error.fields[0].path, "records[1].value");
        assert_eq!(error.fields[0].code.as_deref(), Some("check.ipv4"));
        assert_eq!(error.fields[0].params["value"], json!("{ip6}"));

        let missing = [record(RecordType::Mx, "@", "{zone}")];
        let error = checked(&missing)
            .expect_err("MX needs a priority")
            .to_call_error();
        assert_eq!(error.fields[0].path, "records[0].priority");

        let owner = [record(RecordType::A, "-bad", "{ip}")];
        let error = checked(&owner)
            .expect_err("a bad owner name")
            .to_call_error();
        assert_eq!(error.fields[0].path, "records[0].name");
    }
}
