use jsonschema::paths::LocationSegment;
use keel_sdk::ConfigSection;
use keel_sdk::runtime::Section;
use serde_json::Value;

pub struct Fault {
    pub path: String,
    pub reason: String,
}

pub fn load(section: &ConfigSection, raw: &Value) -> Result<Section, Fault> {
    let schema = section.schema().as_value();
    let validator = jsonschema::validator_for(schema).map_err(|error| Fault {
        path: section.key().to_owned(),
        reason: format!("the schema of the section does not compile: {error}"),
    })?;
    if let Some(error) = validator.iter_errors(raw).next() {
        let mut path = section.key().to_owned();
        for segment in error.instance_path().segments() {
            match segment {
                LocationSegment::Property(name) => {
                    path.push('.');
                    path.push_str(&name);
                }
                LocationSegment::Index(index) => path.push_str(&format!("[{index}]")),
            }
        }
        return Err(Fault {
            path,
            reason: error.to_string(),
        });
    }
    section.parse(raw.clone()).map_err(|error| Fault {
        path: section.key().to_owned(),
        reason: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use keel_sdk::schemars::JsonSchema;
    use keel_sdk::serde::Deserialize;
    use serde_json::json;

    use super::*;

    #[derive(Debug, Default, Deserialize, JsonSchema, PartialEq)]
    #[serde(crate = "keel_sdk::serde", default, deny_unknown_fields)]
    #[schemars(crate = "keel_sdk::schemars")]
    struct Host {
        socket: String,
        retries: u8,
        mirrors: Vec<Mirror>,
    }

    #[derive(Debug, Default, Deserialize, JsonSchema, PartialEq)]
    #[serde(crate = "keel_sdk::serde", default, deny_unknown_fields)]
    #[schemars(crate = "keel_sdk::schemars")]
    struct Mirror {
        url: String,
    }

    #[test]
    fn a_section_is_validated_against_its_schema_and_a_fault_names_the_path_under_the_key() {
        let section = ConfigSection::new::<Host>("host");
        let loaded = load(&section, &json!({ "socket": "/tmp/a.sock", "retries": 2 }))
            .ok()
            .and_then(|parsed| parsed.downcast::<Arc<Host>>().ok())
            .expect("the section loads");
        assert_eq!(loaded.socket, "/tmp/a.sock");
        assert_eq!(loaded.retries, 2);
        let typo = load(&section, &json!({ "sockte": "/tmp/a.sock" })).expect_err("a typo");
        assert_eq!(typo.path, "host");
        assert!(typo.reason.contains("sockte"), "{}", typo.reason);
        let wrong = load(&section, &json!({ "retries": "many" })).expect_err("a wrong type");
        assert_eq!(wrong.path, "host.retries");
        assert!(wrong.reason.contains("many"), "{}", wrong.reason);
        let nested = load(
            &section,
            &json!({ "mirrors": [{ "url": "a" }, { "url": 7 }] }),
        )
        .expect_err("a wrong nested type");
        assert_eq!(nested.path, "host.mirrors[1].url");
        let scalar = load(&section, &json!("nope")).expect_err("a scalar");
        assert_eq!(scalar.path, "host");
    }
}
