use std::collections::BTreeMap;
use std::sync::Arc;

use keel_sdk::Registrar;
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ConnectionTrait, DbBackend, Statement};
use keel_sdk::system::PermissionSource;
use keel_testkit::{ErrorCode, Harness, Who, tokio};

const SECRET_READ: &str = "gate.secret.read";
const ACCOUNT_READ: &str = "keel.account.read";

#[derive(Deserialize, Serialize, JsonSchema)]
struct Nothing {}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Seen {
    login: String,
    grants: BTreeMap<String, bool>,
}

struct ByLogin;

#[async_trait]
impl PermissionSource for ByLogin {
    async fn grants(&self, principal: Id, db: &Db) -> Result<BTreeMap<String, bool>> {
        let row = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT login FROM keel_principal WHERE id = $1",
                [principal.to_string().into()],
            ))
            .await?;
        let login: String = row
            .ok_or_else(|| KeelError::not_found("keel.error.not_found"))?
            .try_get("", "login")?;
        Ok(match login.as_str() {
            "a1" => BTreeMap::from([
                (SECRET_READ.to_owned(), true),
                (ACCOUNT_READ.to_owned(), false),
            ]),
            "reseller" => BTreeMap::from([(SECRET_READ.to_owned(), false)]),
            "admin" => BTreeMap::from([(SECRET_READ.to_owned(), false)]),
            _ => BTreeMap::new(),
        })
    }
}

static GATE: ModuleDef = ModuleDef {
    id: "gate",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: gate_init,
};

fn gate_init(r: &mut Registrar) -> Result<()> {
    r.provide::<dyn PermissionSource>(Arc::new(ByLogin))
        .function(
            function::<Nothing, Seen>("gate.secret.read")
                .kind(Kind::Get)
                .access(Level::User, SECRET_READ)
                .unowned()
                .handler(|call: Call<Nothing>| async move {
                    Ok(Seen {
                        login: call.principal().login.clone(),
                        grants: call.principal().grants.clone(),
                    })
                }),
        )
        .permission(SECRET_READ, &[Level::Reseller]);
    Ok(())
}

#[tokio::test]
async fn the_transports_overlay_the_stored_grants_and_the_guard_reads_them_over_the_defaults() {
    let harness = Harness::new().module(&GATE).boot().await;
    let http = harness.http().await;
    let a1 = http.get(Who::A1, "/api/v1/call/gate.secret.read").await;
    assert_eq!(a1.status, 200, "{}", a1.body);
    assert_eq!(
        a1.json()["data"],
        json!({ "login": "a1", "grants": { SECRET_READ: true, ACCOUNT_READ: false } })
    );
    let taken = http.get(Who::A1, "/api/v1/call/keel.account.list").await;
    assert_eq!(taken.status, 403, "{}", taken.body);
    assert_eq!(taken.json()["error"]["key"], json!("keel.error.permission"));
    let a2 = http.get(Who::A2, "/api/v1/call/gate.secret.read").await;
    assert_eq!(a2.status, 403, "{}", a2.body);
    assert_eq!(a2.json()["error"]["key"], json!("keel.error.permission"));
    let reseller = http
        .get(Who::Reseller, "/api/v1/call/gate.secret.read")
        .await;
    assert_eq!(reseller.status, 403, "{}", reseller.body);
    let admin = http.get(Who::Admin, "/api/v1/call/gate.secret.read").await;
    assert_eq!(admin.status, 200, "{}", admin.body);
    assert_eq!(admin.json()["data"]["grants"], json!({}));
    let index = http.get(Who::A1, "/api/v1/functions").await;
    let names: Vec<String> = index
        .json()
        .as_array()
        .expect("descriptors")
        .iter()
        .filter_map(|descriptor| descriptor["name"].as_str().map(str::to_owned))
        .collect();
    assert!(names.contains(&"gate.secret.read".to_owned()), "{names:?}");
    assert!(
        !names.contains(&"keel.account.list".to_owned()),
        "{names:?}"
    );
    let boot = http.get(Who::A1, "/api/v1/boot").await;
    assert!(
        !boot.body.contains("system/accounts"),
        "the denied node is still in the menu: {}",
        boot.body
    );
    http.stop().await.expect("the server stops");

    let socket = harness.socket().await;
    let as_a1 = socket.call_as("gate.secret.read", json!({}), "a1").await;
    assert_eq!(
        serde_json::to_value(&as_a1).expect("an envelope")["data"]["grants"],
        json!({ SECRET_READ: true, ACCOUNT_READ: false })
    );
    let as_a2 = socket.call_as("gate.secret.read", json!({}), "a2").await;
    assert_eq!(
        serde_json::to_value(&as_a2).expect("an envelope")["error"]["key"],
        json!("keel.error.permission")
    );
    let owner = socket.call("gate.secret.read", json!({})).await;
    assert_eq!(
        serde_json::to_value(&owner).expect("an envelope")["data"]["login"],
        json!("owner")
    );
    socket.stop().await.expect("the socket stops");

    let direct = harness.call(Who::A1, "gate.secret.read", json!({})).await;
    assert_eq!(
        direct.error().map(|error| error.code),
        Some(ErrorCode::Forbidden)
    );
    harness.finish().await;
}
