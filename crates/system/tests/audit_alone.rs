use keel_sdk::chrono::{DateTime, Duration, Utc};
use keel_sdk::prelude::*;
use keel_system::audit::api::Flush;
use keel_system::audit::{MODULE, STATUS_COMMITTED};
use keel_testkit::{Harness, Who, tokio};

const SEEDED_AT: &str = "2026-09-13T19:59:59Z";

fn seeded_at() -> DateTime<Utc> {
    SEEDED_AT.parse().expect("an instant")
}

#[tokio::test]
async fn audit_alone_boots_with_its_table_and_one_owner_write_leaves_one_row_after_a_flush() {
    let harness = Harness::new()
        .module(&MODULE)
        .clock(seeded_at())
        .boot()
        .await;
    assert_eq!(harness.modules(), ["keel", "audit", "testkit"]);
    let applied = harness
        .database()
        .rows("SELECT count(*)::text FROM keel_migration_audit")
        .await;
    assert_eq!(applied, ["1"]);
    let tables = harness
        .database()
        .rows("SELECT table_name FROM information_schema.tables WHERE table_name LIKE 'audit\\_%' ORDER BY table_name")
        .await;
    assert_eq!(tables, ["audit_call"]);

    harness.clock().advance(Duration::seconds(1));
    let created = harness
        .call(Who::Owner, "keel.account.create", json!({ "kind": "user" }))
        .await;
    assert!(
        created.data().is_some(),
        "keel.account.create: {:?}",
        created.error()
    );
    let flush = harness
        .registry()
        .services()
        .get::<Flush>()
        .expect("the audit module provides Flush");
    flush.flush().await.expect("the worker answers the flush");
    assert_eq!(flush.dropped(), 0);

    let trail = harness
        .database()
        .rows(&format!(
            "SELECT function || ' ' || principal_login || ' ' || via || ' ' || status || ' ' || params_redacted::text FROM audit_call WHERE at > '{SEEDED_AT}'"
        ))
        .await;
    assert_eq!(
        trail,
        [format!(
            "keel.account.create {} socket {STATUS_COMMITTED} {{\"kind\": \"user\"}}",
            Who::Owner.login()
        )]
    );
    harness.finish().await;
}
