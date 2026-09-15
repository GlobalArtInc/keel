use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use keel_sdk::prelude::*;
use keel_sdk::system::Client;
use keel_system::auth::{KIND_SESSION, KIND_SU, MODULE, REASON_LOGOUT, REASON_REVOKED};
use keel_testkit::{Answer, CallOptions, Fixture, Harness, TestHttp, Who, tokio};

const PASSWORD: &str = "correct-horse";
const JSON: (&str, &str) = ("content-type", "application/json");

static AGENTS: AtomicUsize = AtomicUsize::new(0);

fn snapshots_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

async fn set_password(harness: &Harness, id: Id) {
    let set = harness
        .call(
            Who::Owner,
            "auth.password.set",
            json!({ "principal": id, "password": PASSWORD }),
        )
        .await;
    assert!(set.data().is_some(), "{id}: {:?}", set.error());
}

async fn boot() -> (Harness, TestHttp) {
    let harness = Harness::new().module(&MODULE).boot().await;
    for who in [Who::Admin, Who::Reseller, Who::A1] {
        set_password(&harness, harness.principal(who).id).await;
    }
    let http = harness.http().await;
    (harness, http)
}

async fn login(http: &TestHttp, login: &str, agent: &str) -> String {
    let answer = http
        .request(
            "POST",
            "/api/v1/call/auth.login",
            &[JSON, ("user-agent", agent)],
            Some(&json!({ "login": login, "password": PASSWORD }).to_string()),
        )
        .await;
    assert_eq!(answer.status, 200, "{login}: {}", answer.body);
    answer
        .session_cookie()
        .unwrap_or_else(|| panic!("no session cookie for {login}: {:?}", answer.headers))
}

async fn call(http: &TestHttp, cookie: &str, func: &str, params: Value) -> Answer {
    http.request(
        "POST",
        &format!("/api/v1/call/{func}"),
        &[("cookie", cookie), JSON],
        Some(&params.to_string()),
    )
    .await
}

async fn sessions(http: &TestHttp, cookie: &str) -> Vec<Value> {
    let answer = call(http, cookie, "auth.session.list", json!({})).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    let page = answer.json();
    page["data"]["data"].as_array().expect("rows").clone()
}

fn by_agent<'a>(rows: &'a [Value], agent: &str) -> &'a Value {
    rows.iter()
        .find(|row| row["userAgent"] == json!(agent))
        .unwrap_or_else(|| panic!("no session of {agent} in {rows:?}"))
}

fn id_of(row: &Value) -> String {
    row["id"].as_str().expect("an id").to_owned()
}

async fn create_principal(harness: &Harness, login: &str, account: Id) -> Id {
    let reply = harness
        .call(
            Who::Owner,
            "keel.principal.create",
            json!({ "login": login, "level": "user", "account": account }),
        )
        .await;
    let id: Id = reply
        .data()
        .and_then(|data| data["id"].as_str())
        .and_then(|id| id.parse().ok())
        .unwrap_or_else(|| panic!("keel.principal.create {login}: {:?}", reply.error()));
    set_password(harness, id).await;
    id
}

async fn reasons(harness: &Harness) -> Vec<String> {
    harness
        .database()
        .rows("SELECT kind || ':' || coalesce(reason, '-') FROM auth_session ORDER BY kind, reason")
        .await
}

struct SessionFixture;

#[async_trait]
impl Fixture for SessionFixture {
    async fn create(&self, harness: &Harness, account: Id) -> String {
        let (who, principal) = harness
            .fixtures()
            .all()
            .find(|(who, principal)| {
                principal.account_id == account && *who != Who::Guest && *who != Who::Owner
            })
            .unwrap_or_else(|| panic!("no fixture principal lives in account {account}"));
        set_password(harness, principal.id).await;
        let agent = format!("fixture/{}", AGENTS.fetch_add(1, Ordering::SeqCst));
        let opened = harness
            .call_with(
                harness.principal(Who::Guest),
                "auth.login",
                json!({ "login": who.login(), "password": PASSWORD }),
                CallOptions {
                    client: Some(Client {
                        ip: Some("127.0.0.1".parse().expect("an address")),
                        user_agent: Some(agent.clone()),
                        session: None,
                    }),
                    ..CallOptions::default()
                },
            )
            .await;
        assert!(opened.data().is_some(), "{who}: {:?}", opened.error());
        let listed = harness
            .call(
                Who::Owner,
                "auth.session.list",
                json!({ "filters": [{ "key": "userAgent", "value": agent }] }),
            )
            .await;
        let page = listed
            .data()
            .unwrap_or_else(|| panic!("auth.session.list: {:?}", listed.error()));
        assert_eq!(page["total"], json!(1), "{page}");
        id_of(&page["data"][0])
    }
}

#[tokio::test]
async fn each_principal_lists_its_own_sessions_the_reseller_its_subtree_the_admin_all_and_the_current_row_is_marked()
 {
    let (harness, http) = boot().await;
    let a1 = harness.principal(Who::A1).clone();
    create_principal(&harness, "a1-member", a1.account_id).await;
    let first = login(&http, "a1", "a1/first").await;
    let second = login(&http, "a1", "a1/second").await;
    let members = login(&http, "a1-member", "member/only").await;
    let reseller = login(&http, "reseller", "reseller/own").await;
    let admin = login(&http, "admin", "admin/own").await;

    let own = sessions(&http, &first).await;
    assert_eq!(own.len(), 2, "{own:?}");
    let current = by_agent(&own, "a1/first");
    assert_eq!(current["current"], json!(true));
    assert_eq!(current["kind"], json!(KIND_SESSION));
    assert_eq!(current["ip"], json!("127.0.0.1"));
    assert_eq!(current["actor"], json!(null));
    assert_eq!(current["revokedAt"], json!(null));
    assert_eq!(id_of(current).len(), 64);
    assert!(current["createdAt"].is_string());
    assert!(current["idleUntil"].is_string());
    assert!(current["absoluteUntil"].is_string());
    let keys: Vec<&String> = current.as_object().expect("an object").keys().collect();
    assert_eq!(
        keys,
        [
            "id",
            "kind",
            "ip",
            "userAgent",
            "createdAt",
            "idleUntil",
            "absoluteUntil",
            "actor",
            "current",
            "revokedAt"
        ]
    );
    assert_eq!(by_agent(&own, "a1/second")["current"], json!(false));
    let seen_by_second = sessions(&http, &second).await;
    assert_eq!(
        by_agent(&seen_by_second, "a1/second")["current"],
        json!(true)
    );

    let of_member = sessions(&http, &members).await;
    assert_eq!(of_member.len(), 1, "{of_member:?}");
    assert_eq!(of_member[0]["userAgent"], json!("member/only"));
    let foreign = call(
        &http,
        &members,
        "auth.session.revoke",
        json!({ "id": id_of(current) }),
    )
    .await;
    assert_eq!(foreign.status, 404, "{}", foreign.body);
    assert_eq!(foreign.json()["error"]["code"], json!("not_found"));

    let subtree = sessions(&http, &reseller).await;
    let agents: Vec<&str> = subtree
        .iter()
        .filter_map(|row| row["userAgent"].as_str())
        .collect();
    assert_eq!(subtree.len(), 4, "{agents:?}");
    for agent in ["a1/first", "a1/second", "member/only", "reseller/own"] {
        assert!(agents.contains(&agent), "{agents:?}");
    }
    assert_eq!(by_agent(&subtree, "reseller/own")["current"], json!(true));
    let all = sessions(&http, &admin).await;
    assert_eq!(all.len(), 5, "{all:?}");
    assert_eq!(by_agent(&all, "admin/own")["current"], json!(true));
    assert_eq!(by_agent(&all, "a1/first")["current"], json!(false));
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn a1_revokes_its_other_session_then_the_current_one_and_each_cookie_dies_with_its_row() {
    let (harness, http) = boot().await;
    let first = login(&http, "a1", "a1/first").await;
    let second = login(&http, "a1", "a1/second").await;
    let own = sessions(&http, &first).await;
    let other = id_of(by_agent(&own, "a1/second"));
    let current = id_of(by_agent(&own, "a1/first"));

    let revoked = call(&http, &first, "auth.session.revoke", json!({ "id": other })).await;
    assert_eq!(revoked.status, 200, "{}", revoked.body);
    assert_eq!(revoked.json()["data"], json!({ "revoked": true }));
    assert_eq!(revoked.set_cookie(), None, "{:?}", revoked.headers);
    let dead = call(&http, &second, "keel.account.list", json!({})).await;
    assert_eq!(dead.status, 401, "{}", dead.body);
    assert!(
        dead.set_cookie()
            .is_some_and(|cookie| cookie.contains("Max-Age=0")),
        "{:?}",
        dead.headers
    );
    let again = call(&http, &first, "auth.session.revoke", json!({ "id": other })).await;
    assert_eq!(again.status, 200, "{}", again.body);
    assert_eq!(again.json()["data"], json!({ "revoked": false }));
    let listed = sessions(&http, &first).await;
    assert!(by_agent(&listed, "a1/second")["revokedAt"].is_string());

    let self_revoked = call(
        &http,
        &first,
        "auth.session.revoke",
        json!({ "id": current }),
    )
    .await;
    assert_eq!(self_revoked.status, 200, "{}", self_revoked.body);
    assert_eq!(
        self_revoked.set_cookie(),
        Some("__Host-keel_session=; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=0"),
        "{:?}",
        self_revoked.headers
    );
    let gone = call(&http, &first, "keel.account.list", json!({})).await;
    assert_eq!(gone.status, 401, "{}", gone.body);
    assert_eq!(
        reasons(&harness).await,
        [
            format!("{KIND_SESSION}:{REASON_REVOKED}"),
            format!("{KIND_SESSION}:{REASON_REVOKED}")
        ]
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn revoking_a_parent_takes_its_su_child_and_revoking_the_child_alone_ends_the_delegation() {
    let (harness, http) = boot().await;
    let a1 = harness.principal(Who::A1).clone();
    let admin = login(&http, "admin", "admin/own").await;
    let entered = call(&http, &admin, "auth.su", json!({ "principal": a1.id })).await;
    assert_eq!(entered.status, 200, "{}", entered.body);
    let as_a1 = sessions(&http, &admin).await;
    let child = as_a1
        .iter()
        .find(|row| row["kind"] == json!(KIND_SU))
        .unwrap_or_else(|| panic!("no su row in {as_a1:?}"));
    assert_eq!(child["actor"], json!(harness.principal(Who::Admin).id));
    assert_eq!(child["current"], json!(false));
    let child_id = id_of(child);
    let listed = call(&http, &admin, "keel.account.list", json!({})).await;
    assert_eq!(listed.json()["meta"]["principal"]["login"], json!("a1"));

    let ended = call(
        &http,
        &admin,
        "auth.session.revoke",
        json!({ "id": child_id }),
    )
    .await;
    assert_eq!(ended.status, 200, "{}", ended.body);
    assert_eq!(ended.set_cookie(), None, "{:?}", ended.headers);
    let back = call(&http, &admin, "keel.account.list", json!({})).await;
    assert_eq!(back.status, 200, "{}", back.body);
    assert_eq!(back.json()["meta"]["principal"]["login"], json!("admin"));
    assert_eq!(back.json()["meta"]["principal"]["actor"], json!(null));

    let entered = call(&http, &admin, "auth.su", json!({ "principal": a1.id })).await;
    assert_eq!(entered.status, 200, "{}", entered.body);
    let own = harness
        .call(Who::Admin, "auth.session.list", json!({}))
        .await;
    let parent = own.data().expect("data")["data"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["userAgent"] == json!("admin/own"))
        .map(id_of)
        .expect("the parent row");
    let revoked = harness
        .call(Who::Admin, "auth.session.revoke", json!({ "id": parent }))
        .await;
    assert_eq!(revoked.data(), Some(&json!({ "revoked": true })));
    let dead = call(&http, &admin, "keel.account.list", json!({})).await;
    assert_eq!(dead.status, 401, "{}", dead.body);
    assert_eq!(
        reasons(&harness).await,
        [
            format!("{KIND_SESSION}:{REASON_REVOKED}"),
            format!("{KIND_SU}:{REASON_REVOKED}"),
            format!("{KIND_SU}:{REASON_REVOKED}"),
        ]
    );

    let admin = login(&http, "admin", "admin/again").await;
    let entered = call(&http, &admin, "auth.su", json!({ "principal": a1.id })).await;
    assert_eq!(entered.status, 200, "{}", entered.body);
    let out = call(&http, &admin, "auth.logout", json!({})).await;
    assert_eq!(out.status, 200, "{}", out.body);
    assert_eq!(
        reasons(&harness).await,
        [
            format!("{KIND_SESSION}:{REASON_LOGOUT}"),
            format!("{KIND_SESSION}:{REASON_REVOKED}"),
            format!("{KIND_SU}:{REASON_LOGOUT}"),
            format!("{KIND_SU}:{REASON_REVOKED}"),
            format!("{KIND_SU}:{REASON_REVOKED}"),
        ]
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn sessions_are_rows_of_their_account_and_the_standard_invariants_hold() {
    let harness = Harness::new().module(&MODULE).boot().await;
    harness
        .assert_reseller_sees_only_subtree("auth.session.list", &SessionFixture)
        .await;
    harness
        .assert_user_cannot_edit_foreign_row("auth.session.revoke", &SessionFixture)
        .await;
    harness.finish().await;
}

#[tokio::test]
async fn the_session_descriptors_match_their_snapshots() {
    let harness = Harness::new().module(&MODULE).boot().await;
    for level in [Level::User, Level::Owner] {
        let names = harness.snapshot_descriptors(level, snapshots_dir());
        assert!(names.contains(&"auth.session.list".to_owned()), "{names:?}");
        assert!(
            names.contains(&"auth.session.revoke".to_owned()),
            "{names:?}"
        );
    }
    let descriptor = harness
        .registry()
        .function("auth.session.list")
        .expect("auth.session.list is registered")
        .descriptor();
    assert_eq!(
        descriptor.access.permission.as_deref(),
        Some("auth.session.read")
    );
    harness.finish().await;
}
