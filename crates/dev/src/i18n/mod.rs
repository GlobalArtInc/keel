mod extract;
mod keys;
mod missing;
mod pull;

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use keel_types::Level;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::descriptors::I18N_DIR;

pub use extract::extract;
pub use missing::missing;
pub use pull::{Pull, pull};

pub const PROJECTS_FILE: &str = "contracts/i18n.json";
const SHARED_PREFIX: &str = "keel";
pub const REQUIRED_LANGUAGE: &str = "en";
pub const DUMP_LEVEL: Level = Level::Admin;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("{}: {source}", path.display())]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("{}: expected {expected}", path.display())]
    Shape {
        path: PathBuf,
        expected: &'static str,
    },
    #[error("{PROJECTS_FILE} names no project {0}")]
    UnknownProject(String),
    #[error("the key {key} of {origin} lands in no directory of {PROJECTS_FILE}")]
    Unplaced { origin: String, key: String },
    #[error("the prefix {prefix} is claimed by both {first} and {second}")]
    PrefixTwice {
        prefix: String,
        first: String,
        second: String,
    },
    #[error("the environment variable {0} is not set")]
    Env(String),
    #[error("the HTTP client failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("GET {url} answered {status}: {body}")]
    Answered {
        url: String,
        status: u16,
        body: String,
    },
    #[error("GET {url} answered {expected}, not a {found}")]
    Payload {
        url: String,
        expected: &'static str,
        found: &'static str,
    },
}

#[derive(Debug, Deserialize)]
pub struct Projects {
    pub url: String,
    pub projects: Vec<Project>,
}

#[derive(Debug, Deserialize)]
pub struct Project {
    pub name: String,
    pub id_env: String,
    pub directories: BTreeMap<String, Vec<String>>,
}

impl Projects {
    pub fn load(path: &Path) -> Result<Self, Error> {
        let text = fs::read_to_string(path).map_err(io(path))?;
        let projects: Self = serde_json::from_str(&text).map_err(json(path))?;
        let mut owners: BTreeMap<&str, &str> = BTreeMap::new();
        for project in &projects.projects {
            for prefix in project.directories.values().flatten() {
                if let Some(first) = owners.insert(prefix, &project.name)
                    && first != project.name
                {
                    return Err(Error::PrefixTwice {
                        prefix: prefix.clone(),
                        first: first.to_owned(),
                        second: project.name.clone(),
                    });
                }
            }
        }
        Ok(projects)
    }

    pub fn named(&self, name: &str) -> Result<&Project, Error> {
        self.projects
            .iter()
            .find(|project| project.name == name)
            .ok_or_else(|| Error::UnknownProject(name.to_owned()))
    }

    pub fn place(&self, key: &str) -> Option<&Project> {
        self.projects
            .iter()
            .filter_map(|project| project.place(key).map(|(_, prefix)| (project, prefix)))
            .max_by_key(|(_, prefix)| prefix.len())
            .map(|(project, _)| project)
    }
}

impl Project {
    fn place(&self, key: &str) -> Option<(&str, &str)> {
        self.directories
            .iter()
            .flat_map(|(directory, prefixes)| {
                prefixes
                    .iter()
                    .map(move |prefix| (directory.as_str(), prefix.as_str()))
            })
            .filter(|(_, prefix)| covers(prefix, key))
            .max_by_key(|(_, prefix)| prefix.len())
    }
}

fn covers(prefix: &str, key: &str) -> bool {
    key == prefix
        || key
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('.'))
}

pub fn shared_of(key: &str, tail: &str) -> Option<String> {
    let shared = format!("{SHARED_PREFIX}.{tail}");
    (shared != key).then_some(shared)
}

pub struct Catalogue {
    messages: Map<String, Value>,
}

impl Catalogue {
    pub fn of_dump(dump: &Path, lang: &str) -> Result<Self, Error> {
        let path = dump
            .join(DUMP_LEVEL.as_str())
            .join(I18N_DIR)
            .join(format!("{lang}.json"));
        let value = read_json(&path)?;
        let messages = match value.get("messages") {
            Some(Value::Object(messages)) => messages.clone(),
            _ => {
                return Err(Error::Shape {
                    path,
                    expected: "a catalogue with a messages object",
                });
            }
        };
        Ok(Self { messages })
    }

    fn of_dictionaries(projects: &Projects, root: &Path, lang: &str) -> Result<Self, Error> {
        let mut flat = BTreeMap::new();
        for directory in projects
            .projects
            .iter()
            .flat_map(|project| project.directories.keys())
        {
            let path = root.join(directory).join(format!("{lang}.json"));
            let Value::Object(dictionary) = read_json(&path)? else {
                return Err(Error::Shape {
                    path,
                    expected: "a dictionary object",
                });
            };
            collect(&dictionary, &mut Vec::new(), &mut flat);
        }
        let messages = flat
            .into_iter()
            .map(|(key, text)| (key, Value::String(text)))
            .collect();
        Ok(Self { messages })
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        find(&self.messages, key).and_then(Value::as_str)
    }

    pub fn leaves(&self) -> BTreeMap<String, String> {
        let mut into = BTreeMap::new();
        collect(&self.messages, &mut Vec::new(), &mut into);
        into
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

fn collect(
    messages: &Map<String, Value>,
    path: &mut Vec<String>,
    into: &mut BTreeMap<String, String>,
) {
    for (key, value) in messages {
        path.push(key.clone());
        match value {
            Value::Object(nested) => collect(nested, path, into),
            Value::String(text) => {
                into.insert(path.join("."), text.clone());
            }
            _ => {}
        }
        path.pop();
    }
}

pub fn nest(flat: &BTreeMap<String, String>) -> Map<String, Value> {
    let mut root = Map::new();
    for (key, text) in flat {
        insert(&mut root, key, text);
    }
    root
}

fn insert(node: &mut Map<String, Value>, key: &str, text: &str) {
    if let Some((head, tail)) = key.split_once('.') {
        if !node.contains_key(head) {
            node.insert(head.to_owned(), Value::Object(Map::new()));
        }
        if let Some(Value::Object(child)) = node.get_mut(head) {
            insert(child, tail, text);
            return;
        }
    }
    node.insert(key.to_owned(), Value::String(text.to_owned()));
}

pub fn pretty(value: &Value) -> String {
    let mut text = serde_json::to_string_pretty(value).unwrap_or_default();
    text.push('\n');
    text
}

pub fn read_json(path: &Path) -> Result<Value, Error> {
    let text = fs::read_to_string(path).map_err(io(path))?;
    serde_json::from_str(&text).map_err(json(path))
}

fn io(path: &Path) -> impl FnOnce(io::Error) -> Error {
    let path = path.to_path_buf();
    move |source| Error::Io { path, source }
}

fn json(path: &Path) -> impl FnOnce(serde_json::Error) -> Error {
    let path = path.to_path_buf();
    move |source| Error::Json { path, source }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    pub(super) fn projects() -> Projects {
        Projects {
            url: "https://api.ownlate.test".to_owned(),
            projects: vec![
                Project {
                    name: "keel-core".to_owned(),
                    id_env: "KEEL_OWNLATE_PROJECT_CORE".to_owned(),
                    directories: BTreeMap::from([
                        (
                            "crates/kernel/i18n".to_owned(),
                            vec![
                                "keel".to_owned(),
                                "phx".to_owned(),
                                "menu.system".to_owned(),
                            ],
                        ),
                        (
                            "crates/system/i18n/auth".to_owned(),
                            vec!["auth".to_owned()],
                        ),
                    ]),
                },
                Project {
                    name: "keel-demo".to_owned(),
                    id_env: "KEEL_OWNLATE_PROJECT_DEMO".to_owned(),
                    directories: BTreeMap::from([
                        (
                            "products/demo/i18n".to_owned(),
                            vec!["product.demo".to_owned(), "menu.demo".to_owned()],
                        ),
                        (
                            "products/demo/i18n/note".to_owned(),
                            vec!["note".to_owned()],
                        ),
                    ]),
                },
            ],
        }
    }

    #[test]
    fn a_key_lands_in_the_directory_of_its_longest_prefix_and_a_bare_group_nowhere() {
        let projects = projects();
        let placed = |key: &str| {
            projects.place(key).and_then(|project| {
                project
                    .place(key)
                    .map(|(directory, _)| (project.name.clone(), directory.to_owned()))
            })
        };
        assert_eq!(
            placed("keel.error.validation"),
            Some(("keel-core".to_owned(), "crates/kernel/i18n".to_owned()))
        );
        assert_eq!(
            placed("menu.system"),
            Some(("keel-core".to_owned(), "crates/kernel/i18n".to_owned()))
        );
        assert_eq!(
            placed("menu.demo"),
            Some(("keel-demo".to_owned(), "products/demo/i18n".to_owned()))
        );
        assert_eq!(
            placed("note.list.title"),
            Some(("keel-demo".to_owned(), "products/demo/i18n/note".to_owned()))
        );
        assert_eq!(
            placed("auth.login"),
            Some(("keel-core".to_owned(), "crates/system/i18n/auth".to_owned()))
        );
        assert_eq!(placed("menu"), None);
        assert_eq!(placed("keeled.thing"), None);
        assert_eq!(placed("notebook.x"), None);
        assert!(covers("keel", "keel"));
        assert!(!covers("keel", "keels"));
        assert!(projects.named("keel-demo").is_ok());
        assert!(matches!(
            projects.named("keel-dns"),
            Err(Error::UnknownProject(name)) if name == "keel-dns"
        ));
    }

    #[test]
    fn the_projects_file_is_parsed_and_a_prefix_claimed_twice_is_refused() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("i18n.json");
        fs::write(
            &path,
            json!({
                "url": "https://api.ownlate.test",
                "projects": [
                    { "name": "keel-core", "id_env": "CORE", "directories": { "crates/kernel/i18n": ["keel"] } },
                    { "name": "keel-demo", "id_env": "DEMO", "directories": { "products/demo/i18n": ["note", "keel"] } }
                ]
            })
            .to_string(),
        )
        .unwrap();
        let error = Projects::load(&path).unwrap_err();
        match error {
            Error::PrefixTwice {
                prefix,
                first,
                second,
            } => {
                assert_eq!(prefix, "keel");
                assert_eq!(first, "keel-core");
                assert_eq!(second, "keel-demo");
            }
            other => panic!("expected the prefix conflict, got {other}"),
        }
        fs::write(
            &path,
            json!({
                "url": "https://api.ownlate.test",
                "projects": [
                    { "name": "keel-core", "id_env": "CORE", "directories": { "crates/kernel/i18n": ["keel"] } }
                ]
            })
            .to_string(),
        )
        .unwrap();
        let projects = Projects::load(&path).unwrap();
        assert_eq!(projects.url, "https://api.ownlate.test");
        assert_eq!(projects.projects[0].id_env, "CORE");
        let missing = Projects::load(&temporary.path().join("nowhere.json")).unwrap_err();
        assert!(matches!(missing, Error::Io { .. }), "{missing}");
    }

    #[test]
    fn nesting_keeps_a_leaf_beside_its_own_children_as_a_dotted_key_and_round_trips() {
        let flat = BTreeMap::from([
            ("auth.login".to_owned(), "Login".to_owned()),
            ("auth.login.title".to_owned(), "Sign in".to_owned()),
            (
                "auth.login.state.authenticated".to_owned(),
                "Signed in".to_owned(),
            ),
            ("auth.logout.title".to_owned(), "Sign out".to_owned()),
            ("menu.system".to_owned(), "System".to_owned()),
            ("z".to_owned(), "Last".to_owned()),
        ]);
        let nested = nest(&flat);
        assert_eq!(
            Value::Object(nested.clone()),
            json!({
                "auth": {
                    "login": "Login",
                    "login.state.authenticated": "Signed in",
                    "login.title": "Sign in",
                    "logout": { "title": "Sign out" }
                },
                "menu": { "system": "System" },
                "z": "Last"
            })
        );
        let catalogue = Catalogue { messages: nested };
        assert_eq!(catalogue.get("auth.login"), Some("Login"));
        assert_eq!(catalogue.get("auth.login.title"), Some("Sign in"));
        assert_eq!(
            catalogue.get("auth.login.state.authenticated"),
            Some("Signed in")
        );
        assert_eq!(catalogue.get("auth.logout.title"), Some("Sign out"));
        assert_eq!(catalogue.get("auth.logout"), None);
        assert_eq!(catalogue.get("auth"), None);
        assert_eq!(catalogue.leaves(), flat);
        let text = pretty(&Value::Object(nest(&flat)));
        assert!(
            text.starts_with("{\n  \"auth\": {\n    \"login\": \"Login\",\n"),
            "{text}"
        );
        assert!(text.ends_with("\"z\": \"Last\"\n}\n"), "{text}");
        assert_eq!(pretty(&Value::Object(Map::new())), "{}\n");
        assert_eq!(
            shared_of("note.column.title", "column.title"),
            Some("keel.column.title".to_owned())
        );
        assert_eq!(shared_of("keel.name", "name"), None);
    }
}
