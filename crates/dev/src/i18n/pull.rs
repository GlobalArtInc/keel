use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::StatusCode;
use serde_json::{Map, Value};

use super::{Error, Project, Projects, nest, pretty};
use crate::descriptors::{LANGUAGES, kind_of};

const URL_ENV: &str = "KEEL_OWNLATE_URL";
const TOKEN_ENV: &str = "KEEL_OWNLATE_TOKEN";
const MAP_PATH: &str = "/public/v1/segments/translations-map";
const TIMEOUT: Duration = Duration::from_secs(60);
const BODY_EXCERPT: usize = 300;

type Release = BTreeMap<String, BTreeMap<String, String>>;

pub struct Pull {
    url: String,
    token: String,
    ids: BTreeMap<String, String>,
}

impl Pull {
    pub fn from_env(projects: &Projects) -> Result<Self, Error> {
        Self::resolve(projects, |name| std::env::var(name).ok())
    }

    fn resolve(
        projects: &Projects,
        variable: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, Error> {
        let url = variable(URL_ENV)
            .filter(|url| !url.is_empty())
            .unwrap_or_else(|| projects.url.clone());
        let token = variable(TOKEN_ENV)
            .filter(|token| !token.is_empty())
            .ok_or_else(|| Error::Env(TOKEN_ENV.to_owned()))?;
        let mut ids = BTreeMap::new();
        for project in &projects.projects {
            let id = variable(&project.id_env)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| Error::Env(project.id_env.clone()))?;
            ids.insert(project.name.clone(), id);
        }
        Ok(Self {
            url: url.trim_end_matches('/').to_owned(),
            token,
            ids,
        })
    }

    fn map_url(&self, project: &Project) -> String {
        let id = self.ids.get(&project.name).map_or("", String::as_str);
        format!("{}{MAP_PATH}?projectId={id}", self.url)
    }

    async fn fetch(&self, project: &Project) -> Result<Release, Error> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .no_proxy()
            .timeout(TIMEOUT)
            .build()?;
        let url = self.map_url(project);
        let response = http
            .get(&url)
            .bearer_auth(&self.token)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if status != StatusCode::OK {
            return Err(Error::Answered {
                url,
                status: status.as_u16(),
                body: body.chars().take(BODY_EXCERPT).collect(),
            });
        }
        let value: Value = serde_json::from_str(&body).map_err(|_| Error::Payload {
            url: url.clone(),
            expected: "a JSON object",
            found: "body that is not JSON",
        })?;
        release_of(&url, &value)
    }
}

impl fmt::Debug for Pull {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pull")
            .field("url", &self.url)
            .field("token", &"***")
            .field("ids", &self.ids)
            .finish()
    }
}

fn release_of(url: &str, value: &Value) -> Result<Release, Error> {
    let Value::Object(files) = value else {
        return Err(Error::Payload {
            url: url.to_owned(),
            expected: "an object of translation files",
            found: kind_of(value),
        });
    };
    let mut release = Release::new();
    for by_lang in files.values() {
        let Value::Object(by_lang) = by_lang else {
            return Err(Error::Payload {
                url: url.to_owned(),
                expected: "an object of languages under each file",
                found: kind_of(by_lang),
            });
        };
        for (lang, messages) in by_lang {
            let Value::Object(messages) = messages else {
                return Err(Error::Payload {
                    url: url.to_owned(),
                    expected: "an object of keys under each language",
                    found: kind_of(messages),
                });
            };
            let into = release.entry(lang.clone()).or_default();
            flatten(messages, &mut Vec::new(), into);
        }
    }
    Ok(release)
}

fn flatten(
    messages: &Map<String, Value>,
    path: &mut Vec<String>,
    into: &mut BTreeMap<String, String>,
) {
    for (key, value) in messages {
        path.push(key.clone());
        match value {
            Value::Object(nested) => flatten(nested, path, into),
            Value::String(text) if !text.is_empty() => {
                into.insert(path.join("."), text.clone());
            }
            _ => {}
        }
        path.pop();
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Pulled {
    written: Vec<PathBuf>,
    unchanged: usize,
}

impl fmt::Display for Pulled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for path in &self.written {
            writeln!(f, "wrote {}", path.display())?;
        }
        write!(
            f,
            "{} dictionaries written, {} unchanged",
            self.written.len(),
            self.unchanged
        )
    }
}

fn write(project: &Project, release: &Release, out: &Path) -> Result<Pulled, Error> {
    let mut split: BTreeMap<(&str, &str), BTreeMap<String, String>> = BTreeMap::new();
    for directory in project.directories.keys() {
        for lang in LANGUAGES {
            split.insert((directory.as_str(), lang), BTreeMap::new());
        }
    }
    for (lang, messages) in release {
        if !LANGUAGES.contains(&lang.as_str()) {
            continue;
        }
        for (key, text) in messages {
            let Some((directory, _)) = project.place(key) else {
                return Err(Error::Unplaced {
                    origin: project.name.clone(),
                    key: key.clone(),
                });
            };
            if let Some(keys) = split.get_mut(&(directory, lang.as_str())) {
                keys.insert(key.clone(), text.clone());
            }
        }
    }
    let mut pulled = Pulled::default();
    for ((directory, lang), keys) in &split {
        let dir = out.join(directory);
        fs::create_dir_all(&dir).map_err(super::io(&dir))?;
        let path = dir.join(format!("{lang}.json"));
        let text = pretty(&Value::Object(nest(keys)));
        let current = match fs::read_to_string(&path) {
            Ok(current) => Some(current),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(source) => return Err(Error::Io { path, source }),
        };
        if current.as_deref() == Some(text.as_str()) {
            pulled.unchanged += 1;
            continue;
        }
        fs::write(&path, text).map_err(super::io(&path))?;
        pulled.written.push(path);
    }
    Ok(pulled)
}

pub async fn pull(projects: &Projects, pull: &Pull, out: &Path) -> Result<Pulled, Error> {
    let mut total = Pulled::default();
    for project in &projects.projects {
        let release = pull.fetch(project).await?;
        let pulled = write(project, &release, out)?;
        total.written.extend(pulled.written);
        total.unchanged += pulled.unchanged;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::Arc;

    use axum::extract::{Query, State};
    use axum::http::{HeaderMap, StatusCode, header};
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;
    use axum::{Json, Router};
    use serde_json::json;
    use std::collections::HashMap;
    use tokio::sync::Mutex;

    use super::super::extract::tests::repository;
    use super::super::tests::projects;
    use super::super::{Catalogue, read_json};
    use super::*;

    fn leaves_of(path: &Path) -> BTreeMap<String, String> {
        let Value::Object(messages) = read_json(path).unwrap() else {
            panic!("{} is not an object", path.display());
        };
        Catalogue { messages }.leaves()
    }

    fn copy_tree(project: &Project, from: &Path, to: &Path) {
        for directory in project.directories.keys() {
            fs::create_dir_all(to.join(directory)).unwrap();
            for lang in LANGUAGES {
                let file = Path::new(directory).join(format!("{lang}.json"));
                fs::copy(from.join(&file), to.join(&file)).unwrap();
            }
        }
    }

    fn release_of_tree(project: &Project, root: &Path) -> Release {
        let mut release = Release::new();
        for directory in project.directories.keys() {
            for lang in LANGUAGES {
                let file = root.join(directory).join(format!("{lang}.json"));
                release
                    .entry(lang.to_owned())
                    .or_default()
                    .extend(leaves_of(&file));
            }
        }
        release
    }

    #[test]
    fn pull_splits_a_release_by_prefix_into_the_dictionaries_and_leaves_a_synced_tree_unchanged() {
        let repository = repository();
        let projects = Projects::load(&repository.join(super::super::PROJECTS_FILE)).unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let out = temporary.path().join("tree");
        let mut dictionaries = 0;
        for project in &projects.projects {
            copy_tree(project, &repository, &out);
            let release = release_of_tree(project, &repository);
            assert!(release["en"].len() >= 2, "{}", project.name);
            assert_eq!(release["en"].len(), release["ru"].len(), "{}", project.name);
            let synced = write(project, &release, &out).unwrap();
            assert!(synced.written.is_empty(), "{synced}");
            assert_eq!(
                synced.unchanged,
                project.directories.len() * LANGUAGES.len()
            );
            dictionaries += synced.unchanged;
        }
        assert_eq!(dictionaries, 48);

        let demo = projects.named("keel-demo").unwrap();
        let mut release = release_of_tree(demo, &repository);
        release
            .get_mut("ru")
            .unwrap()
            .insert("note.list.title".to_owned(), "Записки".to_owned());
        release
            .get_mut("en")
            .unwrap()
            .insert("note.tag.column.tag.hint".to_owned(), "A tag".to_owned());
        release
            .get_mut("en")
            .unwrap()
            .insert("menu.demo".to_owned(), "Demo!".to_owned());
        release.insert(
            "de".to_owned(),
            BTreeMap::from([("note.list.title".to_owned(), "Notizen".to_owned())]),
        );
        let changed = write(demo, &release, &out).unwrap();
        assert_eq!(
            changed.written,
            [
                out.join("products/demo/i18n/en.json"),
                out.join("products/demo/i18n/note/en.json"),
                out.join("products/demo/i18n/note/ru.json"),
            ]
        );
        assert_eq!(changed.unchanged, 1);
        assert!(
            changed
                .to_string()
                .ends_with("3 dictionaries written, 1 unchanged"),
            "{changed}"
        );
        assert!(!out.join("products/demo/i18n/note/de.json").exists());
        let note_en = read_json(&out.join("products/demo/i18n/note/en.json")).unwrap();
        assert_eq!(note_en["note"]["tag"]["column"]["tag"], json!("Tag"));
        assert_eq!(note_en["note"]["tag"]["column"]["tag.hint"], json!("A tag"));
        let note_ru = read_json(&out.join("products/demo/i18n/note/ru.json")).unwrap();
        assert_eq!(note_ru["note"]["list"]["title"], json!("Записки"));
        let demo_en = read_json(&out.join("products/demo/i18n/en.json")).unwrap();
        assert_eq!(
            demo_en,
            json!({ "menu": { "demo": "Demo!" }, "product": { "demo": { "brand": "Keel Demo" } } })
        );
        let text = fs::read_to_string(out.join("products/demo/i18n/en.json")).unwrap();
        assert_eq!(
            text,
            "{\n  \"menu\": {\n    \"demo\": \"Demo!\"\n  },\n  \"product\": {\n    \"demo\": {\n      \"brand\": \"Keel Demo\"\n    }\n  }\n}\n"
        );
        let again = write(demo, &release, &out).unwrap();
        assert!(again.written.is_empty(), "{again}");

        release
            .get_mut("en")
            .unwrap()
            .insert("dns.zone.list.title".to_owned(), "Zones".to_owned());
        match write(demo, &release, &out).unwrap_err() {
            Error::Unplaced { origin, key } => {
                assert_eq!(origin, "keel-demo");
                assert_eq!(key, "dns.zone.list.title");
            }
            other => panic!("expected an unplaced key, got {other}"),
        }

        let empty = Release::new();
        let fresh = temporary.path().join("fresh");
        let blank = write(demo, &empty, &fresh).unwrap();
        assert_eq!(blank.written.len(), 4);
        assert_eq!(
            fs::read_to_string(fresh.join("products/demo/i18n/note/ru.json")).unwrap(),
            "{}\n"
        );
    }

    #[test]
    fn a_translations_map_flattens_files_and_nested_objects_and_skips_empty_texts() {
        let release = release_of(
            "u",
            &json!({
                "kernel.json": { "en": { "keel.name": "Name", "keel": { "state": "State" } }, "ru": { "keel.name": "Имя", "keel.state": "" } },
                "auth.json": { "en": { "auth.login.title": "Sign in" }, "ru": { "auth.login.title": null } }
            }),
        )
        .unwrap();
        assert_eq!(
            release,
            BTreeMap::from([
                (
                    "en".to_owned(),
                    BTreeMap::from([
                        ("auth.login.title".to_owned(), "Sign in".to_owned()),
                        ("keel.name".to_owned(), "Name".to_owned()),
                        ("keel.state".to_owned(), "State".to_owned()),
                    ])
                ),
                (
                    "ru".to_owned(),
                    BTreeMap::from([("keel.name".to_owned(), "Имя".to_owned())])
                ),
            ])
        );
        for (body, found) in [
            (json!([]), "array"),
            (json!({ "kernel.json": 1 }), "number"),
            (json!({ "kernel.json": { "en": "Name" } }), "string"),
        ] {
            match release_of("u", &body).unwrap_err() {
                Error::Payload {
                    url, found: kind, ..
                } => {
                    assert_eq!(url, "u");
                    assert_eq!(kind, found);
                }
                other => panic!("expected a payload error, got {other}"),
            }
        }
    }

    #[test]
    fn the_pull_is_resolved_from_the_environment_and_masks_its_token() {
        let projects = projects();
        let env = HashMap::from([
            ("KEEL_OWNLATE_TOKEN", "secret-token"),
            ("KEEL_OWNLATE_PROJECT_CORE", "core-id"),
            ("KEEL_OWNLATE_PROJECT_DEMO", "demo-id"),
        ]);
        let lookup =
            |env: &HashMap<&str, &str>, name: &str| env.get(name).map(|value| (*value).to_owned());
        let pull = Pull::resolve(&projects, |name| lookup(&env, name)).unwrap();
        assert_eq!(pull.url, "https://api.ownlate.test");
        assert_eq!(pull.token, "secret-token");
        assert_eq!(
            pull.map_url(&projects.projects[1]),
            "https://api.ownlate.test/public/v1/segments/translations-map?projectId=demo-id"
        );
        let debug = format!("{pull:?}");
        assert!(debug.contains("***"), "{debug}");
        assert!(!debug.contains("secret-token"), "{debug}");

        let mut with_url = env.clone();
        with_url.insert("KEEL_OWNLATE_URL", "http://127.0.0.1:9/");
        let local = Pull::resolve(&projects, |name| lookup(&with_url, name)).unwrap();
        assert_eq!(local.url, "http://127.0.0.1:9");

        let mut without_token = env.clone();
        without_token.remove("KEEL_OWNLATE_TOKEN");
        assert!(matches!(
            Pull::resolve(&projects, |name| lookup(&without_token, name)),
            Err(Error::Env(name)) if name == "KEEL_OWNLATE_TOKEN"
        ));
        let mut without_id = env.clone();
        without_id.insert("KEEL_OWNLATE_PROJECT_DEMO", "");
        assert!(matches!(
            Pull::resolve(&projects, |name| lookup(&without_id, name)),
            Err(Error::Env(name)) if name == "KEEL_OWNLATE_PROJECT_DEMO"
        ));
    }

    #[derive(Default)]
    struct Site {
        requests: Vec<(String, Option<String>)>,
    }

    async fn translations_map(
        State(site): State<Arc<Mutex<Site>>>,
        Query(query): Query<HashMap<String, String>>,
        headers: HeaderMap,
    ) -> Response {
        let bearer = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let project = query.get("projectId").cloned().unwrap_or_default();
        site.lock()
            .await
            .requests
            .push((project.clone(), bearer.clone()));
        if bearer.as_deref() != Some("Bearer secret-token") {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "statusCode": 401, "message": "Unauthorized" })),
            )
                .into_response();
        }
        match project.as_str() {
            "core-id" => Json(json!({
                "kernel.json": {
                    "en": { "keel.brand": "Keel", "keel.name": "Name", "menu.system": "System", "phx.close": "Close" },
                    "ru": { "keel.brand": "Keel", "keel.name": "Имя", "menu.system": "Система", "phx.close": "Закрыть" }
                },
                "auth.json": {
                    "en": { "auth.login": "Login", "auth.login.title": "Sign in" },
                    "ru": { "auth.login": "Логин", "auth.login.title": "Вход" }
                }
            }))
            .into_response(),
            "demo-id" => Json(json!({
                "demo.json": {
                    "en": { "menu.demo": "Demo", "product.demo.brand": "Keel Demo", "note.list.title": "Notes" },
                    "ru": { "menu.demo": "Демо", "product.demo.brand": "Keel Demo", "note.list.title": "Заметки" }
                }
            }))
            .into_response(),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "statusCode": 500, "message": "Error serializing response" }))).into_response(),
        }
    }

    async fn serve() -> (String, Arc<Mutex<Site>>) {
        let site = Arc::new(Mutex::new(Site::default()));
        let router = Router::new()
            .route(MAP_PATH, get(translations_map))
            .with_state(Arc::clone(&site));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (format!("http://{addr}"), site)
    }

    #[tokio::test]
    async fn pull_fetches_the_translations_map_of_every_project_with_the_token_and_writes_the_dictionaries()
     {
        let (url, site) = serve().await;
        let projects = projects();
        let env = HashMap::from([
            ("KEEL_OWNLATE_URL", url.as_str()),
            ("KEEL_OWNLATE_TOKEN", "secret-token"),
            ("KEEL_OWNLATE_PROJECT_CORE", "core-id"),
            ("KEEL_OWNLATE_PROJECT_DEMO", "demo-id"),
        ]);
        let resolved = Pull::resolve(&projects, |name| {
            env.get(name).map(|value| (*value).to_owned())
        })
        .unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let out = temporary.path().join("tree");
        let pulled = pull(&projects, &resolved, &out).await.unwrap();
        assert_eq!(pulled.written.len(), 8);
        assert_eq!(pulled.unchanged, 0);
        assert_eq!(
            read_json(&out.join("crates/kernel/i18n/ru.json")).unwrap(),
            json!({ "keel": { "brand": "Keel", "name": "Имя" }, "menu": { "system": "Система" }, "phx": { "close": "Закрыть" } })
        );
        assert_eq!(
            read_json(&out.join("crates/system/i18n/auth/en.json")).unwrap(),
            json!({ "auth": { "login": "Login", "login.title": "Sign in" } })
        );
        assert_eq!(
            read_json(&out.join("products/demo/i18n/en.json")).unwrap(),
            json!({ "menu": { "demo": "Demo" }, "product": { "demo": { "brand": "Keel Demo" } } })
        );
        assert_eq!(
            read_json(&out.join("products/demo/i18n/note/ru.json")).unwrap(),
            json!({ "note": { "list": { "title": "Заметки" } } })
        );
        assert_eq!(
            site.lock().await.requests,
            [
                ("core-id".to_owned(), Some("Bearer secret-token".to_owned())),
                ("demo-id".to_owned(), Some("Bearer secret-token".to_owned())),
            ]
        );
        let again = pull(&projects, &resolved, &out).await.unwrap();
        assert!(again.written.is_empty(), "{again}");
        assert_eq!(again.unchanged, 8);

        let mut wrong = env.clone();
        wrong.insert("KEEL_OWNLATE_TOKEN", "other");
        let refused = Pull::resolve(&projects, |name| {
            wrong.get(name).map(|value| (*value).to_owned())
        })
        .unwrap();
        match pull(&projects, &refused, &out).await.unwrap_err() {
            Error::Answered {
                url: answered,
                status,
                body,
            } => {
                assert!(answered.ends_with("?projectId=core-id"), "{answered}");
                assert_eq!(status, 401);
                assert!(body.contains("Unauthorized"), "{body}");
            }
            other => panic!("expected a refusal, got {other}"),
        }
        let mut unknown = env.clone();
        unknown.insert("KEEL_OWNLATE_PROJECT_DEMO", "nobody");
        let missing = Pull::resolve(&projects, |name| {
            unknown.get(name).map(|value| (*value).to_owned())
        })
        .unwrap();
        assert!(matches!(
            pull(&projects, &missing, &out).await.unwrap_err(),
            Error::Answered { status: 500, .. }
        ));
        let mut down = env.clone();
        down.insert("KEEL_OWNLATE_URL", "http://127.0.0.1:1");
        let unreachable = Pull::resolve(&projects, |name| {
            down.get(name).map(|value| (*value).to_owned())
        })
        .unwrap();
        assert!(matches!(
            pull(&projects, &unreachable, &out).await.unwrap_err(),
            Error::Http(_)
        ));
    }
}
