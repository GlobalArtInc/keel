use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use futures::future::join_all;
use keel_sdk::chrono::Duration;
use keel_sdk::prelude::*;
use keel_sdk::serde_json;
use keel_sdk::system::{AuditEvent, AuditSink, SecretCipher};
use keel_system::auth::{
    CHALLENGE_ATTEMPTS, CHALLENGE_MINUTES, MODULE, REASON_REPLACED, RECOVERY_CODE_LEN,
    RECOVERY_CODES, TOKEN_PREFIX, TOTP_DIGITS, TOTP_ISSUER, TOTP_STEP_SECONDS, decode_totp_secret,
    totp_code, totp_step,
};
use keel_testkit::{Answer, Harness, TestHttp, Who, tokio};

const PASSWORD: &str = "correct-horse";
const JSON: (&str, &str) = ("content-type", "application/json");
const ALPHABET: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

fn snapshots_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

async fn boot_with(extra: Option<&'static ModuleDef>) -> (Harness, TestHttp) {
    let mut builder = Harness::new().module(&MODULE);
    if let Some(module) = extra {
        builder = builder.module(module);
    }
    let harness = builder.boot().await;
    for who in [Who::A1, Who::A2] {
        let id = harness.principal(who).id;
        let set = harness
            .call(
                Who::Admin,
                "auth.password.set",
                json!({ "principal": id, "password": PASSWORD }),
            )
            .await;
        assert!(set.data().is_some(), "{:?}", set.error());
    }
    let http = harness.http().await;
    (harness, http)
}

async fn boot() -> (Harness, TestHttp) {
    boot_with(None).await
}

struct Factor {
    secret: Vec<u8>,
    recovery_codes: Vec<String>,
}

fn code_at(harness: &Harness, secret: &[u8], steps: i64) -> String {
    totp_code(secret, totp_step(harness.clock().now()) + steps).expect("a code")
}

async fn enroll(harness: &Harness, who: Who) -> (Vec<u8>, String) {
    let reply = harness.call(who, "auth.totp.enroll", json!({})).await;
    let data = reply
        .data()
        .unwrap_or_else(|| panic!("auth.totp.enroll by {who} failed: {:?}", reply.error()));
    let secret = data["secret"].as_str().expect("a secret").to_owned();
    let uri = data["uri"].as_str().expect("a uri").to_owned();
    assert_eq!(secret.len(), 32, "{secret}");
    assert!(secret.chars().all(|c| ALPHABET.contains(c)), "{secret}");
    assert_eq!(
        uri,
        format!(
            "otpauth://totp/{TOTP_ISSUER}:{}?secret={secret}&issuer={TOTP_ISSUER}&algorithm=SHA1&digits={TOTP_DIGITS}&period={TOTP_STEP_SECONDS}",
            who.login()
        )
    );
    (decode_totp_secret(&secret).expect("base32"), secret)
}

async fn enroll_and_confirm(harness: &Harness, who: Who) -> Factor {
    let (secret, _) = enroll(harness, who).await;
    let code = code_at(harness, &secret, 0);
    let reply = harness
        .call(who, "auth.totp.confirm", json!({ "code": code }))
        .await;
    let data = reply
        .data()
        .unwrap_or_else(|| panic!("auth.totp.confirm by {who} failed: {:?}", reply.error()));
    let recovery_codes: Vec<String> = data["recoveryCodes"]
        .as_array()
        .expect("the recovery codes")
        .iter()
        .map(|code| code.as_str().expect("a code").to_owned())
        .collect();
    Factor {
        secret,
        recovery_codes,
    }
}

async fn login(http: &TestHttp, who: Who, extra: &[(&str, &str)]) -> Answer {
    let mut headers = vec![JSON];
    headers.extend_from_slice(extra);
    http.request(
        "POST",
        "/api/v1/call/auth.login",
        &headers,
        Some(&json!({ "login": who.login(), "password": PASSWORD }).to_string()),
    )
    .await
}

async fn challenge_of(http: &TestHttp, who: Who) -> String {
    let answer = login(http, who, &[]).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    let body = answer.json();
    assert_eq!(body["data"]["state"], json!("totp"), "{}", answer.body);
    assert_eq!(body["data"].get("principal"), None, "{}", answer.body);
    assert_eq!(answer.set_cookie(), None);
    let challenge = body["data"]["challenge"]
        .as_str()
        .expect("a challenge")
        .to_owned();
    assert_eq!(challenge.len(), 64, "{challenge}");
    challenge
}

async fn verify(http: &TestHttp, challenge: &str, code: &str, extra: &[(&str, &str)]) -> Answer {
    let mut headers = vec![JSON];
    headers.extend_from_slice(extra);
    http.request(
        "POST",
        "/api/v1/call/auth.totp.verify",
        &headers,
        Some(&json!({ "challenge": challenge, "code": code }).to_string()),
    )
    .await
}

fn error_of(answer: &Answer, status: u16, code: &str, key: &str) {
    assert_eq!(answer.status, status, "{}", answer.body);
    let body = answer.json();
    assert_eq!(body["error"]["code"], json!(code), "{}", answer.body);
    assert_eq!(body["error"]["key"], json!(key), "{}", answer.body);
    assert_eq!(answer.set_cookie(), None);
}

fn field_error_of(answer: &Answer, path: &str, key: &str) {
    assert_eq!(answer.status, 422, "{}", answer.body);
    let body = answer.json();
    assert_eq!(
        body["error"]["code"],
        json!("validation"),
        "{}",
        answer.body
    );
    assert_eq!(
        body["error"]["fields"][0]["path"],
        json!(path),
        "{}",
        answer.body
    );
    assert_eq!(
        body["error"]["fields"][0]["key"],
        json!(key),
        "{}",
        answer.body
    );
    assert_eq!(answer.set_cookie(), None);
}

fn field_key(reply: &keel_testkit::Reply) -> Option<String> {
    let error = reply.error()?;
    error
        .fields
        .first()
        .and_then(|field| field.key.clone())
        .or_else(|| Some(error.key.clone()))
}

async fn unlock(harness: &Harness, who: Who) {
    let reply = harness
        .call(Who::Admin, "auth.unlock", json!({ "login": who.login() }))
        .await;
    assert!(
        reply.data().is_some(),
        "auth.unlock {who}: {:?}",
        reply.error()
    );
}

fn cookie_of(answer: &Answer) -> String {
    answer
        .session_cookie()
        .unwrap_or_else(|| panic!("no session cookie: {:?}", answer.headers))
}

async fn boot_totp(http: &TestHttp, cookie: &str) -> Value {
    let booted = http
        .request("GET", "/api/v1/boot", &[("cookie", cookie)], None)
        .await;
    assert_eq!(booted.status, 200, "{}", booted.body);
    booted.json()["principal"]["totp"].clone()
}

async fn last_step(harness: &Harness) -> Vec<String> {
    harness
        .database()
        .rows("SELECT coalesce(last_step::text, 'null') FROM auth_totp")
        .await
}

#[tokio::test]
async fn a1_enrols_confirms_and_logs_in_through_a_challenge_with_the_step_moving_forward_only() {
    let (harness, http) = boot().await;
    let a1 = harness.principal(Who::A1).clone();
    let factor = enroll_and_confirm(&harness, Who::A1).await;
    let step = totp_step(harness.clock().now());
    assert_eq!(last_step(&harness).await, [step.to_string()]);

    let stored = harness
        .database()
        .rows("SELECT secret FROM auth_totp")
        .await;
    let encoded = base32_of(&factor.secret);
    assert!(!stored[0].contains(&encoded), "{}", stored[0]);
    let cipher = harness
        .registry()
        .services()
        .get::<dyn SecretCipher>()
        .expect("the kernel provides the cipher");
    assert_eq!(cipher.open(&stored[0]).expect("opens"), factor.secret);

    let challenge = challenge_of(&http, Who::A1).await;
    let hashes = harness
        .database()
        .rows("SELECT challenge_hash FROM auth_login_challenge")
        .await;
    assert_eq!(hashes.len(), 1);
    assert_ne!(hashes[0], challenge);
    assert_eq!(hashes[0].len(), 64);

    let same_step = verify(
        &http,
        &challenge,
        &code_at(&harness, &factor.secret, 0),
        &[],
    )
    .await;
    error_of(&same_step, 401, "unauthenticated", "auth.error.code");
    let wrong = verify(&http, &challenge, "000000", &[]).await;
    error_of(&wrong, 401, "unauthenticated", "auth.error.code");
    assert_eq!(
        harness
            .database()
            .rows("SELECT count(*)::text FROM auth_session")
            .await,
        ["0"]
    );

    harness
        .clock()
        .advance(Duration::seconds(TOTP_STEP_SECONDS));
    let right = verify(
        &http,
        &challenge,
        &code_at(&harness, &factor.secret, 0),
        &[],
    )
    .await;
    assert_eq!(right.status, 200, "{}", right.body);
    let body = right.json();
    assert_eq!(body["data"]["state"], json!("authenticated"));
    assert_eq!(body["data"]["principal"]["id"], json!(a1.id.to_string()));
    assert_eq!(body["data"].get("challenge"), None, "{}", right.body);
    let cookie = cookie_of(&right);
    assert!(cookie.contains(TOKEN_PREFIX), "{cookie}");
    assert_eq!(last_step(&harness).await, [(step + 1).to_string()]);
    assert_eq!(boot_totp(&http, &cookie).await, json!(true));
    assert_eq!(
        harness
            .database()
            .rows("SELECT coalesce(used_at::text, 'open') FROM auth_login_challenge")
            .await
            .iter()
            .filter(|used| *used == "open")
            .count(),
        0
    );
    let spent = verify(
        &http,
        &challenge,
        &code_at(&harness, &factor.secret, 0),
        &[],
    )
    .await;
    error_of(&spent, 401, "unauthenticated", "auth.error.challenge");

    harness
        .clock()
        .advance(Duration::seconds(TOTP_STEP_SECONDS));
    let current = challenge_of(&http, Who::A1).await;
    let accepted = verify(&http, &current, &code_at(&harness, &factor.secret, 0), &[]).await;
    assert_eq!(accepted.status, 200, "{}", accepted.body);
    assert_eq!(last_step(&harness).await, [(step + 2).to_string()]);
    let previous = challenge_of(&http, Who::A1).await;
    let behind = verify(
        &http,
        &previous,
        &code_at(&harness, &factor.secret, -1),
        &[],
    )
    .await;
    error_of(&behind, 401, "unauthenticated", "auth.error.code");
    let ahead = verify(&http, &previous, &code_at(&harness, &factor.secret, 1), &[]).await;
    assert_eq!(ahead.status, 200, "{}", ahead.body);
    assert_eq!(last_step(&harness).await, [(step + 3).to_string()]);
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

fn base32_of(secret: &[u8]) -> String {
    let mut bits = String::new();
    for byte in secret {
        bits.push_str(&format!("{byte:08b}"));
    }
    bits.as_bytes()
        .chunks(5)
        .map(|chunk| {
            let index = usize::from_str_radix(std::str::from_utf8(chunk).expect("ascii"), 2)
                .expect("binary")
                << (5 - chunk.len());
            ALPHABET.as_bytes()[index] as char
        })
        .collect()
}

#[tokio::test]
async fn the_sixth_wrong_code_kills_the_challenge_and_an_expired_or_unknown_one_is_refused() {
    let (harness, http) = boot().await;
    let factor = enroll_and_confirm(&harness, Who::A1).await;
    harness
        .clock()
        .advance(Duration::seconds(TOTP_STEP_SECONDS));
    let challenge = challenge_of(&http, Who::A1).await;
    for _ in 0..CHALLENGE_ATTEMPTS {
        let wrong = verify(&http, &challenge, "000000", &[]).await;
        error_of(&wrong, 401, "unauthenticated", "auth.error.code");
    }
    let locked = verify(&http, &challenge, "000000", &[]).await;
    error_of(&locked, 403, "forbidden", "auth.error.locked");
    unlock(&harness, Who::A1).await;
    let sixth = verify(&http, &challenge, "000000", &[]).await;
    error_of(&sixth, 401, "unauthenticated", "auth.error.challenge");
    let killed = verify(
        &http,
        &challenge,
        &code_at(&harness, &factor.secret, 0),
        &[],
    )
    .await;
    error_of(&killed, 401, "unauthenticated", "auth.error.challenge");

    let fresh = challenge_of(&http, Who::A1).await;
    harness
        .clock()
        .advance(Duration::minutes(CHALLENGE_MINUTES) + Duration::seconds(1));
    let expired = verify(&http, &fresh, &code_at(&harness, &factor.secret, 0), &[]).await;
    error_of(&expired, 401, "unauthenticated", "auth.error.challenge");
    let unknown = verify(
        &http,
        &"0".repeat(64),
        &code_at(&harness, &factor.secret, 0),
        &[],
    )
    .await;
    error_of(&unknown, 401, "unauthenticated", "auth.error.challenge");

    let socket = harness.socket().await;
    let over_socket = socket
        .call(
            "auth.totp.verify",
            json!({ "challenge": fresh, "code": "000000" }),
        )
        .await;
    let over_socket = serde_json::to_value(&over_socket).expect("an envelope");
    assert_eq!(over_socket["error"]["key"], json!("auth.error.http_only"));
    socket.stop().await.expect("the socket stops");
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn a_recovery_code_logs_in_once_without_moving_the_step_and_is_stored_as_its_hash() {
    let (harness, http) = boot().await;
    let factor = enroll_and_confirm(&harness, Who::A1).await;
    assert_eq!(factor.recovery_codes.len(), RECOVERY_CODES);
    for code in &factor.recovery_codes {
        assert_eq!(code.len(), RECOVERY_CODE_LEN, "{code}");
        assert!(code.chars().all(|c| ALPHABET.contains(c)), "{code}");
    }
    let hashes = harness
        .database()
        .rows("SELECT code_hash FROM auth_recovery_code ORDER BY code_hash")
        .await;
    assert_eq!(hashes.len(), RECOVERY_CODES);
    let mut expected: Vec<String> = factor
        .recovery_codes
        .iter()
        .map(|code| sha256_hex(code))
        .collect();
    expected.sort();
    assert_eq!(hashes, expected);
    let before = last_step(&harness).await;

    let challenge = challenge_of(&http, Who::A1).await;
    let recovered = verify(&http, &challenge, &factor.recovery_codes[3], &[]).await;
    assert_eq!(recovered.status, 200, "{}", recovered.body);
    assert!(recovered.session_cookie().is_some());
    assert_eq!(last_step(&harness).await, before);
    assert_eq!(
        harness
            .database()
            .rows("SELECT count(*)::text FROM auth_recovery_code WHERE used_at IS NOT NULL")
            .await,
        ["1"]
    );

    let again = challenge_of(&http, Who::A1).await;
    let refused = verify(
        &http,
        &again,
        &factor.recovery_codes[3].to_ascii_lowercase(),
        &[],
    )
    .await;
    error_of(&refused, 401, "unauthenticated", "auth.error.code");
    let another = verify(
        &http,
        &again,
        &format!(" {} ", factor.recovery_codes[4].to_ascii_lowercase()),
        &[],
    )
    .await;
    assert_eq!(another.status, 200, "{}", another.body);
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[tokio::test]
async fn disable_needs_a_code_for_oneself_and_an_admin_disables_a_factor_below_its_level_only() {
    let (harness, http) = boot().await;
    let a1 = harness.principal(Who::A1).id;
    let factor = enroll_and_confirm(&harness, Who::A1).await;

    let missing = http
        .post(Who::A1, "/api/v1/call/auth.totp.disable", &json!({}))
        .await;
    field_error_of(&missing, "code", "auth.error.code_required");
    let wrong = http
        .post(
            Who::A1,
            "/api/v1/call/auth.totp.disable",
            &json!({ "code": "000000" }),
        )
        .await;
    field_error_of(&wrong, "code", "auth.error.code");
    let confirmed_step = http
        .post(
            Who::A1,
            "/api/v1/call/auth.totp.disable",
            &json!({ "code": code_at(&harness, &factor.secret, 0) }),
        )
        .await;
    field_error_of(&confirmed_step, "code", "auth.error.code");
    let disabled = http
        .post(
            Who::A1,
            "/api/v1/call/auth.totp.disable",
            &json!({ "code": code_at(&harness, &factor.secret, 1) }),
        )
        .await;
    assert_eq!(disabled.status, 200, "{}", disabled.body);
    assert_eq!(
        disabled.json()["data"],
        json!({ "principal": a1, "disabled": true })
    );
    assert_eq!(
        harness
            .database()
            .rows("SELECT count(*)::text FROM auth_recovery_code")
            .await,
        ["0"]
    );
    let one_step = login(&http, Who::A1, &[]).await;
    assert_eq!(one_step.status, 200, "{}", one_step.body);
    assert_eq!(one_step.json()["data"]["state"], json!("authenticated"));
    let cookie = cookie_of(&one_step);
    assert_eq!(boot_totp(&http, &cookie).await, json!(false));

    let factor = enroll_and_confirm(&harness, Who::A1).await;
    let with_recovery = http
        .post(
            Who::A1,
            "/api/v1/call/auth.totp.disable",
            &json!({ "code": factor.recovery_codes[0] }),
        )
        .await;
    assert_eq!(with_recovery.status, 200, "{}", with_recovery.body);
    assert_eq!(with_recovery.json()["data"]["disabled"], json!(true));

    enroll_and_confirm(&harness, Who::A1).await;
    let by_reseller = http
        .post(
            Who::Reseller,
            "/api/v1/call/auth.totp.disable",
            &json!({ "principal": a1 }),
        )
        .await;
    error_of(
        &by_reseller,
        403,
        "forbidden_field",
        "keel.error.forbidden_field",
    );
    assert_eq!(
        by_reseller.json()["error"]["fields"][0]["path"],
        json!("principal")
    );
    let by_admin = http
        .post(
            Who::Admin,
            "/api/v1/call/auth.totp.disable",
            &json!({ "principal": a1 }),
        )
        .await;
    assert_eq!(by_admin.status, 200, "{}", by_admin.body);
    assert_eq!(
        by_admin.json()["data"],
        json!({ "principal": a1, "disabled": true })
    );
    let nothing_left = http
        .post(
            Who::Admin,
            "/api/v1/call/auth.totp.disable",
            &json!({ "principal": a1 }),
        )
        .await;
    assert_eq!(nothing_left.json()["data"]["disabled"], json!(false));
    let second_admin = harness
        .call(
            Who::Owner,
            "keel.principal.create",
            json!({ "login": "admin2", "level": "admin", "account": harness.fixtures().accounts.root }),
        )
        .await;
    let second_admin = second_admin
        .data()
        .unwrap_or_else(|| panic!("{:?}", second_admin.error()))["id"]
        .clone();
    let peer = http
        .post(
            Who::Admin,
            "/api/v1/call/auth.totp.disable",
            &json!({ "principal": second_admin }),
        )
        .await;
    error_of(&peer, 403, "forbidden", "auth.error.not_below");
    let owner = harness
        .call(Who::Owner, "auth.totp.enroll", json!({}))
        .await;
    assert_eq!(
        owner.error().map(|error| error.key.clone()),
        Some("auth.error.totp_owner".to_owned())
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn two_concurrent_verifies_of_one_challenge_and_two_redeems_of_one_recovery_code_race_to_one_session()
 {
    let (harness, http) = boot().await;
    let factor = enroll_and_confirm(&harness, Who::A1).await;
    harness
        .clock()
        .advance(Duration::seconds(TOTP_STEP_SECONDS));
    let challenge = challenge_of(&http, Who::A1).await;
    let code = code_at(&harness, &factor.secret, 0);
    let (first, second) = tokio::join!(
        verify(&http, &challenge, &code, &[]),
        verify(&http, &challenge, &code, &[])
    );
    let mut statuses = [first.status, second.status];
    statuses.sort_unstable();
    assert_eq!(statuses, [200, 401], "{} / {}", first.body, second.body);
    assert_eq!(
        harness
            .database()
            .rows("SELECT count(*)::text FROM auth_session")
            .await,
        ["1"]
    );

    let one = challenge_of(&http, Who::A1).await;
    let two = challenge_of(&http, Who::A1).await;
    let recovery = &factor.recovery_codes[0];
    let (first, second) = tokio::join!(
        verify(&http, &one, recovery, &[]),
        verify(&http, &two, recovery, &[])
    );
    let mut statuses = [first.status, second.status];
    statuses.sort_unstable();
    assert_eq!(statuses, [200, 401], "{} / {}", first.body, second.body);
    assert_eq!(
        harness
            .database()
            .rows("SELECT count(*)::text FROM auth_session")
            .await,
        ["2"]
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn twenty_concurrent_wrong_codes_against_one_challenge_get_five_answers_and_a_refusal() {
    let (harness, http) = boot().await;
    let factor = enroll_and_confirm(&harness, Who::A1).await;
    harness
        .clock()
        .advance(Duration::seconds(TOTP_STEP_SECONDS));
    let challenge = challenge_of(&http, Who::A1).await;
    let answers = join_all((0..20).map(|_| verify(&http, &challenge, "000000", &[]))).await;
    let keys: Vec<String> = answers
        .iter()
        .map(|answer| {
            let key = answer.json()["error"]["key"]
                .as_str()
                .expect("an error key")
                .to_owned();
            let expected = if key == "auth.error.locked" { 403 } else { 401 };
            assert_eq!(answer.status, expected, "{}", answer.body);
            assert_eq!(answer.set_cookie(), None);
            key
        })
        .collect();
    let wrong_code = keys.iter().filter(|key| *key == "auth.error.code").count();
    let refused = keys
        .iter()
        .filter(|key| *key == "auth.error.challenge" || *key == "auth.error.locked")
        .count();
    assert_eq!(wrong_code, CHALLENGE_ATTEMPTS as usize, "{keys:?}");
    assert_eq!(refused, keys.len() - wrong_code, "{keys:?}");
    let locked = verify(
        &http,
        &challenge,
        &code_at(&harness, &factor.secret, 0),
        &[],
    )
    .await;
    error_of(&locked, 403, "forbidden", "auth.error.locked");
    unlock(&harness, Who::A1).await;
    let late = verify(
        &http,
        &challenge,
        &code_at(&harness, &factor.secret, 0),
        &[],
    )
    .await;
    error_of(&late, 401, "unauthenticated", "auth.error.challenge");
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn enrolling_again_replaces_an_unconfirmed_factor_and_conflicts_with_a_confirmed_one() {
    let (harness, http) = boot().await;
    let (first, _) = enroll(&harness, Who::A1).await;
    let (second, _) = enroll(&harness, Who::A1).await;
    assert_ne!(first, second);
    let stale = harness
        .call(
            Who::A1,
            "auth.totp.confirm",
            json!({ "code": code_at(&harness, &first, 0) }),
        )
        .await;
    assert_eq!(field_key(&stale), Some("auth.error.code".to_owned()));
    let unconfirmed = login(&http, Who::A1, &[]).await;
    assert_eq!(unconfirmed.json()["data"]["state"], json!("authenticated"));
    let confirmed = harness
        .call(
            Who::A1,
            "auth.totp.confirm",
            json!({ "code": code_at(&harness, &second, 0) }),
        )
        .await;
    assert!(confirmed.data().is_some(), "{:?}", confirmed.error());
    let again = harness.call(Who::A1, "auth.totp.enroll", json!({})).await;
    let error = again.error().expect("a conflict");
    assert_eq!(error.code, keel_testkit::ErrorCode::Conflict);
    assert_eq!(error.key, "auth.error.totp_enrolled");
    let twice = harness
        .call(
            Who::A1,
            "auth.totp.confirm",
            json!({ "code": code_at(&harness, &second, 1) }),
        )
        .await;
    assert_eq!(
        field_key(&twice),
        Some("auth.error.totp_enrolled".to_owned())
    );
    let nobody = harness
        .call(Who::A2, "auth.totp.confirm", json!({ "code": "123456" }))
        .await;
    assert_eq!(
        field_key(&nobody),
        Some("auth.error.totp_not_enrolled".to_owned())
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn a_principal_promoted_to_owner_during_its_challenge_is_refused_by_verify_as_by_login() {
    let (harness, http) = boot().await;
    let factor = enroll_and_confirm(&harness, Who::A1).await;
    harness
        .clock()
        .advance(Duration::seconds(TOTP_STEP_SECONDS));
    let challenge = challenge_of(&http, Who::A1).await;
    let a1 = harness.principal(Who::A1).id;
    let root = harness.fixtures().accounts.root;
    let promoted = harness
        .call(
            Who::Owner,
            "keel.principal.update",
            json!({ "id": a1, "level": "owner", "account": root }),
        )
        .await;
    assert!(promoted.data().is_some(), "{:?}", promoted.error());
    let code = code_at(&harness, &factor.secret, 0);
    let refused = verify(&http, &challenge, &code, &[]).await;
    error_of(&refused, 401, "unauthenticated", "auth.error.challenge");
    let recovery = verify(&http, &challenge, &factor.recovery_codes[0], &[]).await;
    error_of(&recovery, 401, "unauthenticated", "auth.error.challenge");
    let direct = login(&http, Who::A1, &[]).await;
    error_of(&direct, 401, "unauthenticated", "auth.error.credentials");
    assert_eq!(
        harness
            .database()
            .rows("SELECT count(*)::text FROM auth_session")
            .await,
        ["0"]
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn verify_is_refused_cross_site_and_replaces_the_session_it_arrived_with() {
    let (harness, http) = boot().await;
    let factor = enroll_and_confirm(&harness, Who::A1).await;
    harness
        .clock()
        .advance(Duration::seconds(TOTP_STEP_SECONDS));
    let challenge = challenge_of(&http, Who::A1).await;
    let code = code_at(&harness, &factor.secret, 0);
    let foreign = verify(
        &http,
        &challenge,
        &code,
        &[("origin", "https://evil.example")],
    )
    .await;
    error_of(&foreign, 403, "forbidden", "keel.error.cross_site");

    let earlier = login(&http, Who::A2, &[]).await;
    assert_eq!(earlier.status, 200, "{}", earlier.body);
    let earlier_cookie = cookie_of(&earlier);
    let replaced = verify(&http, &challenge, &code, &[("cookie", &earlier_cookie)]).await;
    assert_eq!(replaced.status, 200, "{}", replaced.body);
    let fresh_cookie = cookie_of(&replaced);
    assert_ne!(fresh_cookie, earlier_cookie);
    assert_eq!(
        harness
            .database()
            .rows("SELECT reason FROM auth_session WHERE revoked_at IS NOT NULL")
            .await,
        [REASON_REPLACED]
    );
    let stale = http
        .request("GET", "/api/v1/boot", &[("cookie", &earlier_cookie)], None)
        .await;
    assert_eq!(stale.status, 401, "{}", stale.body);
    let booted = http
        .request("GET", "/api/v1/boot", &[("cookie", &fresh_cookie)], None)
        .await;
    assert_eq!(booted.json()["principal"]["login"], json!("a1"));
    assert_eq!(booted.json()["principal"]["totp"], json!(true));
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<AuditEvent>>,
}

impl AuditSink for Recorder {
    fn record(&self, event: AuditEvent) {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
    }
}

static RECORDER: std::sync::LazyLock<Arc<Recorder>> =
    std::sync::LazyLock::new(|| Arc::new(Recorder::default()));

static AUDIT: ModuleDef = ModuleDef {
    id: "auditprobe",
    version: "0.1.0",
    sdk: VERSION,
    requires: &[],
    migrations: None,
    init: audit_init,
};

fn audit_init(r: &mut Registrar) -> Result<()> {
    r.provide::<dyn AuditSink>(RECORDER.clone());
    Ok(())
}

#[tokio::test]
async fn the_code_and_the_challenge_are_masked_in_the_audit_event_and_the_descriptors_match() {
    let (harness, http) = boot_with(Some(&AUDIT)).await;
    let factor = enroll_and_confirm(&harness, Who::A1).await;
    let challenge = challenge_of(&http, Who::A1).await;
    let wrong = verify(&http, &challenge, "000000", &[]).await;
    error_of(&wrong, 401, "unauthenticated", "auth.error.code");
    let events: Vec<AuditEvent> = RECORDER
        .events
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter(|event| event.function.starts_with("auth.totp."))
        .cloned()
        .collect();
    let recorded = serde_json::to_string(&events).expect("events serialise");
    assert!(!recorded.contains(&challenge), "{recorded}");
    assert!(!recorded.contains(&base32_of(&factor.secret)), "{recorded}");
    let failed = events
        .iter()
        .find(|event| event.function == "auth.totp.verify")
        .expect("the failed verify is audited");
    assert!(!failed.outcome.is_committed());
    assert_eq!(failed.params["challenge"], json!("***"));
    assert_eq!(failed.params["code"], json!("***"));
    let confirmed = events
        .iter()
        .find(|event| event.function == "auth.totp.confirm")
        .expect("the confirm is audited");
    assert_eq!(confirmed.params["code"], json!("***"));

    let mut names = Vec::new();
    for level in [Level::Guest, Level::User, Level::Reseller, Level::Admin] {
        names.push(harness.snapshot_descriptors(level, snapshots_dir()));
    }
    assert_eq!(
        names[0],
        ["auth.key.redeem", "auth.login", "auth.totp.verify"]
    );
    assert!(
        names[1].contains(&"auth.totp.enroll".to_owned()),
        "{:?}",
        names[1]
    );
    let registry = harness.registry();
    let params_of = |name: &str| {
        serde_json::to_value(
            &registry
                .function(name)
                .unwrap_or_else(|| panic!("{name} is registered"))
                .descriptor()
                .params,
        )
        .expect("a schema")
    };
    let result_of = |name: &str| {
        serde_json::to_value(
            &registry
                .function(name)
                .unwrap_or_else(|| panic!("{name} is registered"))
                .descriptor()
                .result,
        )
        .expect("a schema")
    };
    let verify_params = params_of("auth.totp.verify");
    assert_eq!(
        verify_params["properties"]["challenge"]["x-keel"]["secret"],
        json!(true)
    );
    assert_eq!(
        verify_params["properties"]["code"]["x-keel"]["secret"],
        json!(true)
    );
    assert_eq!(
        params_of("auth.totp.confirm")["properties"]["code"]["x-keel"]["secret"],
        json!(true)
    );
    let disable_params = params_of("auth.totp.disable");
    assert_eq!(
        disable_params["properties"]["code"]["x-keel"]["secret"],
        json!(true)
    );
    assert_eq!(
        disable_params["properties"]["principal"]["x-keel"]["level"],
        json!("admin")
    );
    let enrolled = result_of("auth.totp.enroll");
    assert_eq!(
        enrolled["properties"]["secret"]["x-keel"]["once"],
        json!(true)
    );
    assert_eq!(enrolled["properties"]["uri"]["x-keel"]["once"], json!(true));
    assert_eq!(
        result_of("auth.totp.confirm")["properties"]["recoveryCodes"]["x-keel"]["once"],
        json!(true)
    );
    assert_eq!(
        result_of("auth.login")["properties"]["challenge"]["x-keel"]["once"],
        json!(true)
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}
