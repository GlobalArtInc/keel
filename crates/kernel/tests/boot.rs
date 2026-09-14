use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, ETAG, IF_NONE_MATCH};
use axum::http::{Request, StatusCode};
use keel_kernel::dispatcher::{CallOptions, Dispatcher, Reply};
use keel_kernel::loader::{BootError, FreezeProblem};
use keel_kernel::socket::{Client, OWNER_LOGIN, Socket, owner};
use keel_kernel::{Composition, Http, HttpOptions, Kernel, Readiness, Storage};
use keel_sdk::Registrar;
use keel_sdk::prelude::*;
use keel_sdk::runtime::Db;
use keel_sdk::system::{Authenticated, Authenticator, Client as HttpClient, Credential};
use keel_testkit::TestDatabase;
use keel_types::{Boot, Envelope, ErrorCode, Scope, Via};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use tempfile::TempDir;
use tower::ServiceExt;

const CUSTOMER: &str = "01J8A0000000000000000000A2";
const BOOT_NONE_FIXTURE: &str = "../../contracts/fixtures/boot-none.json";
const UPDATE_SNAPSHOTS: &str = "KEEL_UPDATE_SNAPSHOTS";

const EN: &str = r#"{
    "probe": { "item": { "list": { "title": "Items" }, "empty": "No items", "public": { "title": "Public" } } },
    "menu": { "probe": "Probe" }
}"#;
const RU: &str = r#"{ "probe": { "item": { "list": { "title": "ru:Items" } } }, "menu": { "probe": "ru:Probe" } }"#;
const BUNDLE_EN: &str =
    r#"{ "product": { "probe": { "brand": "Probe", "level": { "user": "Customer" } } } }"#;
const BUNDLE_RU: &str = r#"{ "product": { "probe": { "brand": "ru:Probe" } } }"#;
const KERNEL_RU: &str = include_str!("../i18n/ru.json");

#[derive(Deserialize, Serialize, JsonSchema)]
struct Item {
    id: Id,
    title: String,
}

struct TestAuthenticator;

#[async_trait]
impl Authenticator for TestAuthenticator {
    async fn authenticate(
        &self,
        credential: &Credential,
        _: &HttpClient,
        db: &Db,
    ) -> Result<Option<Authenticated>> {
        let token = match credential {
            Credential::Bearer(secret) | Credential::Session(secret) => secret.expose(),
        };
        let principal = match token.split_once(':') {
            Some(("login", login)) => stored(db, login).await?,
            Some((level, account)) => {
                let level: Level = level
                    .parse()
                    .map_err(|_| KeelError::internal("probe.error.level"))?;
                let account: Id = account
                    .parse()
                    .map_err(|_| KeelError::internal("probe.error.account"))?;
                Some(principal(level, account, "en"))
            }
            None => None,
        };
        Ok(principal.map(|principal| Authenticated {
            principal,
            session: None,
        }))
    }
}

async fn stored(db: &Db, login: &str) -> Result<Option<Principal>> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, account_id, level, lang FROM keel_principal WHERE login = $1",
            [login.into()],
        ))
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let id: String = row.try_get("", "id")?;
    let account: String = row.try_get("", "account_id")?;
    let level: String = row.try_get("", "level")?;
    let lang: String = row.try_get("", "lang")?;
    let mut principal = principal(
        level
            .parse()
            .map_err(|_| KeelError::internal("probe.error.level"))?,
        account
            .parse()
            .map_err(|_| KeelError::internal("probe.error.account"))?,
        &lang,
    );
    principal.id = id
        .parse()
        .map_err(|_| KeelError::internal("probe.error.id"))?;
    principal.login = login.to_owned();
    Ok(Some(principal))
}

fn principal(level: Level, account: Id, lang: &str) -> Principal {
    let rank = Level::ALL
        .iter()
        .position(|candidate| *candidate == level)
        .unwrap_or_default() as u64;
    let id = ulid::Ulid::from_parts(rank + 1, ulid::Ulid::from(account).random());
    Principal {
        id: Id::from(id),
        login: format!("{level}-{account}"),
        level,
        account_id: account,
        role: "owner".into(),
        actor: None,
        scope: Scope::Full,
        via: Via::Http,
        external_id: None,
        origin: None,
        lang: lang.to_owned(),
        grants: BTreeMap::new(),
    }
}

static PROBE: ModuleDef = ModuleDef {
    id: "probe",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init,
};

static GATE: ModuleDef = ModuleDef {
    id: "gate",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: gate_init,
};

fn gate_init(r: &mut Registrar) -> Result<()> {
    r.contribute::<Arc<dyn Authenticator>>(Arc::new(TestAuthenticator));
    Ok(())
}

fn init(r: &mut Registrar) -> Result<()> {
    r.function(
        function::<ListQuery, Page<Item>>("probe.item.list")
            .kind(Kind::List)
            .access(Level::User, "probe.item.read")
            .unowned()
            .ui(List::new("probe.item.list.title")
                .column(Column::new("title", ColumnType::Text))
                .empty("probe.item.empty", None))
            .handler(|_: Call<ListQuery>| async { Ok(Page::new(Vec::new(), 0)) }),
    )
    .function(
        function::<ListQuery, Page<Item>>("probe.item.public")
            .kind(Kind::List)
            .level(Level::Guest)
            .unowned()
            .ui(List::new("probe.item.public.title").column(Column::new("title", ColumnType::Text)))
            .handler(|_: Call<ListQuery>| async { Ok(Page::new(Vec::new(), 0)) }),
    )
    .permission("probe.item.read", &[Level::User])
    .menu(
        MenuNode::new("probe/items", "probe.item.list")
            .icon("notebook")
            .order(20)
            .badge("probe.item_stale"),
    )
    .menu(MenuNode::new("probe/public", "probe.item.public").order(10))
    .i18n("en", EN)
    .i18n("ru", RU)
    .contribute::<Arc<dyn Authenticator>>(Arc::new(TestAuthenticator));
    Ok(())
}

static PROBE_BUNDLE: ProductManifest = ProductManifest {
    id: "probe",
    name: "product.probe.brand",
    version: "0.1.0",
    api: keel_types::DESCRIPTOR_VERSION,
    brand: Brand {
        logo: "product.probe.logo",
        hue: 30,
        chroma: 0.12,
    },
    modules: &[&PROBE],
    menu: &[MenuGroup {
        id: "probe",
        label: "menu.probe",
        icon: "flask",
        order: 10,
    }],
    levels: LevelLabels {
        labels: &[(Level::User, "product.probe.level.user")],
        disabled: &[],
    },
    account_kinds: &["user"],
    editions: &[
        Edition {
            id: "lite",
            features: &["probe.items"],
        },
        Edition {
            id: "pro",
            features: &["probe.items", "probe.export"],
        },
    ],
    i18n: &[("en", BUNDLE_EN), ("ru", BUNDLE_RU)],
};

struct Bench {
    database: TestDatabase,
    storage: Storage,
    kernel: Kernel,
    dispatcher: Arc<Dispatcher>,
    router: Router,
    dir: TempDir,
}

async fn start(composition: Composition) -> Bench {
    let database = TestDatabase::create().await;
    let storage = database.storage().await;
    let kernel = Kernel::boot(composition, &storage)
        .await
        .expect("the kernel boots");
    storage
        .db()
        .execute_unprepared(&format!(
            "INSERT INTO keel_account (id, kind, parent_id, path, created_at, updated_at) VALUES ('{CUSTOMER}', 'user', '{}', '/{CUSTOMER}/', now(), now())",
            Id::ROOT_ACCOUNT
        ))
        .await
        .expect("seed an account");
    let dispatcher = Arc::new(
        Dispatcher::new(kernel.registry().clone(), storage.db().clone())
            .expect("every schema compiles"),
    );
    let readiness = Readiness::new();
    readiness.migrated();
    let http = Http::new(storage.db().clone(), readiness, HttpOptions::default());
    http.attach(dispatcher.clone())
        .expect("the kernel attaches once");
    let router = http.router();
    Bench {
        database,
        storage,
        kernel,
        dispatcher,
        router,
        dir: tempfile::tempdir().expect("a temporary directory"),
    }
}

struct Answer {
    status: StatusCode,
    etag: Option<String>,
    cache: Option<String>,
    body: Value,
}

impl Bench {
    async fn get(&self, path: &str, auth: Option<&str>, if_none_match: Option<&str>) -> Answer {
        let mut request = Request::get(path);
        if let Some(token) = auth {
            request = request.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        if let Some(etag) = if_none_match {
            request = request.header(IF_NONE_MATCH, etag);
        }
        let response = self
            .router
            .clone()
            .oneshot(request.body(Body::empty()).expect("a request"))
            .await
            .expect("the router answers");
        let status = response.status();
        let header = |name: axum::http::HeaderName| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        };
        let etag = header(ETAG);
        let cache = header(CACHE_CONTROL);
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body is read");
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("the body is JSON")
        };
        Answer {
            status,
            etag,
            cache,
            body,
        }
    }

    async fn boot(&self, auth: Option<&str>) -> Boot {
        let answer = self.get("/api/v1/boot", auth, None).await;
        assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
        serde_json::from_value(answer.body).expect("a boot payload")
    }

    async fn over_socket(&self) -> Boot {
        let serving = Socket::new(self.dispatcher.clone())
            .start(&self.dir.path().join("core.sock"))
            .await
            .expect("the socket starts");
        let mut client = Client::connect(serving.path())
            .await
            .expect("the client connects");
        let envelope = client.boot().await.expect("the socket answers");
        serving.stop().await.expect("the socket stops");
        match envelope {
            Envelope::Ok { data, meta } => {
                assert_eq!(meta.principal.expect("the owner").login, OWNER_LOGIN);
                data
            }
            Envelope::Err { error } => panic!("boot over the socket failed: {error:?}"),
        }
    }

    async fn as_owner(&self, func: &str, params: Value) -> Value {
        let reply = self.call(owner(), func, params).await;
        reply
            .data()
            .cloned()
            .unwrap_or_else(|| panic!("{func} as owner: {:?}", reply.error()))
    }

    async fn call(&self, principal: Principal, func: &str, params: Value) -> Reply {
        self.dispatcher
            .call(principal, func, params, CallOptions::default())
            .await
    }

    async fn enabled_row(&self) -> Value {
        let row = self
            .storage
            .db()
            .query_one(Statement::from_string(
                DbBackend::Postgres,
                "SELECT value FROM keel_kv WHERE key = 'products'",
            ))
            .await
            .expect("the kv row is read");
        row.map_or(Value::Null, |row| {
            row.try_get_by_index::<Value>(0).expect("a JSON value")
        })
    }

    async fn finish(self) {
        drop(self.router);
        self.kernel.stop().await.expect("the kernel stops");
        self.storage.close().await.expect("the pool closes");
        self.database.finish().await;
    }
}

fn paths(boot: &Boot) -> Vec<(Option<&str>, Vec<&str>)> {
    boot.menu
        .iter()
        .map(|group| {
            (
                group.product.as_deref(),
                group.items.iter().map(|item| item.path.as_str()).collect(),
            )
        })
        .collect()
}

#[tokio::test]
async fn boot_over_the_socket_as_the_owner_carries_the_enabled_product_its_brand_and_both_groups() {
    let bench = start(Composition::new().version("7.7.7").product(&PROBE_BUNDLE)).await;
    let boot = bench.over_socket().await;
    assert_eq!(boot.keel.version, "7.7.7");
    assert_eq!(boot.keel.api, keel_types::DESCRIPTOR_VERSION);
    assert_eq!(boot.products.len(), 1);
    assert_eq!(boot.products[0].id, "probe");
    assert_eq!(boot.products[0].name, "product.probe.brand");
    assert_eq!(boot.products[0].edition.as_deref(), Some("lite"));
    assert_eq!(boot.products[0].brand.name, "product.probe.brand");
    assert_eq!(
        boot.products[0].brand.logo.as_deref(),
        Some("/brand/probe/logo.svg")
    );
    assert_eq!(boot.products[0].brand.hue, 30);
    assert_eq!(boot.brand.name, "product.probe.brand");
    assert_eq!(boot.brand.logo.as_deref(), Some("/brand/logo.svg"));
    assert_eq!(boot.brand.hue, 30);
    assert_eq!(boot.brand.chroma, 0.12);
    let principal = boot.principal.as_ref().expect("the owner");
    assert_eq!(principal.login, OWNER_LOGIN);
    assert_eq!(principal.level, Level::Owner);
    assert_eq!(principal.account, Id::ROOT_ACCOUNT);
    assert_eq!(principal.lang, "en");
    assert!(principal.actor.is_none());
    assert!(principal.tz.is_none());
    assert!(!principal.totp);
    assert_eq!(
        paths(&boot),
        [
            (Some("probe"), vec!["probe/public", "probe/items"]),
            (
                None,
                vec!["system/accounts", "system/principals", "system/products"]
            ),
        ]
    );
    let probe = &boot.menu[0];
    assert_eq!(probe.id, "probe");
    assert_eq!(probe.label, "menu.probe");
    assert_eq!(probe.icon, "flask");
    assert_eq!(probe.items[0].label, "probe.item.public.title");
    assert_eq!(probe.items[0].kind, Some(Kind::List));
    assert!(probe.items[0].icon.is_none());
    assert_eq!(probe.items[1].func, "probe.item.list");
    assert_eq!(probe.items[1].icon.as_deref(), Some("notebook"));
    assert_eq!(
        probe.items[1]
            .badge
            .as_ref()
            .map(|badge| badge.problem.as_str()),
        Some("probe.item_stale")
    );
    let system = &boot.menu[1];
    assert_eq!(system.id, "system");
    assert_eq!(system.label, "menu.system");
    assert_eq!(system.items[0].label, "keel.account.list.title");
    assert_eq!(system.items[0].func, "keel.account.list");
    assert!(boot.meta.starts_with("m-"), "{}", boot.meta);
    assert_eq!(boot.features, ["probe.items"]);
    assert!(boot.views.is_empty());
    assert!(boot.dashboard.is_empty());
    assert!(boot.license.is_none());
    assert!(boot.problems.is_none());
    let over_http = bench
        .boot(Some(&format!("owner:{}", Id::ROOT_ACCOUNT)))
        .await;
    assert_eq!(paths(&over_http), paths(&boot));
    assert_eq!(over_http.brand, boot.brand);
    bench.finish().await;
}

#[tokio::test]
async fn boot_without_a_product_shows_the_platform_brand_and_the_platform_group_only() {
    let bench = start(Composition::new()).await;
    let boot = bench.over_socket().await;
    assert!(boot.products.is_empty());
    assert_eq!(boot.brand.name, "keel.brand");
    assert!(boot.brand.logo.is_none());
    assert_eq!(boot.brand.hue, 220);
    assert_eq!(boot.brand.chroma, 0.08);
    assert_eq!(
        paths(&boot),
        [(
            None,
            vec!["system/accounts", "system/principals", "system/products"]
        )]
    );
    assert_eq!(boot.keel.version, env!("CARGO_PKG_VERSION"));
    assert!(boot.views.is_empty());
    let mut rendered = serde_json::to_string_pretty(&boot).expect("the boot payload serialises");
    rendered.push('\n');
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(BOOT_NONE_FIXTURE);
    if std::env::var_os(UPDATE_SNAPSHOTS).is_some() {
        fs::write(&path, &rendered).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    } else {
        let committed = fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "{}: {e}; run the test with {UPDATE_SNAPSHOTS}=1 to write the fixture",
                path.display()
            )
        });
        assert_eq!(
            committed,
            rendered,
            "{} differs from the served payload; review the change and run the test with {UPDATE_SNAPSHOTS}=1",
            path.display()
        );
    }
    bench.finish().await;
}

#[tokio::test]
async fn boot_as_a_guest_returns_no_principal_and_no_node_above_the_guest_level() {
    let bench = start(Composition::new().product(&PROBE_BUNDLE)).await;
    let guest = bench.boot(None).await;
    assert!(guest.principal.is_none());
    assert_eq!(paths(&guest), [(Some("probe"), vec!["probe/public"])]);
    let user = bench.boot(Some(&format!("user:{CUSTOMER}"))).await;
    let principal = user.principal.as_ref().expect("a user");
    assert_eq!(principal.level, Level::User);
    assert_eq!(principal.account.to_string(), CUSTOMER);
    assert_eq!(
        paths(&user),
        [
            (Some("probe"), vec!["probe/public", "probe/items"]),
            (None, vec!["system/accounts", "system/principals"]),
        ]
    );
    assert_ne!(guest.meta, user.meta);
    bench.finish().await;
    let bare = start(Composition::new()).await;
    let nothing = bare.boot(None).await;
    assert!(nothing.principal.is_none());
    assert!(nothing.menu.is_empty(), "{:?}", nothing.menu);
    bare.finish().await;
}

#[tokio::test]
async fn boot_is_cached_by_its_meta_and_meta_changes_with_the_enabled_set_and_the_acl_gen() {
    let with_probe = start(Composition::new().product(&PROBE_BUNDLE)).await;
    let bare = start(Composition::new().module(&GATE)).await;
    let created = with_probe
        .as_owner(
            "keel.principal.create",
            json!({ "login": "alice", "level": "user", "account": CUSTOMER }),
        )
        .await;
    bare.as_owner(
        "keel.principal.create",
        json!({ "login": "alice", "level": "user", "account": CUSTOMER }),
    )
    .await;
    assert_eq!(
        with_probe
            .database
            .count(
                &with_probe.storage,
                "SELECT count(*) FROM keel_principal WHERE login = 'alice'"
            )
            .await,
        1
    );
    let alice = "login:alice";
    let first = with_probe.get("/api/v1/boot", Some(alice), None).await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.body);
    assert_eq!(first.cache.as_deref(), Some("private, no-store"));
    let meta = first.body["meta"].as_str().expect("meta").to_owned();
    assert_eq!(first.etag.as_deref(), Some(format!("\"{meta}\"").as_str()));
    let cached = with_probe
        .get("/api/v1/boot", Some(alice), first.etag.as_deref())
        .await;
    assert_eq!(cached.status, StatusCode::NOT_MODIFIED);
    assert_eq!(cached.body, Value::Null);
    let elsewhere = bare.get("/api/v1/boot", Some(alice), None).await;
    assert_eq!(elsewhere.status, StatusCode::OK, "{}", elsewhere.body);
    assert_ne!(elsewhere.body["meta"], first.body["meta"]);
    with_probe
        .as_owner(
            "keel.principal.update",
            json!({ "id": created["id"], "role": "viewer" }),
        )
        .await;
    let bumped = with_probe
        .get("/api/v1/boot", Some(alice), first.etag.as_deref())
        .await;
    assert_eq!(bumped.status, StatusCode::OK);
    assert_ne!(bumped.body["meta"], first.body["meta"]);
    assert_eq!(bumped.body["principal"]["login"], json!("alice"));
    bare.finish().await;
    with_probe.finish().await;
}

#[tokio::test]
async fn the_catalogue_merges_the_bundle_dictionaries_and_ru_falls_back_to_en_per_key() {
    let bench = start(Composition::new().product(&PROBE_BUNDLE)).await;
    let en = bench.get("/api/v1/i18n?lang=en", None, None).await;
    assert_eq!(en.status, StatusCode::OK, "{}", en.body);
    let messages = &en.body["data"]["messages"];
    assert_eq!(messages["product"]["probe"]["brand"], json!("Probe"));
    assert_eq!(
        messages["product"]["probe"]["level"]["user"],
        json!("Customer")
    );
    assert_eq!(messages["menu"]["probe"], json!("Probe"));
    assert_eq!(messages["menu"]["system"], json!("System"));
    assert_eq!(messages["keel"]["brand"], json!("Keel"));
    let ru = bench.get("/api/v1/i18n?lang=ru", None, None).await;
    assert_eq!(ru.body["data"]["lang"], json!("ru"));
    let messages = &ru.body["data"]["messages"];
    assert_eq!(messages["product"]["probe"]["brand"], json!("ru:Probe"));
    assert_eq!(
        messages["product"]["probe"]["level"]["user"],
        json!("Customer")
    );
    assert_eq!(
        messages["probe"]["item"]["list"]["title"],
        json!("ru:Items")
    );
    assert_eq!(messages["probe"]["item"]["empty"], json!("No items"));
    let kernel_ru: Value = serde_json::from_str(KERNEL_RU).expect("the kernel ru dictionary");
    assert_eq!(messages["menu"]["system"], kernel_ru["menu"]["system"]);
    assert_ne!(messages["menu"]["system"], json!("System"));
    assert_eq!(messages["phx"]["close"], kernel_ru["phx"]["close"]);
    assert_ne!(messages["phx"]["close"], json!("Close"));
    assert_eq!(
        messages["keel"]["shell"]["login"]["title"],
        kernel_ru["keel"]["shell"]["login"]["title"]
    );
    assert_ne!(
        messages["keel"]["shell"]["login"]["title"],
        json!("Sign in")
    );
    let english = &en.body["data"]["messages"];
    assert_eq!(english["phx"]["close"], json!("Close"));
    assert_eq!(english["phx"]["nothingFound"], json!("Nothing found"));
    assert_eq!(english["keel"]["shell"]["login"]["title"], json!("Sign in"));
    assert_eq!(
        english["phx"].as_object().map(|labels| labels.len()),
        messages["phx"].as_object().map(|labels| labels.len())
    );
    assert_ne!(ru.etag, en.etag);
    let again = bench
        .get("/api/v1/i18n?lang=ru", None, ru.etag.as_deref())
        .await;
    assert_eq!(again.status, StatusCode::NOT_MODIFIED);
    bench.finish().await;
    let bare = start(Composition::new()).await;
    let en = bare.get("/api/v1/i18n?lang=en", None, None).await;
    let messages = &en.body["data"]["messages"];
    assert!(messages.get("product").is_none(), "{messages}");
    assert!(messages.get("probe").is_none(), "{messages}");
    assert!(messages["menu"].get("probe").is_none(), "{messages}");
    bare.finish().await;
}

#[tokio::test]
async fn keel_product_functions_list_every_manifest_and_edit_the_enabled_set() {
    let bench = start(
        Composition::new()
            .product(&PROBE_BUNDLE)
            .enabled(Vec::<String>::new()),
    )
    .await;
    assert!(bench.kernel.registry().products().is_empty());
    assert_eq!(bench.kernel.registry().manifests().len(), 1);
    assert!(
        bench
            .kernel
            .registry()
            .function("probe.item.list")
            .is_none()
    );
    assert_eq!(bench.enabled_row().await, Value::Null);
    let listed = bench.as_owner("keel.product.list", json!({})).await;
    assert_eq!(listed["total"], json!(1));
    assert_eq!(
        listed["data"],
        json!([{ "id": "probe", "name": "product.probe.brand", "version": "0.1.0", "state": "disabled",
                 "enabledAt": null, "enabledBy": null }])
    );
    assert_eq!(
        bench
            .as_owner("keel.product.enable", json!({ "id": "probe" }))
            .await,
        json!({ "id": "probe", "state": "enabled", "changed": true, "restart": true })
    );
    let stored = bench.enabled_row().await;
    assert_eq!(stored["probe"]["by"], json!(OWNER_LOGIN));
    let enabled_at = stored["probe"]["enabledAt"]
        .as_str()
        .expect("an RFC 3339 time");
    assert!(
        keel_sdk::chrono::DateTime::parse_from_rfc3339(enabled_at).is_ok(),
        "{enabled_at}"
    );
    assert_eq!(stored.as_object().map(|set| set.len()), Some(1));
    assert_eq!(
        bench
            .as_owner("keel.product.enable", json!({ "id": "probe" }))
            .await,
        json!({ "id": "probe", "state": "enabled", "changed": false, "restart": false })
    );
    let listed = bench.as_owner("keel.product.list", json!({})).await;
    assert_eq!(listed["data"][0]["state"], json!("enabled"));
    assert_eq!(listed["data"][0]["enabledAt"], json!(enabled_at));
    assert_eq!(listed["data"][0]["enabledBy"], json!(OWNER_LOGIN));
    assert!(bench.kernel.registry().products().is_empty());
    assert_eq!(
        bench
            .as_owner("keel.product.disable", json!({ "id": "probe" }))
            .await,
        json!({ "id": "probe", "state": "disabled", "changed": true, "restart": true })
    );
    assert_eq!(bench.enabled_row().await, json!({}));
    assert_eq!(
        bench
            .as_owner("keel.product.disable", json!({ "id": "probe" }))
            .await,
        json!({ "id": "probe", "state": "disabled", "changed": false, "restart": false })
    );
    let unknown = bench
        .call(owner(), "keel.product.enable", json!({ "id": "dns" }))
        .await;
    let error = unknown.error().expect("an unknown product is refused");
    assert_eq!(error.code, ErrorCode::Validation);
    assert_eq!(error.fields[0].path, "id");
    assert_eq!(
        error.fields[0].key.as_deref(),
        Some("keel.product.error.unknown")
    );
    assert_eq!(error.fields[0].params["id"], json!("dns"));
    assert_eq!(bench.enabled_row().await, json!({}));
    let customer: Id = CUSTOMER.parse().expect("an id");
    let refused = bench
        .call(
            principal(Level::User, customer, "en"),
            "keel.product.list",
            json!({}),
        )
        .await;
    assert_eq!(
        refused.error().map(|error| error.code),
        Some(ErrorCode::Forbidden)
    );
    bench.finish().await;
    let enabled = start(Composition::new().product(&PROBE_BUNDLE)).await;
    assert_eq!(enabled.kernel.registry().product_ids(), ["probe"]);
    let listed = enabled.as_owner("keel.product.list", json!({})).await;
    assert_eq!(listed["data"][0]["state"], json!("disabled"));
    let descriptor = enabled
        .get(
            "/api/v1/functions/keel.product.list",
            Some(&format!("owner:{}", Id::ROOT_ACCOUNT)),
            None,
        )
        .await;
    assert_eq!(descriptor.status, StatusCode::OK, "{}", descriptor.body);
    assert_eq!(descriptor.body["access"]["level"], json!("admin"));
    assert_eq!(descriptor.body["mode"], json!("read"));
    assert_eq!(
        descriptor.body["ui"]["actions"][0]["function"],
        json!("keel.product.enable")
    );
    assert_eq!(
        descriptor.body["ui"]["row_actions"],
        json!(["enable", "disable"])
    );
    enabled.finish().await;
    let bare = start(Composition::new()).await;
    let listed = bare.as_owner("keel.product.list", json!({})).await;
    assert_eq!(listed, json!({ "data": [], "total": 0, "totals": null }));
    bare.finish().await;
}

#[tokio::test]
async fn an_enabled_product_the_composition_does_not_carry_fails_the_composition_naming_it() {
    let error = Composition::new()
        .product(&PROBE_BUNDLE)
        .enabled(["probe", "dns"])
        .resolve()
        .expect_err("dns is not carried");
    assert_eq!(
        error.to_string(),
        "the module composition is invalid:\n  - product dns is enabled but not carried by this binary; it carries probe"
    );
    let graph = Composition::new()
        .product(&PROBE_BUNDLE)
        .enabled(["probe"])
        .resolve()
        .expect("probe is carried");
    assert_eq!(graph.product_ids(), ["probe"]);
    assert_eq!(graph.ids(), ["keel", "probe"]);
    let disabled = Composition::new()
        .product(&PROBE_BUNDLE)
        .enabled(Vec::<&str>::new())
        .resolve()
        .expect("nothing enabled is fine");
    assert!(disabled.product_ids().is_empty());
    assert_eq!(disabled.ids(), ["keel"]);
    assert_eq!(disabled.manifests().len(), 1);
}

fn untitled_init(r: &mut Registrar) -> Result<()> {
    r.function(
        function::<ListQuery, Page<Item>>("untitled.item.list")
            .kind(Kind::List)
            .level(Level::User)
            .unowned()
            .ui(List::new("untitled.item.list.title")
                .column(Column::new("weight", ColumnType::Number)))
            .handler(|_: Call<ListQuery>| async { Ok(Page::new(Vec::new(), 0)) }),
    )
    .i18n(
        "ru",
        r#"{ "untitled": { "item": { "list": { "title": "ru:Items" } } } }"#,
    );
    Ok(())
}

static UNTITLED: ModuleDef = ModuleDef {
    id: "untitled",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: untitled_init,
};

#[tokio::test]
async fn a_function_without_an_en_title_key_fails_start_naming_every_missing_key() {
    let database = TestDatabase::create().await;
    let storage = database.storage().await;
    let error = Kernel::boot(Composition::new().module(&UNTITLED), &storage)
        .await
        .expect_err("the start fails");
    let BootError::Freeze(freeze) = &error else {
        panic!("expected a freeze error: {error}");
    };
    assert!(
        matches!(
            freeze.problems(),
            [
                FreezeProblem::Translation { owner, key },
                FreezeProblem::TranslationInherited { key: column, shared, .. },
            ] if owner == "function untitled.item.list"
                && key == "untitled.item.list.title"
                && column == "untitled.item.column.weight"
                && shared == "keel.column.weight"
        ),
        "{freeze}"
    );
    assert_eq!(
        error.to_string(),
        "the registries cannot be frozen:\n  - function untitled.item.list: the en dictionary has no entry for untitled.item.list.title\n  - function untitled.item.list column weight: the en dictionary has no entry for untitled.item.column.weight nor for the shared keel.column.weight"
    );
    storage.close().await.expect("the pool closes");
    database.finish().await;
}
