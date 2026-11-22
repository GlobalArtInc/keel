use keel_sdk::prelude::*;
use keel_sdk::sea_orm::ConnectionTrait;
use keel_sdk::types::TaskState;
use keel_system::tasks::api::Drain;
use keel_testkit::{Harness, Who, tokio};

const VICTIM_SECOND: &str = "x-victim-second";

async fn seed(harness: &Harness, victim: &Principal, bystander: &Principal) {
    let (v, va) = (victim.id, victim.account_id);
    let (b, ba) = (bystander.id, bystander.account_id);
    let v2 = Id::generate();
    harness
        .db()
        .execute_unprepared(&format!(
            "INSERT INTO keel_principal (id, account_id, login, level, created_at, updated_at) \
             VALUES ('{v2}', '{va}', '{VICTIM_SECOND}', 'user', now(), now());
             INSERT INTO auth_password (principal_id, hash, updated_at) VALUES \
             ('{v}', 'x-password-v1', now()), ('{v2}', 'x-password-v2', now()), ('{b}', 'b-password', now());
             INSERT INTO auth_totp (principal_id, secret, created_at) VALUES \
             ('{v}', 'x-totp-v1', now()), ('{v2}', 'x-totp-v2', now()), ('{b}', 'b-totp', now());
             INSERT INTO auth_recovery_code (id, principal_id, code_hash) VALUES \
             ('x-recovery-v1', '{v}', 'h'), ('x-recovery-v2', '{v2}', 'h'), ('b-recovery', '{b}', 'h');
             INSERT INTO auth_principal_address (principal_id, kind, value, updated_at) VALUES \
             ('{v}', 'email', 'x-address-v1', now()), ('{v2}', 'email', 'x-address-v2', now()), \
             ('{b}', 'email', 'b-address', now());
             INSERT INTO auth_login_challenge (challenge_hash, principal_id, expires_at, created_at) VALUES \
             ('x-challenge-v1', '{v}', now(), now()), ('x-challenge-v2', '{v2}', now(), now()), \
             ('b-challenge', '{b}', now(), now());
             INSERT INTO auth_mail_token (token_hash, purpose, principal_id, expires_at, created_at) VALUES \
             ('x-mail-v1', 'reset', '{v}', now(), now()), ('x-mail-v2', 'reset', '{v2}', now(), now()), \
             ('b-mail', 'reset', '{b}', now(), now());
             INSERT INTO auth_token (id, token_hash, principal_id, account_id, link_principal_id, name, \
             level, scope, ip_allow, created_at) VALUES \
             ('x-token-v1', 'x-token-hash-v1', '{v}', '{va}', NULL, 't', 'user', '\"full\"', '[]', now()), \
             ('x-token-v2', 'x-token-hash-v2', '{v2}', '{va}', NULL, 't', 'user', '\"full\"', '[]', now()), \
             ('x-token-linked', 'x-token-hash-linked', '{b}', '{ba}', '{v2}', 't', 'user', '\"full\"', '[]', now()), \
             ('x-token-in-account', 'x-token-hash-in-account', '{b}', '{va}', NULL, 't', 'user', '\"full\"', '[]', now()), \
             ('b-token', 'b-token-hash', '{b}', '{ba}', NULL, 't', 'user', '\"full\"', '[]', now());
             INSERT INTO auth_login_key (key_hash, principal_id, issued_by, link_principal_id, redirect, \
             expires_at, created_at) VALUES \
             ('x-key-v1', '{v}', '{b}', NULL, '/', now(), now()), \
             ('x-key-issued', '{b}', '{v2}', NULL, '/', now(), now()), \
             ('x-key-linked', '{b}', '{b}', '{v}', '/', now(), now()), \
             ('b-key', '{b}', '{b}', NULL, '/', now(), now());
             INSERT INTO auth_grant (id, principal_id, account_id, key, granted, created_at, created_by) VALUES \
             ('x-grant-v1', '{v}', '{va}', 'k.one', true, now(), 'x'), \
             ('x-grant-v2', '{v2}', '{va}', 'k.one', true, now(), 'x'), \
             ('x-grant-in-account', '{b}', '{va}', 'k.two', true, now(), 'x'), \
             ('b-grant', '{b}', '{ba}', 'k.one', true, now(), 'x');
             INSERT INTO auth_session (id_hash, principal_id, account_id, actor_principal_id, \
             link_principal_id, kind, created_at, idle_until, absolute_until) VALUES \
             ('x-session-root', '{v}', '{va}', NULL, NULL, 'session', now(), now(), now()), \
             ('x-session-v2', '{v2}', '{va}', NULL, NULL, 'session', now(), now(), now()), \
             ('x-session-actor', '{b}', '{ba}', '{v2}', NULL, 'su', now(), now(), now()), \
             ('x-session-linked', '{b}', '{ba}', NULL, '{v}', 'session', now(), now(), now()), \
             ('x-session-in-account', '{b}', '{va}', NULL, NULL, 'session', now(), now(), now()), \
             ('b-session', '{b}', '{ba}', NULL, NULL, 'session', now(), now(), now());
             INSERT INTO auth_session (id_hash, principal_id, account_id, actor_principal_id, kind, \
             parent_id_hash, created_at, idle_until, absolute_until) VALUES \
             ('x-session-child', '{v}', '{va}', NULL, 'session', 'x-session-root', now(), now(), now()), \
             ('b-session-child', '{b}', '{ba}', NULL, 'su', 'b-session', now(), now(), now());
             INSERT INTO auth_session (id_hash, principal_id, account_id, kind, created_at, idle_until, \
             absolute_until) VALUES ('b-admin-session', '{b}', '{ba}', 'session', now(), now(), now());
             INSERT INTO auth_session (id_hash, principal_id, account_id, actor_principal_id, kind, \
             parent_id_hash, created_at, idle_until, absolute_until) VALUES \
             ('x-session-su-child', '{v}', '{va}', '{b}', 'su', 'b-admin-session', now(), now(), now());
             UPDATE auth_session SET su_id_hash = 'x-session-su-child' WHERE id_hash = 'b-admin-session';
             UPDATE auth_session SET su_id_hash = 'b-session-child' WHERE id_hash = 'b-session';
             INSERT INTO settings_value (scope_id, key, value, updated_at, updated_by) VALUES \
             ('{va}', 'x-value-v', '1', now(), 'x'), ('{ba}', 'b-value', '1', now(), 'x');
             INSERT INTO settings_pref (principal_id, key, value, updated_at) VALUES \
             ('{v}', 'x-pref-v1', '1', now()), ('{v2}', 'x-pref-v2', '1', now()), ('{b}', 'b-pref', '1', now());
             INSERT INTO notify_problem (id, key, subject, severity, params_json, account_id, source, \
             raised_at, updated_at) VALUES \
             ('x-problem-v', 'k', 's', 'warning', '{{}}', '{va}', 'x', now(), now()), \
             ('b-problem', 'k', 'b', 'warning', '{{}}', '{ba}', 'x', now(), now());
             INSERT INTO notify_message (id, principal_id, account_id, key, params_json, level, source, \
             problem_id, created_at) VALUES \
             ('x-message-v', '{v}', '{va}', 'k', '{{}}', 'info', 'x', NULL, now()), \
             ('x-message-of-problem', '{b}', '{ba}', 'k', '{{}}', 'info', 'x', 'x-problem-v', now()), \
             ('b-message', '{b}', '{ba}', 'k', '{{}}', 'info', 'x', 'b-problem', now());
             INSERT INTO notify_channel (id, kind, name, enabled, config_json, created_at, created_by) \
             VALUES ('x-channel', 'email', 'x', false, '{{}}', now(), 'x');
             INSERT INTO notify_direct (id, principal_id, channel_id, channel, template, address, state, \
             next_at, created_at) VALUES \
             ('x-direct-v', '{v}', 'x-channel', 'email', 't', 'a@example.org', 'sent', now(), now()), \
             ('x-direct-v2', '{v2}', 'x-channel', 'email', 't', 'a@example.org', 'sent', now(), now()), \
             ('b-direct', '{b}', 'x-channel', 'email', 't', 'a@example.org', 'sent', now(), now());
             INSERT INTO webhook_subscription (id, account_id, principal_id, name, url, secret, events, \
             enabled, created_at, created_by, updated_at) VALUES \
             ('x-subscription-v', '{va}', '{v}', 'n', 'https://example.org', 's', '{{x.never.*}}', false, now(), 'x', now()), \
             ('b-subscription', '{ba}', '{b}', 'n', 'https://example.org', 's', '{{x.never.*}}', false, now(), 'x', now());
             INSERT INTO webhook_delivery (id, subscription_id, account_id, event_id, seq, type, state, \
             next_at, created_at) VALUES \
             ('x-delivery-v', 'x-subscription-v', '{va}', 'e1', 1, 'x.never.one', 'delivered', now(), now()), \
             ('b-delivery', 'b-subscription', '{ba}', 'e1', 1, 'x.never.one', 'delivered', now(), now());"
        ))
        .await
        .unwrap();
}

const MARKED: &str = "SELECT id FROM auth_token WHERE id LIKE '%-token%' \
    UNION ALL SELECT key_hash FROM auth_login_key WHERE key_hash LIKE '%-key%' \
    UNION ALL SELECT id_hash FROM auth_session WHERE id_hash LIKE '%-session%' \
    UNION ALL SELECT id FROM auth_grant WHERE id LIKE '%-grant%' \
    UNION ALL SELECT challenge_hash FROM auth_login_challenge WHERE challenge_hash LIKE '%-challenge%' \
    UNION ALL SELECT token_hash FROM auth_mail_token WHERE token_hash LIKE '%-mail%' \
    UNION ALL SELECT id FROM auth_recovery_code WHERE id LIKE '%-recovery%' \
    UNION ALL SELECT secret FROM auth_totp WHERE secret LIKE '%-totp%' \
    UNION ALL SELECT hash FROM auth_password WHERE hash LIKE '%-password%' \
    UNION ALL SELECT value FROM auth_principal_address WHERE value LIKE '%-address%' \
    UNION ALL SELECT key FROM settings_value WHERE key LIKE '%-value%' \
    UNION ALL SELECT key FROM settings_pref WHERE key LIKE '%-pref%' \
    UNION ALL SELECT id FROM notify_problem WHERE id LIKE '%-problem%' \
    UNION ALL SELECT id FROM notify_message WHERE id LIKE '%-message%' \
    UNION ALL SELECT id FROM notify_direct WHERE id LIKE '%-direct%' \
    UNION ALL SELECT id FROM webhook_subscription WHERE id LIKE '%-subscription%' \
    UNION ALL SELECT id FROM webhook_delivery WHERE id LIKE '%-delivery%' \
    ORDER BY 1";

#[tokio::test]
async fn deleting_an_account_removes_every_row_of_its_principals_and_leaves_the_others() {
    let harness = Harness::new()
        .module(&keel_system::tasks::MODULE)
        .module(&keel_system::auth::MODULE)
        .module(&keel_system::settings::MODULE)
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .module(&keel_system::webhooks::MODULE)
        .boot()
        .await;
    let victim = harness.principal(Who::A1).clone();
    let bystander = harness.principal(Who::A2).clone();
    seed(&harness, &victim, &bystander).await;
    let seeded = harness.database().rows(MARKED).await;
    assert_eq!(seeded.len(), 58, "{seeded:?}");

    let deletion = harness
        .call(
            Who::Admin,
            "keel.account.delete",
            json!({ "id": victim.account_id }),
        )
        .await;
    let task: Id = deletion
        .data()
        .and_then(|data| data["task"]["id"].as_str())
        .and_then(|id| id.parse().ok())
        .unwrap_or_else(|| panic!("a task was expected, got {:?}", deletion.envelope));
    let drain = harness
        .registry()
        .services()
        .get::<Drain>()
        .expect("the tasks module provides the drain");
    let state = tokio::time::timeout(std::time::Duration::from_secs(30), drain.wait(task))
        .await
        .expect("the deletion ends in time")
        .expect("the state is read");
    assert_eq!(state, TaskState::Done);

    assert_eq!(
        harness.database().rows(MARKED).await,
        [
            "b-address",
            "b-admin-session",
            "b-challenge",
            "b-delivery",
            "b-direct",
            "b-grant",
            "b-key",
            "b-mail",
            "b-message",
            "b-password",
            "b-pref",
            "b-problem",
            "b-recovery",
            "b-session",
            "b-session-child",
            "b-subscription",
            "b-token",
            "b-totp",
            "b-value",
        ]
    );
    let pointers = harness
        .database()
        .rows(
            "SELECT id_hash || ' ' || coalesce(su_id_hash, 'null') FROM auth_session \
             WHERE id_hash IN ('b-admin-session', 'b-session') ORDER BY id_hash",
        )
        .await;
    assert_eq!(
        pointers,
        ["b-admin-session null", "b-session b-session-child"]
    );
    let left = harness
        .database()
        .rows(&format!(
            "SELECT ((SELECT count(*) FROM keel_account WHERE id = '{account}') \
             + (SELECT count(*) FROM keel_principal WHERE account_id = '{account}') \
             + (SELECT count(*) FROM keel_principal WHERE login = '{VICTIM_SECOND}'))::text",
            account = victim.account_id,
        ))
        .await;
    assert_eq!(left, ["0"]);
    let kept = harness
        .database()
        .rows(&format!(
            "SELECT count(*)::text FROM keel_principal WHERE id IN ('{}', '{}')",
            bystander.id,
            harness.principal(Who::Admin).id,
        ))
        .await;
    assert_eq!(kept, ["2"]);

    harness.finish().await;
}
