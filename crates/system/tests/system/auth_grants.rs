use std::path::{Path, PathBuf};

use keel_sdk::Registrar;
use keel_sdk::prelude::*;
use keel_system::auth::MODULE;
use keel_testkit::{Answer, ErrorCode, Fixture, Harness, TestHttp, Who, tokio};

const SECRET_READ: &str = "probe.secret.read";
const NOBODY: &str = "probe.nobody";
const ACCOUNT_READ: &str = "keel.account.read";
const AUTH_NODES: [&str; 4] = [
    "system/sessions",
    "system/tokens",
    "system/ip-rules",
    "system/grants",
];

fn snapshots_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Nothing {}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Seen {
    seen: bool,
}

static PROBE: ModuleDef = ModuleDef {
    id: "probe",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: probe_init,
};

fn probe_init(r: &mut Registrar) -> Result<()> {
    r.function(
        function::<Nothing, Seen>("probe.secret.read")
            .kind(Kind::Get)
            .access(Level::User, SECRET_READ)
            .unowned()
            .handler(|_: Call<Nothing>| async { Ok(Seen { seen: true }) }),
    )
    .permission(SECRET_READ, &[Level::Reseller])
    .permission(NOBODY, &[]);
    Ok(())
}

async fn boot() -> (Harness, TestHttp) {
    let harness = Harness::new().module(&MODULE).module(&PROBE).boot().await;
    let http = harness.http().await;
    (harness, http)
}

async fn call(http: &TestHttp, who: Who, func: &str, params: Value) -> Answer {
    http.post(who, &format!("/api/v1/call/{func}"), &params)
        .await
}

async fn set(http: &TestHttp, who: Who, principal: Id, key: &str, granted: bool) -> Answer {
    call(
        http,
        who,
        "auth.grant.set",
        json!({ "principal": principal, "key": key, "granted": granted }),
    )
    .await
}

fn error_of(answer: &Answer) -> (String, String) {
    let error = &answer.json()["error"];
    (
        error["code"].as_str().unwrap_or_default().to_owned(),
        error["key"].as_str().unwrap_or_default().to_owned(),
    )
}

async fn etag(http: &TestHttp, who: Who) -> String {
    let answer = http.get(who, "/api/v1/functions").await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    answer
        .header("etag")
        .unwrap_or_else(|| panic!("no ETag on the index of {who}: {:?}", answer.headers))
        .to_owned()
}

async fn index_names(http: &TestHttp, who: Who) -> Vec<String> {
    let answer = http.get(who, "/api/v1/functions").await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    answer
        .json()
        .as_array()
        .expect("descriptors")
        .iter()
        .filter_map(|descriptor| descriptor["name"].as_str().map(str::to_owned))
        .collect()
}

async fn system_paths(http: &TestHttp, who: Who) -> Vec<String> {
    let answer = http.get(who, "/api/v1/boot").await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    answer.json()["menu"]
        .as_array()
        .expect("groups")
        .iter()
        .filter(|group| group["id"] == json!("system"))
        .flat_map(|group| group["items"].as_array().cloned().unwrap_or_default())
        .filter_map(|item| item["path"].as_str().map(str::to_owned))
        .collect()
}

async fn acl_gen(harness: &Harness, principal: Id) -> String {
    harness
        .database()
        .rows(&format!(
            "SELECT acl_gen::text FROM keel_principal WHERE id = '{principal}'"
        ))
        .await
        .pop()
        .expect("the principal row")
}

async fn create_principal(harness: &Harness, login: &str, level: Level, account: Id) -> Id {
    let reply = harness
        .call(
            Who::Owner,
            "keel.principal.create",
            json!({ "login": login, "level": level, "account": account }),
        )
        .await;
    reply
        .data()
        .and_then(|data| data["id"].as_str())
        .and_then(|id| id.parse().ok())
        .unwrap_or_else(|| panic!("keel.principal.create {login}: {:?}", reply.error()))
}

struct GrantFixture;

#[async_trait]
impl Fixture for GrantFixture {
    async fn create(&self, harness: &Harness, account: Id) -> String {
        let (who, principal) = harness
            .fixtures()
            .all()
            .find(|(who, principal)| principal.account_id == account && who.level() == Level::User)
            .unwrap_or_else(|| panic!("no user of the fixture tree lives in account {account}"));
        let reply = harness
            .call(
                Who::Owner,
                "auth.grant.set",
                json!({ "principal": principal.id, "key": SECRET_READ, "granted": true }),
            )
            .await;
        let data = reply
            .data()
            .unwrap_or_else(|| panic!("auth.grant.set for {who}: {:?}", reply.error()));
        data["id"].as_str().expect("an id").to_owned()
    }
}

#[tokio::test]
async fn a_stored_grant_overrides_the_default_and_a_denial_takes_a_default_key_away_until_deleted()
{
    let (harness, http) = boot().await;
    let a1 = harness.principal(Who::A1).id;
    let accounts = harness.fixtures().accounts.clone();

    let refused = call(&http, Who::A1, "probe.secret.read", json!({})).await;
    assert_eq!(refused.status, 403, "{}", refused.body);
    assert_eq!(
        error_of(&refused),
        ("forbidden".into(), "keel.error.permission".into())
    );
    assert_eq!(
        refused.json()["error"]["params"]["permission"],
        json!(SECRET_READ)
    );
    assert!(
        !index_names(&http, Who::A1)
            .await
            .contains(&"probe.secret.read".to_owned())
    );
    let before = etag(&http, Who::A1).await;
    assert_eq!(acl_gen(&harness, a1).await, "0");

    let granted = set(&http, Who::Reseller, a1, SECRET_READ, true).await;
    assert_eq!(granted.status, 200, "{}", granted.body);
    let saved = granted.json()["data"].clone();
    assert_eq!(saved["principal"], json!(a1.to_string()));
    assert_eq!(saved["key"], json!(SECRET_READ));
    assert_eq!(saved["granted"], json!(true));
    let admitted = call(&http, Who::A1, "probe.secret.read", json!({})).await;
    assert_eq!(admitted.status, 200, "{}", admitted.body);
    assert_eq!(admitted.json()["data"], json!({ "seen": true }));
    assert!(
        index_names(&http, Who::A1)
            .await
            .contains(&"probe.secret.read".to_owned())
    );
    let after_grant = etag(&http, Who::A1).await;
    assert_ne!(before, after_grant);
    assert_eq!(acl_gen(&harness, a1).await, "1");

    let again = set(&http, Who::Reseller, a1, SECRET_READ, true).await;
    assert_eq!(again.status, 200, "{}", again.body);
    assert_eq!(again.json()["data"]["id"], saved["id"]);
    assert_eq!(acl_gen(&harness, a1).await, "2");

    let accounts_before = call(&http, Who::A1, "keel.account.list", json!({})).await;
    assert_eq!(accounts_before.status, 200, "{}", accounts_before.body);
    assert!(
        system_paths(&http, Who::A1)
            .await
            .contains(&"system/accounts".to_owned())
    );
    let denied = set(&http, Who::Reseller, a1, ACCOUNT_READ, false).await;
    assert_eq!(denied.status, 200, "{}", denied.body);
    assert_eq!(denied.json()["data"]["granted"], json!(false));
    let denial = denied.json()["data"]["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let taken = call(&http, Who::A1, "keel.account.list", json!({})).await;
    assert_eq!(taken.status, 403, "{}", taken.body);
    assert_eq!(
        error_of(&taken),
        ("forbidden".into(), "keel.error.permission".into())
    );
    assert_eq!(
        taken.json()["error"]["params"]["permission"],
        json!(ACCOUNT_READ)
    );
    assert!(
        !system_paths(&http, Who::A1)
            .await
            .contains(&"system/accounts".to_owned())
    );
    let after_denial = etag(&http, Who::A1).await;
    assert_ne!(after_grant, after_denial);
    let still_by_default = call(&http, Who::A2, "keel.account.list", json!({})).await;
    assert_eq!(still_by_default.status, 200, "{}", still_by_default.body);
    let direct = harness.call(Who::A1, "keel.account.list", json!({})).await;
    assert!(direct.data().is_some(), "{:?}", direct.error());

    let listed = call(
        &http,
        Who::Reseller,
        "auth.grant.list",
        json!({ "search": "keel.account", "sort": { "key": "key", "dir": "asc" } }),
    )
    .await;
    assert_eq!(listed.status, 200, "{}", listed.body);
    let page = listed.json()["data"].clone();
    assert_eq!(page["total"], json!(1));
    let row = &page["data"][0];
    assert_eq!(row["id"], json!(denial));
    assert_eq!(row["principalId"], json!(a1.to_string()));
    assert_eq!(row["principalLogin"], json!("a1"));
    assert_eq!(row["accountId"], json!(accounts.a1.to_string()));
    assert_eq!(row["key"], json!(ACCOUNT_READ));
    assert_eq!(row["granted"], json!(false));
    assert_eq!(
        row["createdBy"],
        json!(harness.principal(Who::Reseller).id.to_string())
    );
    let by_login = call(
        &http,
        Who::Reseller,
        "auth.grant.list",
        json!({ "search": "a1", "filters": [{ "key": "granted", "value": "true" }] }),
    )
    .await;
    assert_eq!(by_login.status, 200, "{}", by_login.body);
    assert_eq!(by_login.json()["data"]["total"], json!(1));
    assert_eq!(
        by_login.json()["data"]["data"][0]["key"],
        json!(SECRET_READ)
    );

    let deleted = call(
        &http,
        Who::Reseller,
        "auth.grant.delete",
        json!({ "id": denial }),
    )
    .await;
    assert_eq!(deleted.status, 200, "{}", deleted.body);
    assert_eq!(deleted.json()["data"], json!({ "deleted": true }));
    let restored = call(&http, Who::A1, "keel.account.list", json!({})).await;
    assert_eq!(restored.status, 200, "{}", restored.body);
    let after_delete = etag(&http, Who::A1).await;
    assert_ne!(after_denial, after_delete);
    assert_eq!(acl_gen(&harness, a1).await, "4");
    let gone = call(
        &http,
        Who::Reseller,
        "auth.grant.delete",
        json!({ "id": denial }),
    )
    .await;
    assert_eq!(gone.status, 404, "{}", gone.body);

    let not_held = set(&http, Who::Reseller, a1, NOBODY, true).await;
    assert_eq!(not_held.status, 403, "{}", not_held.body);
    assert_eq!(
        error_of(&not_held),
        ("forbidden".into(), "auth.error.grant_not_held".into())
    );
    assert_eq!(not_held.json()["error"]["params"]["key"], json!(NOBODY));
    let unknown = set(&http, Who::Reseller, a1, "probe.unknown", true).await;
    assert_eq!(unknown.status, 422, "{}", unknown.body);
    assert_eq!(unknown.json()["error"]["code"], json!("validation"));
    assert_eq!(unknown.json()["error"]["fields"][0]["path"], json!("key"));
    assert_eq!(
        unknown.json()["error"]["fields"][0]["key"],
        json!("auth.error.unknown_permission")
    );
    let by_admin = set(&http, Who::Admin, a1, NOBODY, true).await;
    assert_eq!(by_admin.status, 200, "{}", by_admin.body);

    let second_reseller =
        create_principal(&harness, "reseller2", Level::Reseller, accounts.reseller).await;
    let same_level = set(&http, Who::Reseller, second_reseller, SECRET_READ, true).await;
    assert_eq!(same_level.status, 403, "{}", same_level.body);
    assert_eq!(
        error_of(&same_level),
        ("forbidden".into(), "auth.error.not_below".into())
    );
    for target in [
        harness.principal(Who::Unrelated).id,
        harness.principal(Who::Admin).id,
    ] {
        let foreign = set(&http, Who::Reseller, target, SECRET_READ, true).await;
        assert_eq!(foreign.status, 404, "{target}: {}", foreign.body);
        assert_eq!(foreign.json()["error"]["code"], json!("not_found"));
    }
    let by_user = set(&http, Who::A2, a1, SECRET_READ, true).await;
    assert_eq!(by_user.status, 403, "{}", by_user.body);
    assert_eq!(
        error_of(&by_user),
        ("forbidden".into(), "keel.error.level".into())
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn grants_are_rows_of_the_principals_account_and_the_reseller_sees_its_subtree_only() {
    let harness = Harness::new().module(&MODULE).module(&PROBE).boot().await;
    harness
        .assert_reseller_sees_only_subtree("auth.grant.list", &GrantFixture)
        .await;
    let foreign = GrantFixture
        .create(&harness, harness.fixtures().accounts.unrelated)
        .await;
    let by_reseller = harness
        .call(Who::Reseller, "auth.grant.delete", json!({ "id": foreign }))
        .await;
    assert_eq!(
        by_reseller
            .error()
            .map(|error| (error.code, error.key.clone())),
        Some((ErrorCode::NotFound, "keel.error.not_found".into()))
    );
    for func in ["auth.grant.list", "auth.grant.delete"] {
        let by_user = harness
            .call(Who::Unrelated, func, json!({ "id": foreign }))
            .await;
        assert_eq!(
            by_user.error().map(|error| (error.code, error.key.clone())),
            Some((ErrorCode::Forbidden, "keel.error.level".into())),
            "{func}"
        );
    }
    let by_owner = harness
        .call(Who::Owner, "auth.grant.delete", json!({ "id": foreign }))
        .await;
    assert_eq!(by_owner.data(), Some(&json!({ "deleted": true })));
    harness.finish().await;
}

#[tokio::test]
async fn the_grant_descriptors_and_the_auth_menu_nodes_match_their_snapshots() {
    let harness = Harness::new().module(&MODULE).boot().await;
    for level in [Level::Reseller, Level::Admin, Level::Owner] {
        let names = harness.snapshot_descriptors(level, snapshots_dir());
        for name in ["auth.grant.list", "auth.grant.set", "auth.grant.delete"] {
            assert!(names.contains(&name.to_owned()), "{level}: {names:?}");
        }
    }
    let user = harness.snapshot_descriptors(Level::User, snapshots_dir());
    assert!(
        !user.iter().any(|name| name.starts_with("auth.grant.")),
        "{user:?}"
    );
    let http = harness.http().await;
    let mut settings = insta::Settings::new();
    settings.set_snapshot_path(snapshots_dir());
    settings.set_prepend_module_to_snapshot(false);
    settings.set_omit_expression(true);
    for who in [Who::A1, Who::Reseller, Who::Admin, Who::Owner] {
        let answer = http.get(who, "/api/v1/boot").await;
        assert_eq!(answer.status, 200, "{}", answer.body);
        let nodes: Vec<Value> = answer.json()["menu"]
            .as_array()
            .expect("groups")
            .iter()
            .filter(|group| group["id"] == json!("system"))
            .flat_map(|group| group["items"].as_array().cloned().unwrap_or_default())
            .filter(|item| {
                item["func"]
                    .as_str()
                    .is_some_and(|func| func.starts_with("auth."))
            })
            .collect();
        settings.set_snapshot_suffix(who.level().as_str());
        settings.bind(|| insta::assert_json_snapshot!("auth.menu", nodes));
    }
    let user = system_paths(&http, Who::A1).await;
    assert!(user.contains(&"system/sessions".to_owned()), "{user:?}");
    assert!(user.contains(&"system/tokens".to_owned()), "{user:?}");
    assert!(!user.contains(&"system/ip-rules".to_owned()), "{user:?}");
    assert!(!user.contains(&"system/grants".to_owned()), "{user:?}");
    let admin = system_paths(&http, Who::Admin).await;
    for path in AUTH_NODES {
        assert!(admin.contains(&path.to_owned()), "{admin:?}");
    }
    let guest = system_paths(&http, Who::Guest).await;
    assert!(
        !guest.iter().any(|path| AUTH_NODES.contains(&path.as_str())),
        "{guest:?}"
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}
