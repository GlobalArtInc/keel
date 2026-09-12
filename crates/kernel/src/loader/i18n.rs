use std::collections::BTreeMap;

use serde_json::{Map, Value};

pub const REQUIRED_LANGUAGE: &str = "en";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Catalogue {
    languages: BTreeMap<String, Map<String, Value>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub lang: String,
    pub key: String,
}

impl Catalogue {
    pub fn merge(&mut self, lang: &str, messages: &Value) -> Vec<Conflict> {
        let mut conflicts = Vec::new();
        let Value::Object(incoming) = messages else {
            return conflicts;
        };
        let target = self.languages.entry(lang.to_owned()).or_default();
        merge_into(target, incoming, &mut Vec::new(), &mut |key| {
            conflicts.push(Conflict {
                lang: lang.to_owned(),
                key,
            })
        });
        conflicts
    }

    pub fn has(&self, lang: &str, key: &str) -> bool {
        self.languages
            .get(lang)
            .and_then(|messages| find(messages, key))
            .is_some_and(Value::is_string)
    }

    pub fn into_languages(self) -> BTreeMap<String, Map<String, Value>> {
        self.languages
    }
}

fn merge_into(
    target: &mut Map<String, Value>,
    incoming: &Map<String, Value>,
    path: &mut Vec<String>,
    conflict: &mut impl FnMut(String),
) {
    for (key, value) in incoming {
        path.push(key.clone());
        match (target.get_mut(key), value) {
            (Some(Value::Object(existing)), Value::Object(nested)) => {
                merge_into(existing, nested, path, conflict);
            }
            (Some(_), _) => conflict(path.join(".")),
            (None, _) => {
                target.insert(key.clone(), value.clone());
            }
        }
        path.pop();
    }
}

fn find<'a>(messages: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    if let Some(value) = messages.get(key) {
        return Some(value);
    }
    for (index, _) in key.match_indices('.') {
        let (head, tail) = (&key[..index], &key[index + 1..]);
        if let Some(Value::Object(child)) = messages.get(head)
            && let Some(found) = find(child, tail)
        {
            return Some(found);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn keys_resolve_through_nested_objects_and_flat_entries_alike() {
        let mut catalogue = Catalogue::default();
        let conflicts = catalogue.merge(
            "en",
            &json!({
                "dns": { "zone": { "list": { "title": "Zones" }, "empty": "No zones" } },
                "dns.zone.edit.title": "Zone",
                "menu": { "dns": "DNS" }
            }),
        );
        assert!(conflicts.is_empty());
        assert!(catalogue.has("en", "dns.zone.list.title"));
        assert!(catalogue.has("en", "dns.zone.empty"));
        assert!(catalogue.has("en", "dns.zone.edit.title"));
        assert!(catalogue.has("en", "menu.dns"));
        assert!(!catalogue.has("en", "dns.zone"));
        assert!(!catalogue.has("en", "dns.zone.list"));
        assert!(!catalogue.has("en", "dns.zone.get.title"));
        assert!(!catalogue.has("ru", "dns.zone.list.title"));
    }

    #[test]
    fn dictionaries_of_two_modules_merge_deeply_and_a_shared_leaf_is_a_conflict() {
        let mut catalogue = Catalogue::default();
        catalogue.merge(
            "en",
            &json!({ "menu": { "dns": "DNS" }, "dns": { "a": "A" } }),
        );
        let conflicts = catalogue.merge(
            "en",
            &json!({ "menu": { "auth": "Auth", "dns": "Again" }, "dns": { "b": "B" } }),
        );
        assert_eq!(
            conflicts,
            [Conflict {
                lang: "en".into(),
                key: "menu.dns".into()
            }]
        );
        assert!(catalogue.has("en", "menu.auth"));
        assert!(catalogue.has("en", "dns.a"));
        assert!(catalogue.has("en", "dns.b"));
        let languages = catalogue.into_languages();
        assert_eq!(languages["en"]["menu"]["dns"], json!("DNS"));
    }

    #[test]
    fn a_dictionary_that_is_not_an_object_adds_nothing() {
        let mut catalogue = Catalogue::default();
        assert!(catalogue.merge("en", &json!([1, 2])).is_empty());
        assert!(catalogue.into_languages().is_empty());
    }
}
