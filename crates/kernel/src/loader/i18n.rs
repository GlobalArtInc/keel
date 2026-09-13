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

    pub fn languages(&self) -> Vec<String> {
        self.languages.keys().cloned().collect()
    }

    pub fn into_languages(self) -> BTreeMap<String, Map<String, Value>> {
        self.languages
    }
}

pub fn placeholder_fault(text: &str) -> Option<String> {
    for style in ["{{", "}}"] {
        if text.contains(style) {
            return Some(style.to_owned());
        }
    }
    if let Some(fault) = percent_fault(text) {
        return Some(fault);
    }
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            return Some(rest[open..].to_owned());
        };
        let name = &after[..close];
        if !placeholder_name(name) {
            return Some(format!("{{{name}}}"));
        }
        rest = &after[close + 1..];
    }
    rest.contains('}').then(|| "}".to_owned())
}

fn percent_fault(text: &str) -> Option<String> {
    for (start, c) in text.char_indices() {
        if c != '%' {
            continue;
        }
        let tail = &text[start + 1..];
        let digits = tail.chars().take_while(char::is_ascii_digit).count();
        let after = &tail[digits..];
        let positional = digits > 0 && after.starts_with('$');
        let spec = if positional { &after[1..] } else { tail };
        if let Some(letter) = spec.chars().next()
            && "sdifuxXeEgGc".contains(letter)
            && (positional || digits == 0)
        {
            let len = 1 + digits + usize::from(positional) + letter.len_utf8();
            return Some(text[start..start + len].to_owned());
        }
    }
    None
}

fn placeholder_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub fn leaves(messages: &Map<String, Value>) -> Vec<(String, Value)> {
    let mut into = Vec::new();
    collect(messages, &mut Vec::new(), &mut into);
    into
}

fn collect(messages: &Map<String, Value>, path: &mut Vec<String>, into: &mut Vec<(String, Value)>) {
    for (key, value) in messages {
        path.push(key.clone());
        match value {
            Value::Object(nested) => collect(nested, path, into),
            _ => into.push((path.join("."), value.clone())),
        }
        path.pop();
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

    #[test]
    fn placeholders_are_names_in_single_braces_and_nothing_else() {
        for text in [
            "The level {level} is required",
            "{a} and {b_2} and {_c}",
            "100% sure",
            "50 % of {total}",
            "plain text",
            "",
        ] {
            assert_eq!(placeholder_fault(text), None, "{text:?}");
        }
        let faults = [
            ("Hello {{name}}", "{{"),
            ("Hello {name}}", "}}"),
            ("Hello %s", "%s"),
            ("%d items", "%d"),
            ("%1$s of %2$s", "%1$s"),
            ("Hello {0}", "{0}"),
            ("Hello { name }", "{ name }"),
            ("Hello {name", "{name"),
            ("Hello name}", "}"),
            ("Hello {}", "{}"),
        ];
        for (text, expected) in faults {
            assert_eq!(
                placeholder_fault(text).as_deref(),
                Some(expected),
                "{text:?}"
            );
        }
    }

    #[test]
    fn leaves_walk_nested_objects_and_keep_flat_entries() {
        let messages = json!({ "a": { "b": "B", "c": { "d": 1 } }, "e.f": "EF" })
            .as_object()
            .cloned()
            .unwrap();
        let listed: Vec<(String, Value)> = leaves(&messages);
        assert_eq!(
            listed,
            [
                ("a.b".to_owned(), json!("B")),
                ("a.c.d".to_owned(), json!(1)),
                ("e.f".to_owned(), json!("EF")),
            ]
        );
    }
}
