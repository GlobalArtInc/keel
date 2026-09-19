use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

use super::extract::{Required, required};
use super::{Catalogue, Error, Projects, REQUIRED_LANGUAGE};

#[derive(Debug, PartialEq, Eq)]
struct Gap {
    key: String,
    owner: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Missing {
    gaps: BTreeMap<String, Vec<Gap>>,
    checked: usize,
}

impl Missing {
    pub fn green(&self) -> bool {
        self.gaps.values().all(Vec::is_empty)
    }

    pub fn summary(&self) -> String {
        let langs: Vec<&str> = self.gaps.keys().map(String::as_str).collect();
        if self.green() {
            return format!("{} keys translated in {}", self.checked, langs.join(", "));
        }
        let counts: Vec<String> = self
            .gaps
            .iter()
            .filter(|(_, gaps)| !gaps.is_empty())
            .map(|(lang, gaps)| format!("{} in {lang}", gaps.len()))
            .collect();
        format!(
            "{} of {} keys without a translation: {}",
            self.gaps.values().map(Vec::len).sum::<usize>(),
            self.checked,
            counts.join(", ")
        )
    }
}

impl fmt::Display for Missing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (lang, gaps) in &self.gaps {
            for gap in gaps {
                writeln!(f, "{lang}: {} ({})", gap.key, gap.owner)?;
            }
        }
        Ok(())
    }
}

pub fn missing(
    dump: &Path,
    projects: &Projects,
    root: &Path,
    langs: &[String],
) -> Result<Missing, Error> {
    let mut wanted: Vec<Required> = Catalogue::of_dump(dump, REQUIRED_LANGUAGE)?
        .leaves()
        .into_keys()
        .map(|key| Required {
            owner: format!("{REQUIRED_LANGUAGE} catalogue"),
            key,
            shared: None,
        })
        .collect();
    wanted.extend(required(dump)?);
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut gaps = BTreeMap::new();
    for lang in langs {
        let catalogue = Catalogue::of_dictionaries(projects, root, lang)?;
        let mut of_lang = Vec::new();
        seen.clear();
        for required in &wanted {
            if !seen.insert(required.key.as_str()) {
                continue;
            }
            let satisfied = catalogue.get(&required.key).is_some()
                || required
                    .shared
                    .as_deref()
                    .is_some_and(|shared| catalogue.get(shared).is_some());
            if !satisfied {
                of_lang.push(Gap {
                    key: required.key.clone(),
                    owner: required.owner.clone(),
                });
            }
        }
        gaps.insert(lang.clone(), of_lang);
    }
    Ok(Missing {
        gaps,
        checked: seen.len(),
    })
}

#[cfg(test)]
pub(super) mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde_json::{Value, json};

    use super::super::{Project, nest, pretty};
    use super::*;

    pub(in super::super) fn dump(en: Value, ru: Value) -> (tempfile::TempDir, PathBuf) {
        let temporary = tempfile::tempdir().unwrap();
        let dump = temporary.path().join("dump");
        let admin = dump.join("admin");
        fs::create_dir_all(admin.join("functions")).unwrap();
        fs::create_dir_all(admin.join("i18n")).unwrap();
        let list = json!({
            "keel": 1, "name": "shop.item.list", "module": "shop", "kind": "list", "mode": "read",
            "access": { "level": "user" }, "key": "id",
            "params": { "$ref": "keel://ListQuery" }, "result": { "type": "object" },
            "ui": {
                "title": "shop.item.list.title",
                "columns": [
                    { "key": "name", "type": "text" },
                    { "key": "state", "type": "status", "map": { "open": "info" }, "filter": { "options": ["open", "closed"] } }
                ],
                "actions": [{ "id": "create", "role": "new", "screen": "shop.item.edit" }],
                "empty": { "title": "shop.item.empty" }
            }
        });
        let edit = json!({
            "keel": 1, "name": "shop.item.edit", "module": "shop", "kind": "form", "mode": "write",
            "access": { "level": "user" },
            "params": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "flavour": { "$ref": "#/$defs/Flavour" },
                    "note": { "type": "string", "x-ui": { "placeholder": "shop.item.note.placeholder" } }
                },
                "$defs": { "Flavour": { "type": "string", "enum": ["sweet", "sour"] } }
            },
            "result": { "type": "object" },
            "ui": { "title": "shop.item.edit.title", "pages": [{ "id": "main", "fields": ["name", "flavour", "note"] }] }
        });
        let bare = json!({
            "keel": 1, "name": "shop.item.delete", "module": "shop", "kind": "action", "mode": "write",
            "access": { "level": "user" }, "params": { "type": "object" }, "result": { "type": "object" }
        });
        for descriptor in [&list, &edit, &bare] {
            let name = descriptor["name"].as_str().unwrap();
            fs::write(
                admin.join("functions").join(format!("{name}.json")),
                descriptor.to_string(),
            )
            .unwrap();
        }
        let boot = json!({
            "keel": { "version": "0.1.0", "api": 1 },
            "products": [{ "id": "shop", "name": "product.shop.brand", "edition": null, "brand": { "name": "product.shop.brand", "hue": 10, "chroma": 0.1 } }],
            "brand": { "name": "keel.brand", "hue": 220, "chroma": 0.08 },
            "principal": null, "features": [], "license": null,
            "menu": [{ "id": "shop", "product": "shop", "label": "menu.shop", "icon": "tag", "items": [
                { "path": "shop/items", "label": "shop.item.list.title", "func": "shop.item.list", "kind": "list" }
            ] }],
            "dashboard": [], "views": [], "meta": "m-0000000000000000", "problems": null
        });
        fs::write(admin.join("boot.json"), boot.to_string()).unwrap();
        fs::write(
            admin.join("i18n/en.json"),
            json!({ "lang": "en", "messages": en }).to_string(),
        )
        .unwrap();
        fs::write(
            admin.join("i18n/ru.json"),
            json!({ "lang": "ru", "messages": ru }).to_string(),
        )
        .unwrap();
        (temporary, dump)
    }

    pub(in super::super) fn complete_en() -> Value {
        json!({
            "keel": {
                "brand": "Keel",
                "name": "Name",
                "state": "State",
                "column": { "name": "Name", "state": "State" },
                "action": { "create": "New" },
                "page": { "main": "Main" },
                "error": { "validation": "The input is not valid" }
            },
            "menu": { "shop": "Shop" },
            "product": { "shop": { "brand": "Keel Shop" } },
            "shop": {
                "item": {
                    "list": { "title": "Items" },
                    "edit": { "title": "Item" },
                    "empty": "No items yet",
                    "state": { "open": "Open", "closed": "Closed" },
                    "flavour": "Flavour",
                    "flavour.sweet": "Sweet",
                    "flavour.sour": "Sour",
                    "note": "Note",
                    "note.placeholder": "Anything worth remembering"
                }
            }
        })
    }

    fn ru_of(en: &Value) -> Value {
        match en {
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, value)| (key.clone(), ru_of(value)))
                    .collect(),
            ),
            Value::String(text) => Value::String(format!("ru:{text}")),
            other => other.clone(),
        }
    }

    fn filled(en: &Value, over: &Value) -> Value {
        match (en, over) {
            (Value::Object(en), Value::Object(over)) => Value::Object(
                en.iter()
                    .map(|(key, value)| {
                        let filled = over
                            .get(key)
                            .map_or_else(|| value.clone(), |over| filled(value, over));
                        (key.clone(), filled)
                    })
                    .collect(),
            ),
            (_, over) => over.clone(),
        }
    }

    fn shop_projects() -> Projects {
        Projects {
            url: "https://api.ownlate.test".to_owned(),
            projects: vec![
                Project {
                    name: "keel-core".to_owned(),
                    id_env: "CORE".to_owned(),
                    directories: BTreeMap::from([(
                        "crates/kernel/i18n".to_owned(),
                        vec!["keel".to_owned()],
                    )]),
                },
                Project {
                    name: "keel-shop".to_owned(),
                    id_env: "SHOP".to_owned(),
                    directories: BTreeMap::from([(
                        "products/shop/i18n".to_owned(),
                        vec![
                            "shop".to_owned(),
                            "menu.shop".to_owned(),
                            "product.shop".to_owned(),
                        ],
                    )]),
                },
            ],
        }
    }

    fn write_dictionaries(projects: &Projects, root: &Path, lang: &str, messages: &Value) {
        let Value::Object(messages) = messages else {
            panic!("the {lang} messages are not an object");
        };
        let mut split: BTreeMap<&str, BTreeMap<String, String>> = projects
            .projects
            .iter()
            .flat_map(|project| project.directories.keys())
            .map(|directory| (directory.as_str(), BTreeMap::new()))
            .collect();
        let catalogue = Catalogue {
            messages: messages.clone(),
        };
        for (key, text) in catalogue.leaves() {
            let (directory, _) = projects
                .place(&key)
                .and_then(|project| project.place(&key))
                .unwrap_or_else(|| panic!("{key} lands nowhere"));
            split.get_mut(directory).unwrap().insert(key, text);
        }
        for (directory, keys) in &split {
            let dir = root.join(directory);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join(format!("{lang}.json")),
                pretty(&Value::Object(nest(keys))),
            )
            .unwrap();
        }
    }

    fn langs() -> Vec<String> {
        vec!["en".to_owned(), "ru".to_owned()]
    }

    #[test]
    fn missing_reports_a_key_absent_from_ru_and_accepts_a_derived_key_the_shared_fallback_satisfies()
     {
        let projects = shop_projects();
        let en = complete_en();
        let ru = ru_of(&en);
        let (keep, dump) = self::dump(en.clone(), filled(&en, &ru));
        let root = keep.path().join("root");
        write_dictionaries(&projects, &root, "en", &en);
        write_dictionaries(&projects, &root, "ru", &ru);
        let clean = missing(&dump, &projects, &root, &langs()).unwrap();
        assert!(clean.green(), "{clean}");
        assert_eq!(clean.to_string(), "");
        assert_eq!(clean.summary(), "25 keys translated in en, ru");

        let mut ru = ru_of(&en);
        ru["shop"]["item"]["state"]
            .as_object_mut()
            .unwrap()
            .remove("closed");
        ru["keel"]["column"].as_object_mut().unwrap().remove("name");
        ru["menu"].as_object_mut().unwrap().remove("shop");
        let (keep, dump) = self::dump(en.clone(), filled(&en, &ru));
        let root = keep.path().join("root");
        write_dictionaries(&projects, &root, "en", &en);
        write_dictionaries(&projects, &root, "ru", &ru);
        let served = Catalogue::of_dump(&dump, "ru").unwrap();
        assert_eq!(served.get("shop.item.state.closed"), Some("Closed"));
        assert_eq!(served.get("shop.item.state.open"), Some("ru:Open"));
        assert_eq!(
            served.leaves().keys().collect::<Vec<_>>(),
            Catalogue::of_dump(&dump, "en")
                .unwrap()
                .leaves()
                .keys()
                .collect::<Vec<_>>()
        );
        let gaps = missing(&dump, &projects, &root, &langs()).unwrap();
        assert!(!gaps.green());
        assert_eq!(
            gaps.to_string(),
            "ru: keel.column.name (en catalogue)\n\
             ru: menu.shop (en catalogue)\n\
             ru: shop.item.state.closed (en catalogue)\n\
             ru: shop.item.column.name (function shop.item.list column name)\n"
        );
        assert_eq!(
            gaps.summary(),
            "4 of 25 keys without a translation: 4 in ru"
        );
        assert!(gaps.gaps["en"].is_empty());

        let mut without_shared = en.clone();
        without_shared["keel"]["column"]
            .as_object_mut()
            .unwrap()
            .remove("state");
        without_shared["shop"]["item"]
            .as_object_mut()
            .unwrap()
            .remove("note.placeholder");
        let (keep, dump) = self::dump(without_shared.clone(), ru_of(&without_shared));
        let root = keep.path().join("root");
        write_dictionaries(&projects, &root, "en", &without_shared);
        write_dictionaries(&projects, &root, "ru", &ru_of(&without_shared));
        let derived = missing(&dump, &projects, &root, &langs()).unwrap();
        let en_keys: Vec<&str> = derived.gaps["en"]
            .iter()
            .map(|gap| gap.key.as_str())
            .collect();
        assert_eq!(
            en_keys,
            ["shop.item.note.placeholder", "shop.item.column.state"]
        );
        assert_eq!(
            derived.gaps["en"][0].owner,
            "function shop.item.edit field note x-ui.placeholder"
        );
        assert_eq!(
            derived.gaps["en"][1].owner,
            "function shop.item.list column state"
        );
        assert_eq!(derived.gaps["ru"].len(), 2);
        assert_eq!(
            derived.summary(),
            "4 of 24 keys without a translation: 2 in en, 2 in ru"
        );

        let only_en = missing(&dump, &projects, &root, &["en".to_owned()]).unwrap();
        assert_eq!(only_en.gaps.len(), 1);
        assert_eq!(
            only_en.summary(),
            "2 of 24 keys without a translation: 2 in en"
        );

        let unknown = missing(&dump, &projects, &root, &["de".to_owned()]).unwrap_err();
        assert!(matches!(unknown, Error::Io { .. }), "{unknown}");
        let nowhere =
            missing(Path::new("/nonexistent/dump"), &projects, &root, &langs()).unwrap_err();
        assert!(matches!(nowhere, Error::Io { .. }), "{nowhere}");
        fs::write(root.join("crates/kernel/i18n/ru.json"), "[]\n").unwrap();
        let shape = missing(&dump, &projects, &root, &langs()).unwrap_err();
        assert!(
            matches!(&shape, Error::Shape { expected, .. } if *expected == "a dictionary object"),
            "{shape}"
        );
    }
}
