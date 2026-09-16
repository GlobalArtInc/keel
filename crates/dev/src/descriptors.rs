use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use keel_types::{Envelope, Level};
use reqwest::header::{CONTENT_TYPE, COOKIE, SET_COOKIE};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use similar::TextDiff;

pub const API_PREFIX: &str = "/api/v1";
pub const SESSION_COOKIE: &str = "__Host-keel_session";
pub const BOOT_FILE: &str = "boot.json";
pub const PRODUCTS_FILE: &str = "products.json";
pub const I18N_DIR: &str = "i18n";
pub const FUNCTIONS_DIR: &str = "functions";
pub const LANGUAGES: [&str; 2] = ["en", "ru"];
pub const LOGIN: &str = "auth.login";
pub const LOGOUT: &str = "auth.logout";
pub const PRODUCT_LIST: &str = "keel.product.list";
pub const AUTHENTICATED: &str = "authenticated";
pub const PRINCIPAL_ID: &str = "01J8A0000000000000000000B1";
pub const ACCOUNT_ID: &str = "01J8A0000000000000000000A1";
pub const META_VERSION: &str = "m-0000000000000000";
pub const ENABLED_AT: &str = "2026-01-01T00:00:00Z";
pub const BINARY_LABEL: &str = "binary";
pub const ABSENT: &str = "/dev/null";
const CONTEXT_LINES: usize = 3;
const TIMEOUT: Duration = Duration::from_secs(30);
const BODY_EXCERPT: usize = 300;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("{}: {source}", path.display())]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("the HTTP client failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("--cacert {}: {source}", path.display())]
    Cacert {
        path: PathBuf,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("{method} {path} answered {status}: {body}")]
    Answered {
        method: Method,
        path: String,
        status: u16,
        body: String,
    },
    #[error("{method} {path} was refused with {code} {key}")]
    Refused {
        method: Method,
        path: String,
        code: String,
        key: String,
    },
    #[error("{method} {path} answered {expected}, not a {found}")]
    Shape {
        method: Method,
        path: String,
        expected: &'static str,
        found: &'static str,
    },
    #[error("the guest is dumped without a login; drop --as guest")]
    GuestLogin,
    #[error("--as names the level {0} twice")]
    LevelTwice(Level),
    #[error("{login} logged in at level {actual}, not {expected}")]
    Level {
        login: String,
        expected: Level,
        actual: Level,
    },
    #[error(
        "{login} answered the state {state} to {LOGIN}; the dump logs in with a password alone"
    )]
    State { login: String, state: String },
    #[error("{LOGIN} for {login} set no session cookie")]
    NoCookie { login: String },
}

#[derive(Debug, thiserror::Error)]
pub enum BadLogin {
    #[error("--as takes <level>=<login>:<password>")]
    Shape,
    #[error("--as names an unknown level {0}")]
    Level(String),
}

#[derive(Clone, PartialEq, Eq)]
pub struct Login {
    pub level: Level,
    pub login: String,
    pub password: String,
}

impl FromStr for Login {
    type Err = BadLogin;

    fn from_str(text: &str) -> Result<Self, BadLogin> {
        let (level, credential) = text.split_once('=').ok_or(BadLogin::Shape)?;
        let (login, password) = credential.split_once(':').ok_or(BadLogin::Shape)?;
        if level.is_empty() || login.is_empty() || password.is_empty() {
            return Err(BadLogin::Shape);
        }
        let level = level
            .parse()
            .map_err(|_| BadLogin::Level(level.to_owned()))?;
        Ok(Self {
            level,
            login: login.to_owned(),
            password: password.to_owned(),
        })
    }
}

impl fmt::Debug for Login {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}={}:***", self.level, self.login)
    }
}

#[derive(Debug)]
pub struct Dumped {
    pub dir: PathBuf,
    pub levels: Vec<Level>,
    pub files: usize,
}

impl fmt::Display for Dumped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let levels: Vec<&str> = self.levels.iter().map(|level| level.as_str()).collect();
        write!(
            f,
            "{} files dumped to {} for {}",
            self.files,
            self.dir.display(),
            levels.join(", ")
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub text: String,
}

#[derive(Debug)]
pub struct Diff {
    pub files: Vec<FileDiff>,
    pub compared: usize,
    pub a: String,
    pub b: String,
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn summary(&self) -> String {
        if self.files.is_empty() {
            format!(
                "{} and {} are identical ({} files)",
                self.a, self.b, self.compared
            )
        } else {
            let paths: Vec<&str> = self.files.iter().map(|file| file.path.as_str()).collect();
            format!(
                "{} of {} files differ between {} and {}: {}",
                self.files.len(),
                self.compared,
                self.a,
                self.b,
                paths.join(", ")
            )
        }
    }
}

impl fmt::Display for Diff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for file in &self.files {
            f.write_str(&file.text)?;
        }
        Ok(())
    }
}

struct Answer {
    method: Method,
    path: String,
    status: StatusCode,
    body: String,
    cookie: Option<String>,
}

impl Answer {
    fn failure(&self) -> Error {
        if let Ok(Envelope::<Value>::Err { error }) = serde_json::from_str(&self.body) {
            return Error::Refused {
                method: self.method.clone(),
                path: self.path.clone(),
                code: error.code.as_str().to_owned(),
                key: error.key,
            };
        }
        Error::Answered {
            method: self.method.clone(),
            path: self.path.clone(),
            status: self.status.as_u16(),
            body: self.body.chars().take(BODY_EXCERPT).collect(),
        }
    }

    fn json(&self) -> Result<Value, Error> {
        if self.status != StatusCode::OK {
            return Err(self.failure());
        }
        serde_json::from_str(&self.body).map_err(|_| self.failure())
    }

    fn data(&self) -> Result<Value, Error> {
        match serde_json::from_str::<Envelope<Value>>(&self.body) {
            Ok(Envelope::Ok { data, .. }) if self.status == StatusCode::OK => Ok(data),
            _ => Err(self.failure()),
        }
    }

    fn array(&self, value: Value) -> Result<Vec<Value>, Error> {
        match value {
            Value::Array(items) => Ok(items),
            other => Err(self.shape("an array", &other)),
        }
    }

    fn shape(&self, expected: &'static str, found: &Value) -> Error {
        Error::Shape {
            method: self.method.clone(),
            path: self.path.clone(),
            expected,
            found: kind_of(found),
        }
    }
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

pub struct Client {
    http: reqwest::Client,
    base: String,
}

impl Client {
    pub fn new(url: &str, cacert: Option<&Path>) -> Result<Self, Error> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut builder = reqwest::Client::builder().no_proxy().timeout(TIMEOUT);
        if let Some(path) = cacert {
            let failed = |source: Box<dyn std::error::Error + Send + Sync>| Error::Cacert {
                path: path.to_owned(),
                source,
            };
            let pem = fs::read(path).map_err(|error| failed(Box::new(error)))?;
            let certificates = reqwest::Certificate::from_pem_bundle(&pem)
                .map_err(|error| failed(Box::new(error)))?;
            if certificates.is_empty() {
                return Err(failed("the file holds no CERTIFICATE block".into()));
            }
            builder = builder.tls_certs_only(certificates);
        }
        let http = builder.build()?;
        Ok(Self {
            http,
            base: url.trim_end_matches('/').to_owned(),
        })
    }

    async fn get(&self, path: &str, session: Option<&str>) -> Result<Answer, Error> {
        let request = self.http.get(format!("{}{path}", self.base));
        self.send(Method::GET, path, request, session).await
    }

    async fn call(
        &self,
        func: &str,
        params: &Value,
        session: Option<&str>,
    ) -> Result<Answer, Error> {
        let path = format!("{API_PREFIX}/call/{func}");
        let request = self
            .http
            .post(format!("{}{path}", self.base))
            .header(CONTENT_TYPE, "application/json")
            .body(params.to_string());
        self.send(Method::POST, &path, request, session).await
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        mut request: reqwest::RequestBuilder,
        session: Option<&str>,
    ) -> Result<Answer, Error> {
        if let Some(session) = session {
            request = request.header(COOKIE, session);
        }
        let response = request.send().await?;
        let status = response.status();
        let cookie = response
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .find_map(session_pair);
        let body = response.text().await?;
        Ok(Answer {
            method,
            path: path.to_owned(),
            status,
            body,
            cookie,
        })
    }

    async fn login(&self, login: &Login) -> Result<String, Error> {
        let answer = self
            .call(
                LOGIN,
                &json!({ "login": login.login, "password": login.password }),
                None,
            )
            .await?;
        let data = answer.data()?;
        let state = data["state"].as_str().unwrap_or_default();
        if state != AUTHENTICATED {
            return Err(Error::State {
                login: login.login.clone(),
                state: state.to_owned(),
            });
        }
        let actual: Level = data["principal"]["level"]
            .as_str()
            .and_then(|level| level.parse().ok())
            .ok_or_else(|| answer.shape("a principal with a level", &data["principal"]))?;
        if actual != login.level {
            return Err(Error::Level {
                login: login.login.clone(),
                expected: login.level,
                actual,
            });
        }
        answer.cookie.ok_or_else(|| Error::NoCookie {
            login: login.login.clone(),
        })
    }

    async fn logout(&self, session: &str) -> Result<(), Error> {
        self.call(LOGOUT, &json!({}), Some(session))
            .await?
            .data()
            .map(|_| ())
    }
}

fn session_pair(set_cookie: &str) -> Option<String> {
    let pair = set_cookie.split(';').next().unwrap_or_default().trim();
    let value = pair.strip_prefix(SESSION_COOKIE)?.strip_prefix('=')?;
    if value.is_empty() {
        return None;
    }
    Some(pair.to_owned())
}

pub async fn dump(
    url: &str,
    cacert: Option<&Path>,
    out: &Path,
    logins: &[Login],
) -> Result<Dumped, Error> {
    let mut seen = BTreeSet::new();
    for login in logins {
        if login.level == Level::Guest {
            return Err(Error::GuestLogin);
        }
        if !seen.insert(login.level) {
            return Err(Error::LevelTwice(login.level));
        }
    }
    let client = Client::new(url, cacert)?;
    let mut levels = vec![Level::Guest];
    let mut files = dump_level(&client, out, Level::Guest, None).await?;
    for login in logins {
        let session = client.login(login).await?;
        files += dump_level(&client, out, login.level, Some(&session)).await?;
        client.logout(&session).await?;
        levels.push(login.level);
    }
    Ok(Dumped {
        dir: out.to_path_buf(),
        levels,
        files,
    })
}

async fn dump_level(
    client: &Client,
    out: &Path,
    level: Level,
    session: Option<&str>,
) -> Result<usize, Error> {
    let answer = client.get(&format!("{API_PREFIX}/boot"), session).await?;
    let mut boot = answer.json()?;
    normalise_boot(&mut boot);
    let answer = client
        .get(&format!("{API_PREFIX}/functions"), session)
        .await?;
    let descriptors = answer.array(answer.json()?)?;
    let mut named = Vec::with_capacity(descriptors.len());
    for descriptor in descriptors {
        let name = descriptor["name"]
            .as_str()
            .filter(|name| file_name_safe(name))
            .ok_or_else(|| answer.shape("a descriptor with a name", &descriptor))?
            .to_owned();
        named.push((name, descriptor));
    }
    let mut catalogues = Vec::with_capacity(LANGUAGES.len());
    for lang in LANGUAGES {
        let answer = client
            .get(&format!("{API_PREFIX}/i18n?lang={lang}"), session)
            .await?;
        catalogues.push((lang, answer.data()?));
    }
    let products = if named.iter().any(|(name, _)| name == PRODUCT_LIST) {
        let answer = client.call(PRODUCT_LIST, &json!({}), session).await?;
        let page = answer.data()?;
        let mut rows = Value::Array(answer.array(page["data"].clone())?);
        normalise_products(&mut rows);
        Some(rows)
    } else {
        None
    };
    let dir = out.join(level.as_str());
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(io(&dir))?;
    }
    let i18n = dir.join(I18N_DIR);
    let functions = dir.join(FUNCTIONS_DIR);
    fs::create_dir_all(&i18n).map_err(io(&i18n))?;
    fs::create_dir_all(&functions).map_err(io(&functions))?;
    let mut written = 0;
    written += write(&dir.join(BOOT_FILE), &boot)?;
    for (name, descriptor) in &named {
        written += write(&functions.join(format!("{name}.json")), descriptor)?;
    }
    for (lang, catalogue) in &catalogues {
        written += write(&i18n.join(format!("{lang}.json")), catalogue)?;
    }
    if let Some(rows) = &products {
        written += write(&dir.join(PRODUCTS_FILE), rows)?;
    }
    Ok(written)
}

fn file_name_safe(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && !name.starts_with('.')
}

pub fn normalise_boot(boot: &mut Value) {
    if let Some(principal) = boot.get_mut("principal").and_then(Value::as_object_mut) {
        principal.insert("id".to_owned(), Value::String(PRINCIPAL_ID.to_owned()));
        principal.insert("account".to_owned(), Value::String(ACCOUNT_ID.to_owned()));
    }
    if let Some(boot) = boot.as_object_mut() {
        boot.insert("meta".to_owned(), Value::String(META_VERSION.to_owned()));
    }
}

pub fn normalise_products(rows: &mut Value) {
    let Some(rows) = rows.as_array_mut() else {
        return;
    };
    for row in rows {
        if let Some(row) = row.as_object_mut()
            && row.get("enabledAt").is_some_and(|at| !at.is_null())
        {
            row.insert("enabledAt".to_owned(), Value::String(ENABLED_AT.to_owned()));
        }
    }
}

fn write(path: &Path, value: &Value) -> Result<usize, Error> {
    let mut text = serde_json::to_string_pretty(value).map_err(|source| Error::Json {
        path: path.to_path_buf(),
        source,
    })?;
    text.push('\n');
    fs::write(path, text).map_err(io(path))?;
    Ok(1)
}

fn io(path: &Path) -> impl FnOnce(io::Error) -> Error {
    let path = path.to_path_buf();
    move |source| Error::Io { path, source }
}

pub fn diff(a: &Path, b: &Path) -> Result<Diff, Error> {
    diff_labelled(a, b, &a.display().to_string(), &b.display().to_string())
}

pub fn diff_labelled(a: &Path, b: &Path, label_a: &str, label_b: &str) -> Result<Diff, Error> {
    let mut paths = files_under(a)?;
    paths.extend(files_under(b)?);
    let mut files = Vec::new();
    for path in &paths {
        let old = read_present(&a.join(path))?;
        let new = read_present(&b.join(path))?;
        if old == new {
            continue;
        }
        files.push(FileDiff {
            path: path.clone(),
            text: unified(path, old.as_deref(), new.as_deref(), label_a, label_b),
        });
    }
    Ok(Diff {
        files,
        compared: paths.len(),
        a: label_a.to_owned(),
        b: label_b.to_owned(),
    })
}

pub(crate) fn unified(
    path: &str,
    old: Option<&str>,
    new: Option<&str>,
    label_a: &str,
    label_b: &str,
) -> String {
    let from = old.map_or(ABSENT.to_owned(), |_| format!("{label_a}/{path}"));
    let to = new.map_or(ABSENT.to_owned(), |_| format!("{label_b}/{path}"));
    let diff = TextDiff::from_lines(old.unwrap_or_default(), new.unwrap_or_default());
    let mut unified = diff.unified_diff();
    unified.context_radius(CONTEXT_LINES).header(&from, &to);
    unified.to_string()
}

fn read_present(path: &Path) -> Result<Option<String>, Error> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(io(path)(source)),
    }
}

pub fn files_under(root: &Path) -> Result<BTreeSet<String>, Error> {
    let mut paths = BTreeSet::new();
    let mut pending = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = pending.pop() {
        for entry in fs::read_dir(&dir).map_err(io(&dir))? {
            let entry = entry.map_err(io(&dir))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let relative = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let kind = entry.file_type().map_err(io(&entry.path()))?;
            if kind.is_dir() {
                pending.push((entry.path(), relative));
            } else {
                paths.insert(relative);
            }
        }
    }
    Ok(paths)
}

pub async fn check(
    url: &str,
    cacert: Option<&Path>,
    dir: &Path,
    logins: &[Login],
) -> Result<Diff, Error> {
    let temporary = tempfile::tempdir().map_err(io(&std::env::temp_dir()))?;
    dump(url, cacert, temporary.path(), logins).await?;
    diff_labelled(
        dir,
        temporary.path(),
        &dir.display().to_string(),
        BINARY_LABEL,
    )
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::Arc;

    use axum::extract::{Path as Segment, Query, State};
    use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
    use axum::response::{IntoResponse, Response};
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use std::collections::HashMap;
    use tokio::sync::Mutex;

    use super::*;

    const ADMIN_COOKIE: &str = "__Host-keel_session=ks_admin";
    const USER_COOKIE: &str = "__Host-keel_session=ks_user";

    #[derive(Default)]
    struct Site {
        logouts: Vec<String>,
    }

    fn principal_of(headers: &HeaderMap) -> Option<(&'static str, Level)> {
        match headers.get(COOKIE).and_then(|value| value.to_str().ok()) {
            Some(ADMIN_COOKIE) => Some(("root", Level::Admin)),
            Some(USER_COOKIE) => Some(("alice", Level::User)),
            _ => None,
        }
    }

    fn envelope(data: Value) -> Json<Value> {
        Json(json!({ "ok": true, "data": data, "meta": { "requestId": "r-1" } }))
    }

    fn refusal(status: StatusCode, code: &str, key: &str) -> Response {
        (
            status,
            Json(json!({ "ok": false, "error": { "code": code, "key": key, "requestId": "r-2" } })),
        )
            .into_response()
    }

    async fn boot(headers: HeaderMap) -> Json<Value> {
        let principal = principal_of(&headers).map(|(login, level)| {
            json!({ "id": "01J9ZZZZZZZZZZZZZZZZZZZZZZ", "login": login, "level": level, "account": "01J9YYYYYYYYYYYYYYYYYYYYYY",
                    "actor": null, "lang": "en", "tz": null, "totp": false })
        });
        Json(json!({
            "keel": { "version": "0.1.0", "api": 1 },
            "products": [], "brand": { "name": "keel.brand", "hue": 220, "chroma": 0.08 },
            "principal": principal, "features": [], "license": null, "menu": [], "dashboard": [],
            "views": [], "meta": format!("m-{:016x}", headers.len()), "problems": null
        }))
    }

    async fn functions(headers: HeaderMap) -> Json<Value> {
        let mut list = vec![json!({ "name": "auth.login", "module": "auth" })];
        if let Some((_, level)) = principal_of(&headers) {
            list.push(json!({ "name": "note.list", "module": "note" }));
            if level == Level::Admin {
                list.push(json!({ "name": PRODUCT_LIST, "module": "keel" }));
            }
        }
        Json(Value::Array(list))
    }

    async fn i18n(Query(query): Query<HashMap<String, String>>) -> Json<Value> {
        let lang = query
            .get("lang")
            .cloned()
            .unwrap_or_else(|| "en".to_owned());
        envelope(json!({ "lang": lang, "messages": { "keel": { "brand": lang.to_uppercase() } } }))
    }

    async fn call(
        State(site): State<Arc<Mutex<Site>>>,
        Segment(func): Segment<String>,
        headers: HeaderMap,
        body: String,
    ) -> Response {
        let params: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        match func.as_str() {
            LOGIN => {
                let (cookie, login, level) = match (params["login"].as_str(), params["password"].as_str()) {
                    (Some("root"), Some("pw-root")) => (ADMIN_COOKIE, "root", Level::Admin),
                    (Some("alice"), Some("pw-alice")) => (USER_COOKIE, "alice", Level::User),
                    (Some("totp"), Some("pw-totp")) => {
                        return envelope(json!({ "state": "totp", "challenge": "c" })).into_response();
                    }
                    _ => return refusal(StatusCode::UNAUTHORIZED, "unauthenticated", "auth.error.credentials"),
                };
                let mut response = envelope(json!({
                    "state": AUTHENTICATED,
                    "principal": { "id": "01J9ZZZZZZZZZZZZZZZZZZZZZZ", "login": login, "level": level, "account": "01J9YYYYYYYYYYYYYYYYYYYYYY", "actor": null }
                }))
                .into_response();
                response.headers_mut().insert(
                    header::SET_COOKIE,
                    HeaderValue::from_str(&format!("{cookie}; Path=/; Secure; HttpOnly; SameSite=Lax")).unwrap(),
                );
                response
            }
            LOGOUT => {
                let Some((login, _)) = principal_of(&headers) else {
                    return refusal(StatusCode::UNAUTHORIZED, "unauthenticated", "keel.error.unauthenticated");
                };
                site.lock().await.logouts.push(login.to_owned());
                envelope(json!({ "revoked": true })).into_response()
            }
            PRODUCT_LIST => match principal_of(&headers) {
                Some((_, Level::Admin)) => envelope(json!({
                    "data": [
                        { "id": "demo", "state": "enabled", "enabledAt": "2026-09-15T10:11:12.123456Z", "enabledBy": "install", "modules": ["note"] },
                        { "id": "dns", "state": "disabled", "enabledAt": null, "enabledBy": null, "modules": ["dns"] }
                    ],
                    "total": 2, "totals": null
                }))
                .into_response(),
                _ => refusal(StatusCode::FORBIDDEN, "forbidden", "keel.error.forbidden"),
            },
            _ => refusal(StatusCode::NOT_FOUND, "not_found", "keel.error.function_not_found"),
        }
    }

    async fn serve() -> (String, Arc<Mutex<Site>>) {
        let site = Arc::new(Mutex::new(Site::default()));
        let router = Router::new()
            .route("/api/v1/boot", get(boot))
            .route("/api/v1/functions", get(functions))
            .route("/api/v1/i18n", get(i18n))
            .route("/api/v1/call/{func}", post(call))
            .with_state(Arc::clone(&site));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (format!("http://{addr}"), site)
    }

    fn read(path: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    fn login(text: &str) -> Login {
        text.parse().unwrap()
    }

    const CACERT: &str = "-----BEGIN CERTIFICATE-----\nMIIBPDCB46ADAgECAgkAjV0nv83bZuYwCgYIKoZIzj0EAwIwFDESMBAGA1UEAwwJ\na2VlbC50ZXN0MB4XDTI2MDkxNjExNTUwMloXDTM2MDkxMzExNTUwMlowFDESMBAG\nA1UEAwwJa2VlbC50ZXN0MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAENDKfeUdk\nMU5iCUU9EvdblbRMXMsnLw1hE2ABqZxWzewdk1OGnLdRt1Nwvi7bOExiI888Z3lv\nYxyUy10txTnmq6MeMBwwGgYDVR0RBBMwEYIJa2VlbC50ZXN0hwR/AAABMAoGCCqG\nSM49BAMCA0gAMEUCIC91i0gyNvK0EwDe9smH6XUvWz1hogNOGblyUHUmnGZeAiEA\nsZspc60D8jeme2bdIqYAKl1mePsxIEvC92uMuTkeHdU=\n-----END CERTIFICATE-----\n";

    #[tokio::test]
    async fn dump_accepts_a_cacert_against_a_plain_server_and_names_a_file_it_cannot_read() {
        let (url, _site) = serve().await;
        let temporary = tempfile::tempdir().unwrap();
        let cacert = temporary.path().join("cert.pem");
        fs::write(&cacert, CACERT).unwrap();
        let out = temporary.path().join("dump");
        let dumped = dump(&url, Some(&cacert), &out, &[login("admin=root:pw-root")])
            .await
            .unwrap();
        assert_eq!(dumped.levels, [Level::Guest, Level::Admin]);
        let clean = check(&url, Some(&cacert), &out, &[login("admin=root:pw-root")])
            .await
            .unwrap();
        assert!(clean.is_empty(), "{clean}");

        let missing = temporary.path().join("nowhere.pem");
        let error = dump(&url, Some(&missing), &out, &[]).await.unwrap_err();
        assert!(matches!(error, Error::Cacert { .. }), "{error}");
        assert!(
            error.to_string().contains(&missing.display().to_string()),
            "{error}"
        );
        let garbage = temporary.path().join("garbage.pem");
        fs::write(&garbage, "not a certificate").unwrap();
        let error = dump(&url, Some(&garbage), &out, &[]).await.unwrap_err();
        assert!(matches!(error, Error::Cacert { .. }), "{error}");
    }

    #[test]
    fn a_login_is_parsed_from_level_login_and_password_and_its_debug_masks_the_password() {
        let parsed = login("admin=root:s3cret");
        assert_eq!(parsed.level, Level::Admin);
        assert_eq!(parsed.login, "root");
        assert_eq!(parsed.password, "s3cret");
        assert_eq!(format!("{parsed:?}"), "admin=root:***");
        let with_colon = login("user=alice:pa:ss");
        assert_eq!(with_colon.password, "pa:ss");
        for bad in [
            "admin",
            "admin=root",
            "admin=:pw",
            "admin=root:",
            "=root:pw",
        ] {
            assert!(
                matches!(bad.parse::<Login>(), Err(BadLogin::Shape)),
                "{bad}"
            );
        }
        match "boss=root:pw".parse::<Login>() {
            Err(BadLogin::Level(level)) => assert_eq!(level, "boss"),
            other => panic!("expected an unknown level, got {other:?}"),
        }
        assert_eq!(
            BadLogin::Shape.to_string(),
            "--as takes <level>=<login>:<password>"
        );
    }

    #[test]
    fn the_run_bound_values_of_boot_and_the_product_rows_are_replaced_by_placeholders() {
        let mut boot = json!({
            "keel": { "version": "0.1.0", "api": 1 },
            "principal": { "id": "01J9ZZZZZZZZZZZZZZZZZZZZZZ", "login": "root", "level": "admin", "account": "00000000000000000000000000", "actor": null },
            "meta": "m-7f3a2c1b9d4e0a62"
        });
        normalise_boot(&mut boot);
        assert_eq!(boot["principal"]["id"], json!(PRINCIPAL_ID));
        assert_eq!(boot["principal"]["account"], json!(ACCOUNT_ID));
        assert_eq!(boot["principal"]["login"], json!("root"));
        assert_eq!(boot["meta"], json!(META_VERSION));
        let keys: Vec<&String> = boot["principal"].as_object().unwrap().keys().collect();
        assert_eq!(keys, ["id", "login", "level", "account", "actor"]);
        let mut guest = json!({ "principal": null, "meta": "m-1" });
        normalise_boot(&mut guest);
        assert_eq!(guest, json!({ "principal": null, "meta": META_VERSION }));
        let mut rows = json!([
            { "id": "demo", "enabledAt": "2026-09-15T10:11:12.123456Z", "enabledBy": "install" },
            { "id": "dns", "enabledAt": null, "enabledBy": null }
        ]);
        normalise_products(&mut rows);
        assert_eq!(rows[0]["enabledAt"], json!(ENABLED_AT));
        assert_eq!(rows[1]["enabledAt"], Value::Null);
        assert!(PRINCIPAL_ID.parse::<keel_types::Id>().is_ok());
        assert!(ACCOUNT_ID.parse::<keel_types::Id>().is_ok());
    }

    #[test]
    fn descriptors_diff_over_two_directories_differing_in_one_file_prints_that_files_diff_and_identical_directories_exit_zero()
     {
        let temporary = tempfile::tempdir().unwrap();
        let a = temporary.path().join("a");
        let b = temporary.path().join("b");
        for root in [&a, &b] {
            fs::create_dir_all(root.join("admin/functions")).unwrap();
            fs::write(root.join("admin/boot.json"), "{\n  \"meta\": \"m-0\"\n}\n").unwrap();
            fs::write(
                root.join("admin/functions/note.list.json"),
                "{\n  \"name\": \"note.list\",\n  \"kind\": \"list\"\n}\n",
            )
            .unwrap();
        }
        let identical = diff(&a, &b).unwrap();
        assert!(identical.is_empty());
        assert_eq!(identical.compared, 2);
        assert_eq!(identical.to_string(), "");
        assert_eq!(
            identical.summary(),
            format!(
                "{} and {} are identical (2 files)",
                a.display(),
                b.display()
            )
        );

        fs::write(
            b.join("admin/functions/note.list.json"),
            "{\n  \"name\": \"note.list\",\n  \"kind\": \"form\"\n}\n",
        )
        .unwrap();
        let changed = diff(&a, &b).unwrap();
        assert!(!changed.is_empty());
        assert_eq!(changed.files.len(), 1);
        assert_eq!(changed.files[0].path, "admin/functions/note.list.json");
        let text = changed.to_string();
        assert!(
            text.starts_with(&format!(
                "--- {}/admin/functions/note.list.json\n+++ {}/admin/functions/note.list.json\n@@",
                a.display(),
                b.display()
            )),
            "{text}"
        );
        assert!(
            text.contains("-  \"kind\": \"list\"\n+  \"kind\": \"form\"\n"),
            "{text}"
        );
        assert!(!text.contains("boot.json"), "{text}");
        assert!(changed.summary().starts_with("1 of 2 files differ between"));
        assert!(
            changed
                .summary()
                .ends_with(": admin/functions/note.list.json")
        );

        fs::write(a.join("admin/products.json"), "[]\n").unwrap();
        fs::create_dir_all(b.join("user")).unwrap();
        fs::write(b.join("user/boot.json"), "{}\n").unwrap();
        let one_sided = diff(&a, &b).unwrap();
        let paths: Vec<&str> = one_sided
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect();
        assert_eq!(
            paths,
            [
                "admin/functions/note.list.json",
                "admin/products.json",
                "user/boot.json"
            ]
        );
        assert!(
            one_sided.files[1].text.starts_with(&format!(
                "--- {}/admin/products.json\n+++ {ABSENT}\n",
                a.display()
            )),
            "{}",
            one_sided.files[1].text
        );
        assert!(one_sided.files[1].text.contains("\n-[]\n"));
        assert!(
            one_sided.files[2].text.starts_with(&format!(
                "--- {ABSENT}\n+++ {}/user/boot.json\n",
                b.display()
            )),
            "{}",
            one_sided.files[2].text
        );
        assert!(one_sided.files[2].text.contains("\n+{}\n"));
        assert_eq!(one_sided.compared, 4);

        let missing = diff(&a, &temporary.path().join("nowhere")).unwrap_err();
        assert!(matches!(missing, Error::Io { .. }), "{missing}");
    }

    #[tokio::test]
    async fn dump_logs_in_per_level_writes_the_projected_files_and_check_diffs_against_a_directory()
    {
        let (url, site) = serve().await;
        let temporary = tempfile::tempdir().unwrap();
        let out = temporary.path().join("dump");
        let logins = [login("admin=root:pw-root"), login("user=alice:pw-alice")];

        let dumped = dump(&url, None, &out, &logins).await.unwrap();
        assert_eq!(dumped.levels, [Level::Guest, Level::Admin, Level::User]);
        assert_eq!(dumped.files, 4 + 7 + 5);
        assert_eq!(
            dumped.to_string(),
            format!(
                "16 files dumped to {} for guest, admin, user",
                out.display()
            )
        );
        let files: Vec<String> = files_under(&out).unwrap().into_iter().collect();
        assert_eq!(
            files,
            [
                "admin/boot.json",
                "admin/functions/auth.login.json",
                "admin/functions/keel.product.list.json",
                "admin/functions/note.list.json",
                "admin/i18n/en.json",
                "admin/i18n/ru.json",
                "admin/products.json",
                "guest/boot.json",
                "guest/functions/auth.login.json",
                "guest/i18n/en.json",
                "guest/i18n/ru.json",
                "user/boot.json",
                "user/functions/auth.login.json",
                "user/functions/note.list.json",
                "user/i18n/en.json",
                "user/i18n/ru.json",
            ]
        );
        let admin_boot = read(&out.join("admin/boot.json"));
        assert_eq!(admin_boot["principal"]["login"], json!("root"));
        assert_eq!(admin_boot["principal"]["id"], json!(PRINCIPAL_ID));
        assert_eq!(admin_boot["principal"]["account"], json!(ACCOUNT_ID));
        assert_eq!(admin_boot["meta"], json!(META_VERSION));
        let guest_boot = read(&out.join("guest/boot.json"));
        assert_eq!(guest_boot["principal"], Value::Null);
        assert_eq!(guest_boot["meta"], json!(META_VERSION));
        assert_eq!(
            read(&out.join("user/boot.json"))["principal"]["level"],
            json!("user")
        );
        let products = read(&out.join("admin/products.json"));
        assert_eq!(products[0]["id"], json!("demo"));
        assert_eq!(products[0]["enabledAt"], json!(ENABLED_AT));
        assert_eq!(products[0]["modules"], json!(["note"]));
        assert_eq!(products[1]["enabledAt"], Value::Null);
        assert_eq!(
            read(&out.join("admin/i18n/ru.json")),
            json!({ "lang": "ru", "messages": { "keel": { "brand": "RU" } } })
        );
        assert_eq!(
            read(&out.join("user/functions/note.list.json")),
            json!({ "name": "note.list", "module": "note" })
        );
        let text = fs::read_to_string(out.join("guest/boot.json")).unwrap();
        assert!(text.starts_with("{\n  \"keel\": {"), "{text}");
        assert!(text.ends_with("}\n"), "{text}");
        assert_eq!(site.lock().await.logouts, ["root", "alice"]);

        let stale = out.join("admin/functions/gone.json");
        fs::write(&stale, "{}\n").unwrap();
        dump(&url, None, &out, &logins[..1]).await.unwrap();
        assert!(!stale.exists());
        assert!(out.join("user/boot.json").exists());

        let clean = check(&url, None, &out, &logins).await.unwrap();
        assert!(clean.is_empty(), "{clean}");
        assert_eq!(clean.compared, 16);
        fs::write(
            out.join("admin/functions/note.list.json"),
            "{\n  \"name\": \"note.list\",\n  \"module\": \"tags\"\n}\n",
        )
        .unwrap();
        let drifted = check(&url, None, &out, &logins).await.unwrap();
        assert_eq!(drifted.files.len(), 1);
        assert_eq!(drifted.files[0].path, "admin/functions/note.list.json");
        assert!(
            drifted.files[0].text.starts_with(&format!(
                "--- {}/admin/functions/note.list.json\n+++ {BINARY_LABEL}/admin/functions/note.list.json\n",
                out.display()
            )),
            "{}",
            drifted.files[0].text
        );
        assert!(
            drifted.files[0]
                .text
                .contains("-  \"module\": \"tags\"\n+  \"module\": \"note\"\n")
        );
        assert!(
            drifted
                .summary()
                .contains(&format!("between {} and {BINARY_LABEL}", out.display()))
        );
    }

    #[tokio::test]
    async fn dump_refuses_a_guest_login_a_level_twice_a_wrong_password_a_second_factor_and_a_mislabelled_level()
     {
        let (url, _site) = serve().await;
        let temporary = tempfile::tempdir().unwrap();
        let out = temporary.path().join("dump");
        let refused = dump(&url, None, &out, &[login("guest=root:pw-root")])
            .await
            .unwrap_err();
        assert!(matches!(refused, Error::GuestLogin), "{refused}");
        let twice = dump(
            &url,
            None,
            &out,
            &[login("admin=root:pw-root"), login("admin=root:pw-root")],
        )
        .await
        .unwrap_err();
        assert!(matches!(twice, Error::LevelTwice(Level::Admin)), "{twice}");
        assert!(!out.exists());

        let wrong = dump(&url, None, &out, &[login("admin=root:nope")])
            .await
            .unwrap_err();
        match wrong {
            Error::Refused {
                code, key, path, ..
            } => {
                assert_eq!(code, "unauthenticated");
                assert_eq!(key, "auth.error.credentials");
                assert_eq!(path, "/api/v1/call/auth.login");
            }
            other => panic!("expected the refusal, got {other}"),
        }
        assert!(out.join("guest/boot.json").exists());

        let totp = dump(&url, None, &out, &[login("admin=totp:pw-totp")])
            .await
            .unwrap_err();
        match totp {
            Error::State { login, state } => {
                assert_eq!(login, "totp");
                assert_eq!(state, "totp");
            }
            other => panic!("expected the state, got {other}"),
        }

        let mislabelled = dump(&url, None, &out, &[login("reseller=alice:pw-alice")])
            .await
            .unwrap_err();
        match mislabelled {
            Error::Level {
                login,
                expected,
                actual,
            } => {
                assert_eq!(login, "alice");
                assert_eq!(expected, Level::Reseller);
                assert_eq!(actual, Level::User);
            }
            other => panic!("expected the level mismatch, got {other}"),
        }
        assert!(!out.join("reseller").exists());

        let unreachable = dump("http://127.0.0.1:1", None, &out, &[])
            .await
            .unwrap_err();
        assert!(matches!(unreachable, Error::Http(_)), "{unreachable}");
    }
}
