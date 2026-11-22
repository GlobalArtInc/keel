use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use keel_types::{Boot, BootMenuItem, Descriptor};

use super::{Catalogue, DUMP_LEVEL, Error, Projects, REQUIRED_LANGUAGE, keys, read_json};
use crate::descriptors::{BOOT_FILE, FUNCTIONS_DIR};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Required {
    pub owner: String,
    pub key: String,
    pub shared: Option<String>,
}

pub fn required(dump: &Path) -> Result<Vec<Required>, Error> {
    let level = dump.join(DUMP_LEVEL.as_str());
    let mut required = Vec::new();
    let functions = level.join(FUNCTIONS_DIR);
    for path in sorted_files(&functions)? {
        let descriptor: Descriptor = parse(&path)?;
        let owner = format!("function {}", descriptor.name);
        if let Some(ui) = &descriptor.ui {
            required.push(Required {
                owner: owner.clone(),
                key: ui.title.clone(),
                shared: None,
            });
            if let Some(empty) = &ui.empty {
                required.push(Required {
                    owner: owner.clone(),
                    key: empty.title.clone(),
                    shared: None,
                });
            }
        }
        for derived in keys::of(&descriptor) {
            required.push(Required {
                owner: format!("{owner} {}", derived.member),
                key: derived.key,
                shared: derived.shared,
            });
        }
    }
    let boot: Boot = parse(&level.join(BOOT_FILE))?;
    for group in &boot.menu {
        required.push(Required {
            owner: format!("menu group {}", group.id),
            key: group.label.clone(),
            shared: None,
        });
        for item in &group.items {
            require_item(&mut required, item);
        }
    }
    for product in &boot.products {
        required.push(Required {
            owner: format!("product {}", product.id),
            key: product.name.clone(),
            shared: None,
        });
    }
    Ok(required)
}

fn require_item(required: &mut Vec<Required>, item: &BootMenuItem) {
    let kind = if item.children.is_empty() {
        "menu node"
    } else {
        "menu section"
    };
    required.push(Required {
        owner: format!("{kind} {}", item.path),
        key: item.label.clone(),
        shared: None,
    });
    for child in &item.children {
        require_item(required, child);
    }
}

fn sorted_files(dir: &Path) -> Result<Vec<PathBuf>, Error> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(dir).map_err(super::io(dir))? {
        let entry = entry.map_err(super::io(dir))?;
        if entry.path().extension().is_some_and(|ext| ext == "json") {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

fn parse<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, Error> {
    let value = read_json(path)?;
    serde_json::from_value(value).map_err(super::json(path))
}

#[derive(Debug, PartialEq, Eq)]
pub struct Extracted {
    files: Vec<(String, usize)>,
    dir: PathBuf,
}

impl fmt::Display for Extracted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (project, keys) in &self.files {
            writeln!(f, "{project}: {keys} keys")?;
        }
        write!(f, "written to {}", self.dir.display())
    }
}

pub fn extract(
    dump: &Path,
    projects: &Projects,
    project: Option<&str>,
    lang: &str,
    root: &Path,
    out: &Path,
) -> Result<Extracted, Error> {
    let only = project.map(|name| projects.named(name)).transpose()?;
    let (catalogue, origin) = if lang == REQUIRED_LANGUAGE {
        (
            Catalogue::of_dump(dump, lang)?,
            format!("the {} {lang} catalogue of the dump", DUMP_LEVEL.as_str()),
        )
    } else {
        (
            Catalogue::of_dictionaries(projects, root, lang)?,
            format!("the {lang} dictionaries under {}", root.display()),
        )
    };
    let mut per_project: BTreeMap<&str, BTreeMap<String, String>> = projects
        .projects
        .iter()
        .filter(|candidate| only.is_none_or(|only| only.name == candidate.name))
        .map(|candidate| (candidate.name.as_str(), BTreeMap::new()))
        .collect();
    for (key, text) in catalogue.leaves() {
        let Some(owner) = projects.place(&key) else {
            return Err(Error::Unplaced { origin, key });
        };
        if let Some(keys) = per_project.get_mut(owner.name.as_str()) {
            keys.insert(key, text);
        }
    }
    fs::create_dir_all(out).map_err(super::io(out))?;
    let mut files = Vec::with_capacity(per_project.len());
    for (name, keys) in &per_project {
        let path = out.join(format!("{name}.{lang}.json"));
        let value = serde_json::Value::Object(
            keys.iter()
                .map(|(key, text)| (key.clone(), serde_json::Value::String(text.clone())))
                .collect(),
        );
        fs::write(&path, super::pretty(&value)).map_err(super::io(&path))?;
        files.push(((*name).to_owned(), keys.len()));
    }
    Ok(Extracted {
        files,
        dir: out.to_path_buf(),
    })
}

#[cfg(test)]
pub(super) mod tests {
    use std::collections::BTreeMap;

    use serde_json::Value;

    use super::super::tests::projects as fixture;
    use super::*;

    pub(in super::super) fn repository() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn read(path: &Path) -> BTreeMap<String, String> {
        let value: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        value
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, text)| (key.clone(), text.as_str().unwrap().to_owned()))
            .collect()
    }

    #[test]
    fn a_section_item_requires_its_own_label_and_the_labels_of_its_tabs() {
        let tab = |path: &str, label: &str| BootMenuItem {
            path: path.into(),
            label: label.into(),
            func: "keel.principal.list".into(),
            kind: None,
            icon: None,
            badge: None,
            children: Vec::new(),
        };
        let section = BootMenuItem {
            children: vec![tab("access/users/principals", "keel.principal.list.title")],
            ..tab("access/users", "keel.menu.users")
        };
        let mut required = Vec::new();
        require_item(&mut required, &section);
        require_item(&mut required, &tab("system/health", "keel.health.title"));
        let pairs: Vec<(&str, &str)> = required
            .iter()
            .map(|required| (required.owner.as_str(), required.key.as_str()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("menu section access/users", "keel.menu.users"),
                (
                    "menu node access/users/principals",
                    "keel.principal.list.title"
                ),
                ("menu node system/health", "keel.health.title"),
            ]
        );
    }

    #[test]
    fn extract_derives_the_keys_of_a_dump_by_the_key_table_and_keeps_its_explicit_ones() {
        let repository = repository();
        let dump = repository.join("contracts/dump");
        let required = required(&dump).unwrap();
        let find = |key: &str| {
            required
                .iter()
                .find(|required| required.key == key)
                .unwrap_or_else(|| panic!("{key} is not required"))
        };
        let column = find("note.column.kind");
        assert_eq!(column.owner, "function note.list column kind");
        assert_eq!(column.shared.as_deref(), Some("keel.column.kind"));
        let value = find("note.kind.memo");
        assert!(
            value.owner == "function note.edit field kind value memo"
                || value.owner == "function note.list column kind value memo",
            "{}",
            value.owner
        );
        assert_eq!(value.shared.as_deref(), Some("keel.kind.memo"));
        assert_eq!(
            find("note.action.export_task").owner,
            "function note.list action export_task"
        );
        assert_eq!(
            find("note.page.owner").owner,
            "function note.edit page owner"
        );
        assert_eq!(find("note.account").shared.as_deref(), Some("keel.account"));
        assert_eq!(find("note.kind").shared.as_deref(), Some("keel.kind"));
        for (key, owner) in [
            ("note.list.title", "function note.list"),
            ("note.empty", "function note.list"),
            ("menu.demo", "menu group demo"),
            ("menu.system", "menu group system"),
            ("note.tag.list.title", "menu node demo/tags"),
            ("product.demo.brand", "product demo"),
        ] {
            let explicit = find(key);
            assert!(explicit.shared.is_none(), "{key}");
            assert!(
                required
                    .iter()
                    .any(|required| required.key == key && required.owner == owner),
                "{key} is not owned by {owner}"
            );
        }
        assert!(
            !required
                .iter()
                .any(|required| required.key.starts_with("keel.error.")),
            "error keys are not derived from descriptors"
        );

        let projects = Projects::load(&repository.join(super::super::PROJECTS_FILE)).unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let out = temporary.path().join("extract");
        let extracted = extract(&dump, &projects, None, "en", &repository, &out).unwrap();
        let names: Vec<&str> = extracted
            .files
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(names, ["keel-core", "keel-demo", "keel-dns", "keel-panel"]);
        let core = read(&out.join("keel-core.en.json"));
        let demo = read(&out.join("keel-demo.en.json"));
        assert_eq!(extracted.files[0].1, core.len());
        assert_eq!(extracted.files[1].1, demo.len());
        assert_eq!(
            demo.get("note.list.title").map(String::as_str),
            Some("Notes")
        );
        assert_eq!(demo.get("menu.demo").map(String::as_str), Some("Demo"));
        assert!(demo.contains_key("product.demo.brand"));
        assert!(demo.contains_key("note.kind.memo"));
        assert!(demo.contains_key("note.column.kind"));
        assert!(
            !demo.contains_key("note.kind"),
            "the shared entry keel.kind covers it"
        );
        assert!(core.contains_key("keel.kind"));
        assert!(core.contains_key("keel.error.validation"));
        assert!(core.contains_key("auth.login.title"));
        assert!(core.contains_key("menu.system"));
        assert!(core.keys().all(|key| !key.starts_with("note.")));
        assert!(demo.keys().all(|key| !key.starts_with("keel.")));
        let text = fs::read_to_string(out.join("keel-demo.en.json")).unwrap();
        assert!(
            text.starts_with("{\n  \"menu.demo\": \"Demo\",\n"),
            "{text}"
        );
        assert!(text.ends_with("\"\n}\n"), "{text}");
        assert!(extracted.to_string().starts_with(&format!(
            "keel-core: {} keys\nkeel-demo: {} keys\n",
            core.len(),
            demo.len()
        )));

        let only = extract(
            &dump,
            &projects,
            Some("keel-demo"),
            "ru",
            &repository,
            &temporary.path().join("only"),
        )
        .unwrap();
        assert_eq!(only.files.len(), 1);
        assert_eq!(only.files[0].0, "keel-demo");
        assert!(!temporary.path().join("only/keel-core.ru.json").exists());
        let russian = read(&temporary.path().join("only/keel-demo.ru.json"));
        assert_eq!(russian.len(), demo.len());
        assert_ne!(russian["note.list.title"], demo["note.list.title"]);
        assert!(matches!(
            extract(&dump, &projects, Some("keel-nowhere"), "en", &repository, &out),
            Err(Error::UnknownProject(name)) if name == "keel-nowhere"
        ));
        assert!(matches!(
            extract(&dump, &projects, None, "de", &repository, &out),
            Err(Error::Io { .. })
        ));
        assert!(matches!(
            extract(&dump, &projects, None, "ru", temporary.path(), &out),
            Err(Error::Io { .. })
        ));
        let partial = fixture();
        match extract(&dump, &partial, None, "en", &repository, &out).unwrap_err() {
            Error::Unplaced { origin, key } => {
                assert_eq!(origin, "the admin en catalogue of the dump");
                assert_eq!(key, "audit.column.actorLogin");
            }
            other => panic!("expected an unplaced key, got {other}"),
        }
    }
}
