use keel_sdk::prelude::*;
use keel_sdk::sea_orm::ConnectionTrait;
use keel_system::auth::MODULE;
use keel_system::auth::api::retire_credentials;
use keel_testkit::{Harness, Who, tokio};

async fn seed(harness: &Harness, retired: &Principal, bystander: &Principal) {
    let (r, ra) = (retired.id, retired.account_id);
    let (b, ba) = (bystander.id, bystander.account_id);
    let admin = harness.principal(Who::Admin).id;
    harness
        .db()
        .execute_unprepared(&format!(
            "INSERT INTO auth_password (principal_id, hash, updated_at) VALUES \
             ('{r}', 'hash-r', now()), ('{b}', 'hash-b', now());
             INSERT INTO auth_token (id, token_hash, principal_id, account_id, name, level, scope, \
             ip_allow, created_at) VALUES \
             ('token-r', 'token-hash-r', '{r}', '{ra}', 'r', 'user', '\"full\"', '[]', now()), \
             ('token-b', 'token-hash-b', '{b}', '{ba}', 'b', 'user', '\"full\"', '[]', now());
             INSERT INTO auth_session (id_hash, principal_id, account_id, actor_principal_id, kind, \
             created_at, idle_until, absolute_until) VALUES \
             ('session-r', '{r}', '{ra}', NULL, 'session', now(), now() + interval '1 hour', now() + interval '1 hour'), \
             ('session-su-by-r', '{b}', '{ba}', '{r}', 'su', now(), now() + interval '1 hour', now() + interval '1 hour'), \
             ('session-b', '{b}', '{ba}', NULL, 'session', now(), now() + interval '1 hour', now() + interval '1 hour');
             INSERT INTO auth_login_key (key_hash, principal_id, issued_by, redirect, expires_at, \
             created_at) VALUES \
             ('key-r', '{r}', '{admin}', '/', now() + interval '5 minutes', now()), \
             ('key-by-r', '{b}', '{r}', '/', now() + interval '5 minutes', now()), \
             ('key-b', '{b}', '{admin}', '/', now() + interval '5 minutes', now());
             INSERT INTO auth_login_challenge (challenge_hash, principal_id, expires_at, created_at) \
             VALUES ('challenge-r', '{r}', now() + interval '5 minutes', now()), \
             ('challenge-b', '{b}', now() + interval '5 minutes', now());
             INSERT INTO auth_mail_token (token_hash, purpose, principal_id, expires_at, created_at) \
             VALUES ('mail-r', 'reset', '{r}', now() + interval '1 hour', now()), \
             ('mail-b', 'reset', '{b}', now() + interval '1 hour', now());"
        ))
        .await
        .unwrap();
}

#[tokio::test]
async fn retiring_a_principal_ends_every_credential_it_holds_or_issued_and_no_other() {
    let harness = Harness::new().module(&MODULE).boot().await;
    let retired = harness.principal(Who::A1).clone();
    let bystander = harness.principal(Who::A2).clone();
    seed(&harness, &retired, &bystander).await;

    let db = Db::from_connection(harness.db().clone());
    retire_credentials(&db, retired.id, harness.clock().now())
        .await
        .expect("the credentials are retired");

    let state = harness
        .database()
        .rows(
            "SELECT 'password ' || hash FROM auth_password WHERE hash LIKE 'hash-%' \
             UNION ALL SELECT 'token ' || id || ' ' || (revoked_at IS NOT NULL)::text FROM auth_token WHERE id LIKE 'token-%' \
             UNION ALL SELECT 'session ' || id_hash || ' ' || coalesce(reason, 'open') FROM auth_session WHERE id_hash LIKE 'session-%' \
             UNION ALL SELECT 'key ' || key_hash || ' ' || (used_at IS NOT NULL)::text FROM auth_login_key WHERE key_hash LIKE 'key-%' \
             UNION ALL SELECT 'challenge ' || challenge_hash || ' ' || (used_at IS NOT NULL)::text FROM auth_login_challenge WHERE challenge_hash LIKE 'challenge-%' \
             UNION ALL SELECT 'mail ' || token_hash || ' ' || (used_at IS NOT NULL)::text FROM auth_mail_token WHERE token_hash LIKE 'mail-%' \
             ORDER BY 1",
        )
        .await;
    assert_eq!(
        state,
        [
            "challenge challenge-b false",
            "challenge challenge-r true",
            "key key-b false",
            "key key-by-r true",
            "key key-r true",
            "mail mail-b false",
            "mail mail-r true",
            "password hash-b",
            "session session-b open",
            "session session-r revoked",
            "session session-su-by-r revoked",
            "token token-b false",
            "token token-r true",
        ]
    );

    harness.finish().await;
}
