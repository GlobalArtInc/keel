use std::path::{Path, PathBuf};

use keel_sdk::chrono::Duration;
use keel_sdk::prelude::*;
use keel_sdk::system::Client;
use keel_system::auth::api::{Lockout, RateLimit};
use keel_system::auth::{
    GUEST_PER_MINUTE, LOCKOUT_MINUTES, LOCKOUT_THRESHOLD, MODULE, TOTP_STEP_SECONDS,
    decode_totp_secret, totp_code, totp_step,
};
use keel_testkit::{
    CallOptions, ErrorCode, Harness, PRINCIPAL_HEADER, Reply, TestHttp, Who, tokio,
};

const PASSWORD: &str = "correct-horse";
const ATTACKER: &str = "10.1.1.1";
const LOCAL: &str = "127.0.0.1";

fn snapshots_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

async fn boot() -> Harness {
    let harness = Harness::new().module(&MODULE).boot().await;
    for who in [Who::Admin, Who::A1] {
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
    harness
}

fn from(ip: &str) -> CallOptions {
    CallOptions {
        client: Some(Client {
            ip: Some(ip.parse().expect("an address")),
            user_agent: Some("keel-system-tests/1".into()),
            session: None,
        }),
        ..CallOptions::default()
    }
}

async fn login_from(harness: &Harness, ip: &str, login: &str, password: &str) -> Reply {
    harness
        .call_with(
            harness.principal(Who::Guest),
            "auth.login",
            json!({ "login": login, "password": password }),
            from(ip),
        )
        .await
}

async fn verify_from(harness: &Harness, ip: &str, challenge: &str, code: &str) -> Reply {
    harness
        .call_with(
            harness.principal(Who::Guest),
            "auth.totp.verify",
            json!({ "challenge": challenge, "code": code }),
            from(ip),
        )
        .await
}

fn refused(reply: &Reply, status: u16, code: ErrorCode, key: &str) {
    let error = reply
        .error()
        .unwrap_or_else(|| panic!("an error was expected, got {:?}", reply.envelope));
    assert_eq!(reply.status(), status, "{error:?}");
    assert_eq!(error.code, code, "{error:?}");
    assert_eq!(error.key, key, "{error:?}");
}

fn state_of(reply: &Reply) -> String {
    let data = reply
        .data()
        .unwrap_or_else(|| panic!("a login was expected, got {:?}", reply.error()));
    data["state"].as_str().expect("a state").to_owned()
}

fn rate_limit(harness: &Harness) -> Arc<RateLimit> {
    harness
        .registry()
        .services()
        .get::<RateLimit>()
        .expect("the auth module provides its rate limit")
}

fn lockout(harness: &Harness) -> Arc<Lockout> {
    harness
        .registry()
        .services()
        .get::<Lockout>()
        .expect("the auth module provides its lockout")
}

#[tokio::test]
async fn the_thirty_first_guest_call_from_one_address_within_a_minute_is_rate_limited_and_the_bucket_leaves_with_its_window()
 {
    let harness = boot().await;
    let other = Harness::new().module(&MODULE).boot().await;
    for attempt in 1..=GUEST_PER_MINUTE {
        let reply = login_from(&harness, "10.0.0.1", "a1", PASSWORD).await;
        assert_eq!(
            reply.status(),
            200,
            "attempt {attempt}: {:?}",
            reply.error()
        );
    }
    let limited = login_from(&harness, "10.0.0.1", "a1", PASSWORD).await;
    refused(
        &limited,
        429,
        ErrorCode::RateLimited,
        "auth.error.rate_limited",
    );
    let elsewhere = login_from(&harness, "10.0.0.2", "a1", PASSWORD).await;
    assert_eq!(elsewhere.status(), 200, "{:?}", elsewhere.error());
    let separate = login_from(&other, "10.0.0.1", "nobody", PASSWORD).await;
    refused(
        &separate,
        401,
        ErrorCode::Unauthenticated,
        "auth.error.credentials",
    );
    assert_eq!(rate_limit(&other).len(), 1);

    let authenticated = harness
        .call_with(
            harness.principal(Who::A1),
            "keel.account.list",
            json!({}),
            from("10.0.0.1"),
        )
        .await;
    assert_eq!(authenticated.status(), 200, "{:?}", authenticated.error());
    let still = login_from(&harness, "10.0.0.1", "a1", PASSWORD).await;
    refused(
        &still,
        429,
        ErrorCode::RateLimited,
        "auth.error.rate_limited",
    );
    assert_eq!(rate_limit(&harness).len(), 2);

    harness.clock().advance(Duration::minutes(1));
    let again = login_from(&harness, "10.0.0.1", "a1", PASSWORD).await;
    assert_eq!(again.status(), 200, "{:?}", again.error());

    harness.clock().advance(Duration::minutes(1));
    let probe = login_from(&harness, "192.0.2.1", "a1", PASSWORD).await;
    assert_eq!(probe.status(), 200, "{:?}", probe.error());
    assert_eq!(rate_limit(&harness).len(), 1);
    other.finish().await;
    harness.finish().await;
}

#[tokio::test]
async fn five_wrong_passwords_lock_the_login_from_that_address_only_until_the_window_passes_or_the_admin_unlocks()
 {
    let harness = boot().await;
    for attempt in 1..=LOCKOUT_THRESHOLD {
        let reply = login_from(&harness, ATTACKER, "a1", "wrong-password").await;
        refused(
            &reply,
            401,
            ErrorCode::Unauthenticated,
            "auth.error.credentials",
        );
        assert!(attempt <= LOCKOUT_THRESHOLD);
    }
    let locked = login_from(&harness, ATTACKER, "a1", PASSWORD).await;
    refused(&locked, 403, ErrorCode::Forbidden, "auth.error.locked");
    let victim = login_from(&harness, LOCAL, "a1", PASSWORD).await;
    assert_eq!(victim.status(), 200, "{:?}", victim.error());
    assert_eq!(state_of(&victim), "authenticated");

    for _ in 0..LOCKOUT_THRESHOLD {
        let reply = login_from(&harness, ATTACKER, "nobody", PASSWORD).await;
        refused(
            &reply,
            401,
            ErrorCode::Unauthenticated,
            "auth.error.credentials",
        );
    }
    let unknown = login_from(&harness, ATTACKER, "nobody", PASSWORD).await;
    refused(&unknown, 403, ErrorCode::Forbidden, "auth.error.locked");
    assert_eq!(lockout(&harness).len(), 2);

    harness.clock().advance(Duration::minutes(LOCKOUT_MINUTES));
    let released = login_from(&harness, ATTACKER, "a1", PASSWORD).await;
    assert_eq!(released.status(), 200, "{:?}", released.error());
    assert_eq!(lockout(&harness).len(), 0);

    for _ in 0..LOCKOUT_THRESHOLD {
        login_from(&harness, ATTACKER, "a1", "wrong-password").await;
    }
    refused(
        &login_from(&harness, ATTACKER, "a1", PASSWORD).await,
        403,
        ErrorCode::Forbidden,
        "auth.error.locked",
    );
    let not_admin = harness
        .call(Who::Reseller, "auth.unlock", json!({ "login": "a1" }))
        .await;
    refused(&not_admin, 403, ErrorCode::Forbidden, "keel.error.level");
    let unlocked = harness
        .call(Who::Admin, "auth.unlock", json!({ "login": "a1" }))
        .await;
    assert_eq!(
        unlocked.data(),
        Some(&json!({ "login": "a1", "cleared": 1 })),
        "{:?}",
        unlocked.error()
    );
    let admitted = login_from(&harness, ATTACKER, "a1", PASSWORD).await;
    assert_eq!(admitted.status(), 200, "{:?}", admitted.error());
    harness.finish().await;
}

async fn enroll_a1(harness: &Harness) -> Vec<u8> {
    let enrolled = harness.call(Who::A1, "auth.totp.enroll", json!({})).await;
    let secret = enrolled
        .data()
        .and_then(|data| data["secret"].as_str())
        .map(|secret| decode_totp_secret(secret).expect("base32"))
        .unwrap_or_else(|| panic!("auth.totp.enroll failed: {:?}", enrolled.error()));
    let code = totp_code(&secret, totp_step(harness.clock().now())).expect("a code");
    let confirmed = harness
        .call(Who::A1, "auth.totp.confirm", json!({ "code": code }))
        .await;
    assert!(confirmed.data().is_some(), "{:?}", confirmed.error());
    secret
}

fn challenge_of(reply: &Reply) -> String {
    assert_eq!(state_of(reply), "totp");
    reply.data().expect("data")["challenge"]
        .as_str()
        .expect("a challenge")
        .to_owned()
}

#[tokio::test]
async fn five_wrong_codes_lock_the_challenge_pair_for_the_address_and_the_code_passes_from_another()
{
    let harness = boot().await;
    let secret = enroll_a1(&harness).await;

    let challenged = login_from(&harness, ATTACKER, "a1", PASSWORD).await;
    let challenge = challenge_of(&challenged);
    for _ in 0..LOCKOUT_THRESHOLD {
        let wrong = verify_from(&harness, ATTACKER, &challenge, "000000").await;
        refused(&wrong, 401, ErrorCode::Unauthenticated, "auth.error.code");
    }
    harness
        .clock()
        .advance(Duration::seconds(TOTP_STEP_SECONDS));
    let right = totp_code(&secret, totp_step(harness.clock().now())).expect("a code");
    let locked = verify_from(&harness, ATTACKER, &challenge, &right).await;
    refused(&locked, 403, ErrorCode::Forbidden, "auth.error.locked");
    let relogin = login_from(&harness, ATTACKER, "a1", PASSWORD).await;
    refused(&relogin, 403, ErrorCode::Forbidden, "auth.error.locked");

    let elsewhere = login_from(&harness, LOCAL, "a1", PASSWORD).await;
    let challenge = challenge_of(&elsewhere);
    let accepted = verify_from(&harness, LOCAL, &challenge, &right).await;
    assert_eq!(accepted.status(), 200, "{:?}", accepted.error());
    assert_eq!(state_of(&accepted), "authenticated");

    harness.clock().advance(Duration::minutes(LOCKOUT_MINUTES));
    let released = login_from(&harness, ATTACKER, "a1", PASSWORD).await;
    assert_eq!(state_of(&released), "totp");
    assert_eq!(lockout(&harness).len(), 0);
    harness.finish().await;
}

#[tokio::test]
async fn a_new_challenge_does_not_reset_the_wrong_code_count_of_the_pair() {
    let harness = boot().await;
    let secret = enroll_a1(&harness).await;

    let first = login_from(&harness, ATTACKER, "a1", PASSWORD).await;
    let challenge = challenge_of(&first);
    for _ in 0..LOCKOUT_THRESHOLD - 1 {
        let wrong = verify_from(&harness, ATTACKER, &challenge, "000000").await;
        refused(&wrong, 401, ErrorCode::Unauthenticated, "auth.error.code");
    }

    let second = login_from(&harness, ATTACKER, "a1", PASSWORD).await;
    let challenge = challenge_of(&second);
    let wrong = verify_from(&harness, ATTACKER, &challenge, "000000").await;
    refused(&wrong, 401, ErrorCode::Unauthenticated, "auth.error.code");

    harness
        .clock()
        .advance(Duration::seconds(TOTP_STEP_SECONDS));
    let right = totp_code(&secret, totp_step(harness.clock().now())).expect("a code");
    let locked = verify_from(&harness, ATTACKER, &challenge, &right).await;
    refused(&locked, 403, ErrorCode::Forbidden, "auth.error.locked");
    let relogin = login_from(&harness, ATTACKER, "a1", PASSWORD).await;
    refused(&relogin, 403, ErrorCode::Forbidden, "auth.error.locked");

    let elsewhere = login_from(&harness, LOCAL, "a1", PASSWORD).await;
    let challenge = challenge_of(&elsewhere);
    let accepted = verify_from(&harness, LOCAL, &challenge, &right).await;
    assert_eq!(accepted.status(), 200, "{:?}", accepted.error());
    assert_eq!(state_of(&accepted), "authenticated");
    assert_eq!(lockout(&harness).len(), 1);
    harness.finish().await;
}

async fn rule(harness: &Harness, action: &str, cidr: &str, level: &str) -> String {
    let reply = harness
        .call(
            Who::Admin,
            "auth.ip_rule.create",
            json!({ "action": action, "cidr": cidr, "level": level, "comment": "test" }),
        )
        .await;
    let data = reply
        .data()
        .unwrap_or_else(|| panic!("auth.ip_rule.create failed: {:?}", reply.error()));
    assert_eq!(data["action"], json!(action));
    assert_eq!(data["cidr"], json!(cidr));
    assert_eq!(data["level"], json!(level));
    data["id"].as_str().expect("an id").to_owned()
}

#[tokio::test]
async fn ip_rules_refuse_by_address_and_level_take_effect_at_once_and_never_apply_to_the_socket() {
    let harness = boot().await;
    let http = harness.http().await;
    let socket = harness.socket().await;
    let deny = rule(&harness, "deny", "127.0.0.1/32", "guest").await;

    let login = http
        .post(
            Who::Guest,
            "/api/v1/call/auth.login",
            &json!({ "login": "a1", "password": PASSWORD }),
        )
        .await;
    assert_eq!(login.status, 403, "{}", login.body);
    assert_eq!(login.json()["error"]["key"], json!("auth.error.ip_denied"));
    let listed = http
        .post(Who::A1, "/api/v1/call/keel.account.list", &json!({}))
        .await;
    assert_eq!(listed.status, 403, "{}", listed.body);
    assert_eq!(listed.json()["error"]["code"], json!("forbidden"));
    assert_eq!(listed.json()["error"]["key"], json!("auth.error.ip_denied"));
    let over_socket = socket.call("keel.account.list", json!({})).await;
    assert_eq!(over_socket.status(), 200, "{over_socket:?}");
    let through_dispatcher = login_from(&harness, LOCAL, "a1", PASSWORD).await;
    refused(
        &through_dispatcher,
        403,
        ErrorCode::Forbidden,
        "auth.error.ip_denied",
    );

    let rules = harness
        .call(Who::Admin, "auth.ip_rule.list", json!({}))
        .await;
    let page = rules
        .data()
        .unwrap_or_else(|| panic!("auth.ip_rule.list failed: {:?}", rules.error()));
    assert_eq!(page["total"], json!(1));
    assert_eq!(page["data"][0]["id"], json!(deny));
    assert_eq!(page["data"][0]["comment"], json!("test"));
    let as_reseller = harness
        .call(Who::Reseller, "auth.ip_rule.list", json!({}))
        .await;
    refused(&as_reseller, 403, ErrorCode::Forbidden, "keel.error.level");

    let deleted = harness
        .call(Who::Admin, "auth.ip_rule.delete", json!({ "id": deny }))
        .await;
    assert_eq!(deleted.data(), Some(&json!({ "deleted": true })));
    let gone = harness
        .call(Who::Admin, "auth.ip_rule.delete", json!({ "id": deny }))
        .await;
    refused(&gone, 404, ErrorCode::NotFound, "keel.error.not_found");
    let listed = http
        .post(Who::A1, "/api/v1/call/keel.account.list", &json!({}))
        .await;
    assert_eq!(listed.status, 200, "{}", listed.body);

    rule(&harness, "allow", "10.0.0.0/8", "admin").await;
    let admin = http
        .post(Who::Admin, "/api/v1/call/keel.account.list", &json!({}))
        .await;
    assert_eq!(admin.status, 403, "{}", admin.body);
    assert_eq!(admin.json()["error"]["key"], json!("auth.error.ip_denied"));
    let user = http
        .post(Who::A1, "/api/v1/call/keel.account.list", &json!({}))
        .await;
    assert_eq!(user.status, 200, "{}", user.body);
    let inside = harness
        .call_with(
            harness.principal(Who::Admin),
            "keel.account.list",
            json!({}),
            from("10.2.3.4"),
        )
        .await;
    assert_eq!(inside.status(), 200, "{:?}", inside.error());
    let outside = harness
        .call_with(
            harness.principal(Who::Owner),
            "keel.account.list",
            json!({}),
            from("192.0.2.1"),
        )
        .await;
    refused(&outside, 403, ErrorCode::Forbidden, "auth.error.ip_denied");
    let over_socket = socket.call("keel.account.list", json!({})).await;
    assert_eq!(over_socket.status(), 200, "{over_socket:?}");

    let bad_net = harness
        .call(
            Who::Admin,
            "auth.ip_rule.create",
            json!({ "action": "deny", "cidr": "10.0.0.0", "level": "guest" }),
        )
        .await;
    assert_eq!(bad_net.status(), 422, "{:?}", bad_net.envelope);
    assert_eq!(
        bad_net
            .error()
            .and_then(|error| error.fields.first().map(|field| field.path.clone())),
        Some("cidr".to_owned())
    );
    socket.stop().await.expect("the socket stops");
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn the_ip_rule_and_unlock_descriptors_match_their_snapshots() {
    let harness = Harness::new().module(&MODULE).boot().await;
    let names = harness.snapshot_descriptors(Level::Admin, snapshots_dir());
    for name in [
        "auth.ip_rule.list",
        "auth.ip_rule.create",
        "auth.ip_rule.delete",
        "auth.unlock",
    ] {
        assert!(names.contains(&name.to_owned()), "{names:?}");
    }
    let names = harness.snapshot_descriptors(Level::Reseller, snapshots_dir());
    assert!(
        !names.contains(&"auth.ip_rule.list".to_owned()),
        "{names:?}"
    );
    assert!(!names.contains(&"auth.unlock".to_owned()), "{names:?}");
    harness.finish().await;
}

#[tokio::test]
async fn an_ip_rule_a_lockout_and_the_audit_record_see_the_forwarded_address_behind_a_trusted_proxy()
 {
    let builder = Harness::new().module(&MODULE);
    #[cfg(feature = "audit")]
    let builder = builder.module(&keel_system::audit::MODULE);
    let harness = builder.boot().await;
    for who in [Who::Admin, Who::A1] {
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
    let behind = harness.http_behind(&["127.0.0.0/8"]).await;
    let direct = harness.http().await;
    let forwarded = ("x-forwarded-for", "203.0.113.9, 127.0.0.2");
    let login_body = json!({ "login": "a1", "password": PASSWORD }).to_string();
    async fn login(
        http: &TestHttp,
        headers: Vec<(&'static str, &'static str)>,
        body: String,
    ) -> keel_testkit::Answer {
        let mut all = vec![
            (PRINCIPAL_HEADER, Who::Guest.login()),
            ("content-type", "application/json"),
        ];
        all.extend(headers);
        http.request("POST", "/api/v1/call/auth.login", &all, Some(&body))
            .await
    }

    rule(&harness, "deny", "203.0.113.0/24", "guest").await;
    let denied = login(&behind, vec![forwarded], login_body.clone()).await;
    assert_eq!(denied.status, 403, "{}", denied.body);
    assert_eq!(denied.json()["error"]["key"], json!("auth.error.ip_denied"));
    let ignored = login(&direct, vec![forwarded], login_body.clone()).await;
    assert_eq!(ignored.status, 200, "{}", ignored.body);
    let listed = behind
        .post(Who::A1, "/api/v1/call/keel.account.list", &json!({}))
        .await;
    assert_eq!(listed.status, 200, "{}", listed.body);
    let denied_list = behind
        .request(
            "POST",
            "/api/v1/call/keel.account.list",
            &[
                (PRINCIPAL_HEADER, Who::A1.login()),
                ("content-type", "application/json"),
                forwarded,
            ],
            Some("{}"),
        )
        .await;
    assert_eq!(denied_list.status, 403, "{}", denied_list.body);
    assert_eq!(
        denied_list.json()["error"]["key"],
        json!("auth.error.ip_denied")
    );
    let rules = harness
        .call(Who::Admin, "auth.ip_rule.list", json!({}))
        .await;
    let id = rules.data().expect("a page")["data"][0]["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let deleted = harness
        .call(Who::Admin, "auth.ip_rule.delete", json!({ "id": id }))
        .await;
    assert_eq!(deleted.data(), Some(&json!({ "deleted": true })));

    let wrong = json!({ "login": "a1", "password": "wrong-password" }).to_string();
    for _ in 0..LOCKOUT_THRESHOLD {
        let reply = login(&behind, vec![forwarded], wrong.clone()).await;
        assert_eq!(reply.status, 401, "{}", reply.body);
    }
    let locked = login(&behind, vec![forwarded], login_body.clone()).await;
    assert_eq!(locked.status, 403, "{}", locked.body);
    assert_eq!(locked.json()["error"]["key"], json!("auth.error.locked"));
    let other_address = login(
        &behind,
        vec![("x-forwarded-for", "198.51.100.7")],
        login_body.clone(),
    )
    .await;
    assert_eq!(other_address.status, 200, "{}", other_address.body);
    let peer_itself = login(&direct, vec![forwarded], login_body.clone()).await;
    assert_eq!(peer_itself.status, 200, "{}", peer_itself.body);
    assert_eq!(lockout(&harness).len(), 1);

    #[cfg(feature = "audit")]
    {
        harness
            .registry()
            .services()
            .get::<keel_system::audit::api::Flush>()
            .expect("the audit module provides Flush")
            .flush()
            .await
            .expect("the worker answers the flush");
        let recorded = harness
            .database()
            .rows("SELECT coalesce(ip, '-') || ' ' || status || ' ' || coalesce(error_key, '-') FROM audit_call WHERE function = 'auth.login' AND via = 'http' ORDER BY at, id")
            .await;
        assert!(
            recorded
                .iter()
                .filter(|row| row.starts_with("203.0.113.9 "))
                .count()
                >= LOCKOUT_THRESHOLD as usize + 2,
            "the forwarded address is recorded: {recorded:?}"
        );
        assert!(
            recorded.contains(&"203.0.113.9 failed auth.error.locked".to_owned()),
            "{recorded:?}"
        );
        assert!(
            recorded.contains(&"198.51.100.7 committed -".to_owned()),
            "{recorded:?}"
        );
        assert!(
            recorded
                .iter()
                .any(|row| row.starts_with("127.0.0.1 committed")),
            "the untrusted peer is recorded as itself: {recorded:?}"
        );
    }
    direct.stop().await.expect("the server stops");
    behind.stop().await.expect("the server stops");
    harness.finish().await;
}
