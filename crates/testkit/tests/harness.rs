use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use keel_sdk::Registrar;
use keel_sdk::prelude::*;
use keel_testkit::{ErrorCode, Fixture, Harness, Server, TestDatabase, Who, database};

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Pick {
    id: Id,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Saved {
    id: Id,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Row {
    id: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct SampleQuery {
    name: String,
    #[schemars(extend("x-keel" = json!({ "level": "reseller" })))]
    account: Option<Id>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Nothing {}

static SAMPLE: ModuleDef = ModuleDef {
    id: "sample",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: init_sample,
};

fn init_sample(r: &mut Registrar) -> Result<()> {
    r.function(
        function::<SampleQuery, Row>("sample.get")
            .kind(Kind::Get)
            .level(Level::User)
            .unowned()
            .handler(|call: Call<SampleQuery>| async move {
                Ok(Row {
                    id: call.params().name.clone(),
                })
            }),
    )
    .function(
        function::<Nothing, Nothing>("sample.admin")
            .kind(Kind::Action)
            .level(Level::Admin)
            .unowned()
            .handler(|_: Call<Nothing>| async { Ok(Nothing {}) }),
    );
    Ok(())
}

static LEAKY_ROWS: Mutex<Vec<String>> = Mutex::new(Vec::new());

static LEAKY: ModuleDef = ModuleDef {
    id: "leaky",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: init_leaky,
};

fn init_leaky(r: &mut Registrar) -> Result<()> {
    r.function(
        function::<ListQuery, Page<Row>>("leaky.list")
            .kind(Kind::List)
            .level(Level::User)
            .unowned()
            .handler(|_: Call<ListQuery>| async {
                let rows: Vec<Row> = LEAKY_ROWS
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .iter()
                    .map(|id| Row { id: id.clone() })
                    .collect();
                let total = rows.len() as u64;
                Ok(Page::new(rows, total))
            }),
    )
    .function(
        function::<Pick, Saved>("leaky.edit")
            .kind(Kind::Form)
            .level(Level::User)
            .unowned()
            .handler(|call: Call<Pick>| async move {
                Ok(Saved {
                    id: call.params().id,
                })
            }),
    );
    Ok(())
}

struct LeakyFixture;

#[async_trait]
impl Fixture for LeakyFixture {
    async fn create(&self, _: &Harness, _: Id) -> String {
        let id = Id::generate().to_string();
        LEAKY_ROWS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(id.clone());
        id
    }
}

struct PrincipalFixture;

#[async_trait]
impl Fixture for PrincipalFixture {
    async fn create(&self, harness: &Harness, account: Id) -> String {
        let reply = harness
            .call(
                Who::Owner,
                "keel.principal.create",
                json!({
                    "login": format!("member-{}", Id::generate().to_string().to_lowercase()),
                    "level": "user",
                    "account": account
                }),
            )
            .await;
        id_of(reply.data()).to_string()
    }
}

struct AccountFixture;

#[async_trait]
impl Fixture for AccountFixture {
    async fn create(&self, harness: &Harness, account: Id) -> String {
        let reply = harness
            .call(
                Who::Owner,
                "keel.account.create",
                json!({ "kind": "user", "parent": account }),
            )
            .await;
        id_of(reply.data()).to_string()
    }
}

fn id_of(data: Option<&Value>) -> Id {
    data.and_then(|data| data["id"].as_str())
        .and_then(|id| id.parse().ok())
        .unwrap_or_else(|| panic!("a saved id was expected, got {data:?}"))
}

fn ids(page: &Value) -> Vec<String> {
    let mut ids: Vec<String> = page["data"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["id"].as_str().expect("an id").to_owned())
        .collect();
    ids.sort();
    ids
}

fn sorted(ids: &[Id]) -> Vec<String> {
    let mut ids: Vec<String> = ids.iter().map(ToString::to_string).collect();
    ids.sort();
    ids
}

fn snapshots_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

#[tokio::test]
async fn a_bare_harness_seeds_the_fixture_tree_and_answers_by_level() {
    let harness = Harness::new().boot().await;
    let accounts = &harness.fixtures().accounts;
    assert_eq!(accounts.root, Id::ROOT_ACCOUNT);

    let a1 = harness
        .call(Who::Owner, "keel.account.get", json!({ "id": accounts.a1 }))
        .await;
    let a1 = a1.data().expect("the account a1");
    assert_eq!(a1["parentId"], json!(accounts.reseller.to_string()));
    assert_eq!(a1["kind"], json!("user"));
    assert_eq!(
        a1["path"],
        json!(format!("/{}/{}/", accounts.reseller, accounts.a1))
    );
    assert_eq!(
        a1["createdAt"],
        json!(
            harness
                .clock()
                .now()
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        )
    );
    let unrelated = harness
        .call(
            Who::Owner,
            "keel.account.get",
            json!({ "id": accounts.unrelated }),
        )
        .await;
    assert_eq!(
        unrelated.data().expect("the account u")["parentId"],
        json!(Id::ROOT_ACCOUNT.to_string())
    );

    let everything = sorted(&[
        accounts.root,
        accounts.reseller,
        accounts.a1,
        accounts.a2,
        accounts.unrelated,
    ]);
    let owner = harness
        .call_as(Level::Owner, "keel.account.list", json!({}))
        .await;
    assert_eq!(ids(owner.data().expect("a page")), everything);
    let admin = harness
        .call_as(Level::Admin, "keel.account.list", json!({}))
        .await;
    assert_eq!(ids(admin.data().expect("a page")), everything);
    let reseller = harness
        .call_as(Level::Reseller, "keel.account.list", json!({}))
        .await;
    assert_eq!(
        ids(reseller.data().expect("a page")),
        sorted(&[accounts.reseller, accounts.a1, accounts.a2])
    );
    let user = harness
        .call_as(Level::User, "keel.account.list", json!({}))
        .await;
    assert_eq!(ids(user.data().expect("a page")), sorted(&[accounts.a1]));
    let a2 = harness.call(Who::A2, "keel.account.list", json!({})).await;
    assert_eq!(ids(a2.data().expect("a page")), sorted(&[accounts.a2]));
    let service = harness
        .call_as(Level::Service, "keel.account.list", json!({}))
        .await;
    assert_eq!(
        service.error().map(|error| error.code),
        Some(ErrorCode::Forbidden)
    );
    let guest = harness
        .call_as(Level::Guest, "keel.account.list", json!({}))
        .await;
    assert_eq!(guest.status(), 401);

    let logins = harness
        .call(Who::Owner, "keel.principal.list", json!({}))
        .await;
    let logins: Vec<&str> = logins.data().expect("a page")["data"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["login"].as_str().expect("a login"))
        .collect();
    assert_eq!(
        logins,
        ["a1", "a2", "admin", "reseller", "service", "unrelated"]
    );
    for (who, principal) in harness.fixtures().all() {
        assert_eq!(principal.login, who.login());
        assert_eq!(principal.level, who.level());
    }
    assert_eq!(
        harness.fixtures().account_of(Who::Reseller),
        accounts.reseller
    );

    harness.clock().advance(chrono::Duration::hours(1));
    let later = harness
        .call(
            Who::Owner,
            "keel.account.create",
            json!({ "kind": "user", "parent": accounts.a2 }),
        )
        .await;
    let later = harness
        .call(
            Who::Owner,
            "keel.account.get",
            json!({ "id": id_of(later.data()) }),
        )
        .await;
    assert_eq!(
        later.data().expect("the account")["createdAt"],
        json!(
            harness
                .clock()
                .now()
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        )
    );

    let name = harness.database().name().to_owned();
    assert!(name.starts_with(database::PREFIX));
    assert!(database::exists(&name).await.expect("pg_database answers"));
    harness.finish().await;
    assert!(!database::exists(&name).await.expect("pg_database answers"));
}

#[tokio::test]
async fn the_database_is_dropped_when_a_test_panics_before_finish() {
    let harness = Harness::new().boot().await;
    let name = harness.database().name().to_owned();
    let outcome = catch_unwind(AssertUnwindSafe(move || {
        let _harness = harness;
        panic!("the test body fails");
    }));
    assert!(outcome.is_err());
    assert!(!database::exists(&name).await.expect("pg_database answers"));

    let database = TestDatabase::create().await;
    let name = database.name().to_owned();
    assert!(database::exists(&name).await.expect("pg_database answers"));
    drop(database);
    assert!(!database::exists(&name).await.expect("pg_database answers"));
}

#[tokio::test]
async fn the_server_is_the_named_one_when_the_variable_is_set_and_a_started_container_otherwise() {
    let database = TestDatabase::create().await;
    let named = std::env::var(database::VARIABLE).ok();
    match named {
        Some(url) => {
            assert_eq!(database.server(), Server::Named);
            assert_eq!(database.admin_url(), url);
        }
        None => {
            assert_eq!(database.server(), Server::Container);
            assert!(
                database
                    .admin_url()
                    .starts_with("postgres://postgres:postgres@"),
                "{}",
                database.admin_url()
            );
        }
    }
    let harness = Harness::new().boot().await;
    assert_eq!(harness.database().server(), database.server());
    assert_eq!(harness.database().admin_url(), database.admin_url());
    harness.finish().await;
    database.finish().await;
}

#[tokio::test]
async fn the_standard_helpers_pass_against_the_kernel_functions() {
    let harness = Harness::new().boot().await;
    harness
        .assert_reseller_sees_only_subtree("keel.principal.list", &PrincipalFixture)
        .await;
    harness
        .assert_reseller_sees_only_subtree("keel.account.list", &AccountFixture)
        .await;
    harness
        .assert_user_cannot_edit_foreign_row("keel.principal.update", &PrincipalFixture)
        .await;
    harness.finish().await;
}

#[tokio::test]
async fn the_standard_helpers_fail_against_a_module_that_leaks_foreign_rows() {
    let harness = std::sync::Arc::new(Harness::new().module(&LEAKY).boot().await);
    let listing = tokio::spawn({
        let harness = harness.clone();
        async move {
            harness
                .assert_reseller_sees_only_subtree("leaky.list", &LeakyFixture)
                .await
        }
    })
    .await;
    assert!(listing.is_err_and(|error| error.is_panic()));
    let editing = tokio::spawn({
        let harness = harness.clone();
        async move {
            harness
                .assert_user_cannot_edit_foreign_row("leaky.edit", &LeakyFixture)
                .await
        }
    })
    .await;
    assert!(editing.is_err_and(|error| error.is_panic()));
    let harness =
        std::sync::Arc::try_unwrap(harness).unwrap_or_else(|_| panic!("the harness is shared"));
    harness.finish().await;
}

#[tokio::test]
async fn the_http_server_maps_the_test_header_to_the_fixture_principals() {
    let harness = Harness::new().boot().await;
    let server = harness.http().await;
    assert_ne!(server.addr().port(), 0);
    assert_eq!(
        server.url("/healthz"),
        format!("http://{}/healthz", server.addr())
    );

    let health = server.get(Who::Guest, "/healthz").await;
    assert_eq!(health.status, 200);
    assert_eq!(health.json()["status"], json!("ok"));
    let ready = server.get(Who::Guest, "/readyz").await;
    assert_eq!(ready.status, 200, "{}", ready.body);

    let reseller = server
        .get(Who::Reseller, "/api/v1/call/keel.account.list")
        .await;
    assert_eq!(reseller.status, 200, "{}", reseller.body);
    assert!(reseller.header("x-keel-request-id").is_some());
    let body = reseller.json();
    assert_eq!(body["meta"]["principal"]["login"], json!("reseller"));
    assert_eq!(body["meta"]["principal"]["level"], json!("reseller"));
    let accounts = &harness.fixtures().accounts;
    assert_eq!(
        ids(&body["data"]),
        sorted(&[accounts.reseller, accounts.a1, accounts.a2])
    );

    let owner = server
        .post(Who::Owner, "/api/v1/call/keel.account.list", &json!({}))
        .await;
    assert_eq!(owner.status, 200, "{}", owner.body);
    assert_eq!(owner.json()["meta"]["principal"]["login"], json!("owner"));

    let guest = server
        .get(Who::Guest, "/api/v1/call/keel.account.list")
        .await;
    assert_eq!(guest.status, 401, "{}", guest.body);
    let nobody = server
        .request(
            "GET",
            "/api/v1/call/keel.account.list",
            &[(keel_testkit::PRINCIPAL_HEADER, "nobody")],
            None,
        )
        .await;
    assert_eq!(nobody.status, 401, "{}", nobody.body);
    assert_eq!(nobody.json()["error"]["code"], json!("unauthenticated"));
    let cookie = format!("{}={}", keel_testkit::SESSION_COOKIE, Who::Admin);
    let session = server
        .request(
            "GET",
            "/api/v1/call/keel.account.list",
            &[("cookie", cookie.as_str())],
            None,
        )
        .await;
    assert_eq!(session.status, 401, "{}", session.body);
    assert_eq!(session.json()["error"]["code"], json!("unauthenticated"));

    let boot = server.get(Who::A2, "/api/v1/boot").await;
    assert_eq!(boot.status, 200, "{}", boot.body);
    assert_eq!(
        boot.json()["principal"]["account"],
        json!(accounts.a2.to_string())
    );

    server.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn descriptor_snapshots_match_the_committed_files_and_a_changed_descriptor_fails() {
    let harness = Harness::new().module(&SAMPLE).boot().await;
    let committed = snapshots_dir();
    let user = harness.snapshot_descriptors(Level::User, &committed);
    assert_eq!(user, ["sample.get"]);
    let admin = harness.snapshot_descriptors(Level::Admin, &committed);
    assert_eq!(admin, ["sample.admin", "sample.get"]);
    let guest = harness.snapshot_descriptors(Level::Guest, &committed);
    assert!(guest.is_empty());

    let scratch = tempfile::tempdir().expect("a scratch directory");
    for entry in fs::read_dir(&committed).expect("the snapshot directory") {
        let entry = entry.expect("an entry");
        if entry.path().extension().is_some_and(|ext| ext == "snap") {
            fs::copy(entry.path(), scratch.path().join(entry.file_name())).expect("a copy");
        }
    }
    let untouched = catch_unwind(AssertUnwindSafe(|| {
        harness.snapshot_descriptors(Level::User, scratch.path())
    }));
    assert!(untouched.is_ok());

    let changed = scratch.path().join("sample.get@user.snap");
    let text = fs::read_to_string(&changed).expect("the copied snapshot");
    assert!(text.contains("\"level\": \"user\""), "{text}");
    fs::write(
        &changed,
        text.replace("\"level\": \"user\"", "\"level\": \"reseller\""),
    )
    .expect("the snapshot is rewritten");
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        harness.snapshot_descriptors(Level::User, scratch.path())
    }));
    let message = match outcome {
        Ok(_) => panic!("a changed descriptor must fail the snapshot"),
        Err(payload) => payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| {
                payload
                    .downcast_ref::<&str>()
                    .map(|text| (*text).to_owned())
            })
            .unwrap_or_default(),
    };
    assert!(
        message.contains("snapshot assertion for 'sample.get@user' failed"),
        "{message}"
    );
    harness.finish().await;
}
