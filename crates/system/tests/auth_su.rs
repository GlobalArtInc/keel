use std::path::{Path, PathBuf};

use keel_sdk::chrono::Duration;
use keel_sdk::prelude::*;
use keel_system::auth::{IDLE_MINUTES, KIND_SESSION, KIND_SU, MODULE, REASON_SU_EXIT};
use keel_testkit::{Answer, Harness, TestHttp, Who, tokio};

const PASSWORD: &str = "correct-horse";
const JSON: (&str, &str) = ("content-type", "application/json");

fn snapshots_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

async fn boot() -> (Harness, TestHttp) {
    let harness = Harness::new().module(&MODULE).boot().await;
    for who in [Who::Admin, Who::Reseller, Who::A1] {
        let id = harness.principal(who).id;
        let set = harness
            .call(
                Who::Owner,
                "auth.password.set",
                json!({ "principal": id, "password": PASSWORD }),
            )
            .await;
        assert!(set.data().is_some(), "{who}: {:?}", set.error());
    }
    let http = harness.http().await;
    (harness, http)
}

async fn login(http: &TestHttp, who: Who) -> String {
    let answer = http
        .request(
            "POST",
            "/api/v1/call/auth.login",
            &[JSON],
            Some(&json!({ "login": who.login(), "password": PASSWORD }).to_string()),
        )
        .await;
    assert_eq!(answer.status, 200, "{who}: {}", answer.body);
    answer
        .session_cookie()
        .unwrap_or_else(|| panic!("no session cookie for {who}: {:?}", answer.headers))
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

async fn boot_with(http: &TestHttp, cookie: &str) -> Answer {
    http.request("GET", "/api/v1/boot", &[("cookie", cookie)], None)
        .await
}

async fn create_principal(harness: &Harness, login: &str, level: Level, account: Id) -> Id {
    let reply = harness
        .call(
            Who::Owner,
            "keel.principal.create",
            json!({ "login": login, "level": level, "account": account }),
        )
        .await;
    let data = reply
        .data()
        .unwrap_or_else(|| panic!("keel.principal.create {login}: {:?}", reply.error()));
    data["id"].as_str().expect("an id").parse().expect("a ulid")
}

fn error_of(answer: &Answer, status: u16, code: &str, key: &str) {
    assert_eq!(answer.status, status, "{}", answer.body);
    let body = answer.json();
    assert_eq!(body["error"]["code"], json!(code), "{}", answer.body);
    assert_eq!(body["error"]["key"], json!(key), "{}", answer.body);
}

fn account_ids(page: &Value) -> Vec<String> {
    page["data"]["data"]
        .as_array()
        .expect("rows")
        .iter()
        .filter_map(|row| row["id"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test]
async fn login_boot_list_su_narrows_the_list_and_fills_the_actor_then_su_exit_returns() {
    let (harness, http) = boot().await;
    let admin = harness.principal(Who::Admin).clone();
    let a1 = harness.principal(Who::A1).clone();
    let cookie = login(&http, Who::Admin).await;

    let booted = boot_with(&http, &cookie).await;
    assert_eq!(booted.status, 200, "{}", booted.body);
    assert_eq!(booted.json()["principal"]["login"], json!("admin"));
    assert_eq!(booted.json()["principal"]["actor"], json!(null));
    let admin_etag = booted.header("etag").expect("an ETag").to_owned();

    let wide = call(&http, &cookie, "keel.account.list", json!({})).await;
    assert_eq!(wide.status, 200, "{}", wide.body);
    assert_eq!(wide.json()["data"]["total"], json!(5));
    assert_eq!(wide.json()["meta"]["principal"]["actor"], json!(null));

    let entered = call(&http, &cookie, "auth.su", json!({ "principal": a1.id })).await;
    assert_eq!(entered.status, 200, "{}", entered.body);
    assert_eq!(entered.set_cookie(), None, "{:?}", entered.headers);
    assert_eq!(
        entered.json()["data"]["principal"],
        json!({
            "id": a1.id,
            "login": "a1",
            "level": "user",
            "account": a1.account_id,
            "actor": { "id": admin.id, "login": "admin", "level": "admin", "via": "su" }
        })
    );

    let narrow = call(&http, &cookie, "keel.account.list", json!({})).await;
    assert_eq!(narrow.status, 200, "{}", narrow.body);
    assert_eq!(narrow.json()["data"]["total"], json!(1));
    assert_eq!(account_ids(&narrow.json()), [a1.account_id.to_string()]);
    assert_eq!(
        narrow.json()["meta"]["principal"],
        json!({
            "id": a1.id,
            "login": "a1",
            "level": "user",
            "account": a1.account_id,
            "actor": { "id": admin.id, "login": "admin", "level": "admin", "via": "su" }
        })
    );

    let delegated = boot_with(&http, &cookie).await;
    assert_eq!(delegated.status, 200, "{}", delegated.body);
    assert_eq!(delegated.json()["principal"]["login"], json!("a1"));
    assert_eq!(
        delegated.json()["principal"]["actor"],
        json!({ "id": admin.id, "login": "admin", "level": "admin", "via": "su" })
    );
    let su_etag = delegated.header("etag").expect("an ETag").to_owned();
    assert_ne!(su_etag, admin_etag);

    let rows = harness
        .database()
        .rows(
            "SELECT kind || ' ' || coalesce(actor_principal_id, '-') || ' ' || (parent_id_hash IS NOT NULL)::text || ' ' || (su_id_hash IS NOT NULL)::text FROM auth_session ORDER BY kind DESC",
        )
        .await;
    assert_eq!(
        rows,
        [
            format!("{KIND_SU} {} true false", admin.id),
            format!("{KIND_SESSION} - false true")
        ]
    );
    let linked = harness
        .database()
        .rows(
            "SELECT count(*)::text FROM auth_session parent JOIN auth_session child ON child.id_hash = parent.su_id_hash AND child.parent_id_hash = parent.id_hash",
        )
        .await;
    assert_eq!(linked, ["1"]);

    let exited = call(&http, &cookie, "auth.su.exit", json!({})).await;
    assert_eq!(exited.status, 200, "{}", exited.body);
    assert_eq!(exited.set_cookie(), None, "{:?}", exited.headers);
    assert_eq!(
        exited.json()["data"]["principal"],
        json!({ "id": admin.id, "login": "admin", "level": "admin", "account": admin.account_id, "actor": null })
    );
    assert_eq!(
        exited.json()["meta"]["principal"]["actor"]["via"],
        json!("su")
    );

    let back = call(&http, &cookie, "keel.account.list", json!({})).await;
    assert_eq!(back.status, 200, "{}", back.body);
    assert_eq!(back.json()["data"]["total"], json!(5));
    assert_eq!(back.json()["meta"]["principal"]["login"], json!("admin"));
    assert_eq!(back.json()["meta"]["principal"]["actor"], json!(null));
    let returned = boot_with(&http, &cookie).await;
    assert_eq!(returned.json()["principal"]["actor"], json!(null));
    assert_eq!(returned.header("etag"), Some(admin_etag.as_str()));

    let after = harness
        .database()
        .rows(
            "SELECT kind || ' ' || coalesce(reason, '-') || ' ' || (su_id_hash IS NOT NULL)::text FROM auth_session ORDER BY kind DESC",
        )
        .await;
    assert_eq!(
        after,
        [
            format!("{KIND_SU} {REASON_SU_EXIT} false"),
            format!("{KIND_SESSION} - false")
        ]
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn su_goes_down_the_ladder_inside_the_scope_only_and_needs_a_session_that_is_not_delegated() {
    let (harness, http) = boot().await;
    let accounts = harness.fixtures().accounts.clone();
    let a1 = harness.principal(Who::A1).id;
    let unrelated = harness.principal(Who::Unrelated).id;
    let reseller = harness.principal(Who::Reseller).id;
    let second_reseller =
        create_principal(&harness, "reseller2", Level::Reseller, accounts.reseller).await;
    let second_admin = create_principal(&harness, "admin2", Level::Admin, accounts.root).await;

    let reseller_cookie = login(&http, Who::Reseller).await;
    let outside = call(
        &http,
        &reseller_cookie,
        "auth.su",
        json!({ "principal": unrelated }),
    )
    .await;
    error_of(&outside, 404, "not_found", "keel.error.not_found");
    let same_level = call(
        &http,
        &reseller_cookie,
        "auth.su",
        json!({ "principal": second_reseller }),
    )
    .await;
    error_of(&same_level, 403, "forbidden", "auth.error.not_below");
    let entered = call(
        &http,
        &reseller_cookie,
        "auth.su",
        json!({ "principal": a1 }),
    )
    .await;
    assert_eq!(entered.status, 200, "{}", entered.body);
    let as_a1 = call(&http, &reseller_cookie, "keel.account.list", json!({})).await;
    assert_eq!(as_a1.json()["data"]["total"], json!(1));
    assert_eq!(
        as_a1.json()["meta"]["principal"]["actor"]["login"],
        json!("reseller")
    );

    let admin_cookie = login(&http, Who::Admin).await;
    let peer = call(
        &http,
        &admin_cookie,
        "auth.su",
        json!({ "principal": second_admin }),
    )
    .await;
    error_of(&peer, 403, "forbidden", "auth.error.not_below");
    let outside_su = call(&http, &admin_cookie, "auth.su.exit", json!({})).await;
    error_of(&outside_su, 422, "validation", "auth.error.not_delegated");
    let entered = call(
        &http,
        &admin_cookie,
        "auth.su",
        json!({ "principal": reseller }),
    )
    .await;
    assert_eq!(entered.status, 200, "{}", entered.body);
    let nested = call(&http, &admin_cookie, "auth.su", json!({ "principal": a1 })).await;
    error_of(&nested, 409, "conflict", "auth.error.su_nested");
    let by_user = http
        .post(Who::A1, "/api/v1/call/auth.su", &json!({ "principal": a1 }))
        .await;
    error_of(&by_user, 403, "forbidden", "keel.error.level");

    let token = harness
        .call(Who::Admin, "auth.token.create", json!({ "name": "cli" }))
        .await;
    let token = token.data().expect("a token")["token"]
        .as_str()
        .expect("the plaintext")
        .to_owned();
    let under_token = http
        .request(
            "POST",
            "/api/v1/call/auth.su",
            &[("authorization", &format!("Bearer {token}")), JSON],
            Some(&json!({ "principal": a1 }).to_string()),
        )
        .await;
    error_of(&under_token, 422, "validation", "auth.error.session_only");

    let revoked = harness
        .database()
        .rows(&format!(
            "UPDATE auth_session SET revoked_at = now(), reason = 'test' WHERE kind = '{KIND_SESSION}' AND principal_id = '{}' RETURNING id_hash",
            harness.principal(Who::Admin).id
        ))
        .await;
    assert_eq!(revoked.len(), 1);
    let gone = call(&http, &admin_cookie, "auth.su.exit", json!({})).await;
    error_of(&gone, 401, "unauthenticated", "keel.error.unauthenticated");
    let listed = call(&http, &admin_cookie, "keel.account.list", json!({})).await;
    assert_eq!(listed.status, 401, "{}", listed.body);
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn inside_su_the_actor_stays_filled_while_the_parent_slides_and_idle_expiry_ends_both() {
    let (harness, http) = boot().await;
    let a1 = harness.principal(Who::A1).id;
    let cookie = login(&http, Who::Admin).await;
    let entered = call(&http, &cookie, "auth.su", json!({ "principal": a1 })).await;
    assert_eq!(entered.status, 200, "{}", entered.body);
    let step = 10;
    for tick in 1..=6 {
        harness.clock().advance(Duration::minutes(step));
        let answer = call(&http, &cookie, "keel.account.list", json!({})).await;
        assert_eq!(
            answer.status,
            200,
            "after {} minutes: {}",
            tick * step,
            answer.body
        );
        assert_eq!(
            answer.json()["meta"]["principal"]["actor"]["login"],
            json!("admin"),
            "after {} minutes",
            tick * step
        );
        assert_eq!(answer.json()["meta"]["principal"]["login"], json!("a1"));
    }
    let idle = harness
        .database()
        .rows("SELECT count(DISTINCT idle_until)::text FROM auth_session")
        .await;
    assert_eq!(idle, ["1"]);
    harness.clock().advance(Duration::minutes(IDLE_MINUTES + 1));
    let expired = call(&http, &cookie, "keel.account.list", json!({})).await;
    error_of(
        &expired,
        401,
        "unauthenticated",
        "keel.error.unauthenticated",
    );
    let fresh = login(&http, Who::Admin).await;
    let as_admin = call(&http, &fresh, "keel.account.list", json!({})).await;
    assert_eq!(as_admin.status, 200, "{}", as_admin.body);
    assert_eq!(as_admin.json()["meta"]["principal"]["actor"], json!(null));
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn the_su_descriptors_match_their_snapshots() {
    let harness = Harness::new().module(&MODULE).boot().await;
    let names = harness.snapshot_descriptors(Level::Reseller, snapshots_dir());
    assert!(names.contains(&"auth.su".to_owned()), "{names:?}");
    assert!(names.contains(&"auth.su.exit".to_owned()), "{names:?}");
    let names = harness.snapshot_descriptors(Level::User, snapshots_dir());
    assert!(!names.contains(&"auth.su".to_owned()), "{names:?}");
    assert!(names.contains(&"auth.su.exit".to_owned()), "{names:?}");
    let descriptor = harness
        .registry()
        .function("auth.su")
        .expect("auth.su is registered")
        .descriptor();
    assert_eq!(descriptor.access.permission.as_deref(), Some("auth.su"));
    harness.finish().await;
}
