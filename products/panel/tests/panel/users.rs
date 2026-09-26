use keel::prelude::*;
use keel::sdk::sea_orm::{ConnectionTrait, DbBackend, Statement};
use keel::types::{ErrorCode, TaskState};
use keel_system::tasks::api::Drain;
use keel_testkit::{Harness, Who, tokio};

use crate::support::{
    FIRST_UID, LAST_UID, PATIENCE, Site, create, data, eventually, panel, rows, state_becomes,
};

async fn account(harness: &Harness, verb: &str, account: Id) {
    let reply = harness
        .call(
            Who::Reseller,
            &format!("keel.account.{verb}"),
            json!({ "id": account }),
        )
        .await;
    assert_eq!(data(&reply)["changed"], json!(true), "{verb}");
}

async fn states(harness: &Harness) -> Vec<String> {
    rows(
        harness,
        "SELECT name || ' ' || state FROM hosting_user ORDER BY name",
    )
    .await
}

#[tokio::test]
async fn a_hosting_user_is_an_os_user_inside_the_uid_range_under_hp() {
    let site = Site::new();
    let harness = panel(&site).await;
    let a1 = harness.fixtures().accounts.a1;

    let reply = harness
        .call(Who::A1, "hosting.user.create", json!({ "name": "alice" }))
        .await;
    let created = data(&reply).clone();
    assert_eq!(created["name"], json!("hp_alice"));
    assert_eq!(created["accountId"], json!(a1.to_string()));
    assert_eq!(created["state"], json!("pending"));
    assert_eq!(created["uid"], Value::Null);
    let id = created["id"].as_str().expect("an id").to_owned();

    state_becomes(&harness, &id, "active").await;
    let home = site.home("alice");
    let got = harness
        .call(Who::A1, "hosting.user.get", json!({ "id": id }))
        .await;
    let uid = data(&got)["uid"].as_u64().expect("a uid");
    assert!((u64::from(FIRST_UID)..=u64::from(LAST_UID)).contains(&uid));
    assert_eq!(data(&got)["home"], json!(home.display().to_string()));
    assert_eq!(data(&got)["shell"], json!(false));
    assert!(home.is_dir());
    assert_eq!(
        site.passwd_line("hp_alice"),
        Some(format!(
            "hp_alice:x:{uid}:{uid}::{}:/usr/sbin/nologin",
            home.display()
        ))
    );
    assert_eq!(
        site.calls(),
        [
            format!("groupadd --gid {uid} hp_alice"),
            format!(
                "useradd --uid {uid} --gid {uid} --shell /usr/sbin/nologin --home-dir {} --create-home hp_alice",
                home.display()
            ),
        ]
    );

    let edited = harness
        .call(
            Who::A1,
            "hosting.user.edit",
            json!({ "id": id, "shell": true }),
        )
        .await;
    assert_eq!(data(&edited)["shell"], json!(true));
    eventually("the shell of hp_alice turns to bash", async || {
        site.passwd_line("hp_alice")
            .is_some_and(|line| line.ends_with(":/bin/bash"))
    })
    .await;
    assert_eq!(
        site.calls().last().map(String::as_str),
        Some("usermod --shell /bin/bash hp_alice")
    );

    let taken = harness
        .call(Who::A2, "hosting.user.create", json!({ "name": "alice" }))
        .await;
    let error = taken.error().expect("the name is taken");
    assert_eq!(error.code, ErrorCode::Validation, "{error:?}");
    assert_eq!(error.fields[0].path, "name");
    assert_eq!(
        error.fields[0].key.as_deref(),
        Some("hosting.user.error.exists")
    );
    let malformed = harness
        .call(Who::A1, "hosting.user.create", json!({ "name": "bob;id" }))
        .await;
    assert_eq!(
        malformed.error().expect("the name is refused").code,
        ErrorCode::Validation
    );
    let theirs = harness.call(Who::A2, "hosting.user.list", json!({})).await;
    assert_eq!(data(&theirs)["total"], json!(0));
    let hidden = harness
        .call(Who::A2, "hosting.user.get", json!({ "id": id }))
        .await;
    assert!(hidden.error().is_some());

    let deleted = harness
        .call(Who::A1, "hosting.user.delete", json!({ "ids": [id] }))
        .await;
    assert_eq!(data(&deleted)["deleted"], json!(1));
    eventually("hp_alice and its home are removed", async || {
        site.passwd_line("hp_alice").is_none() && !home.exists()
    })
    .await;
    assert!(
        site.calls()
            .contains(&"userdel --remove hp_alice".to_owned())
    );
    assert!(states(&harness).await.is_empty());
    harness.finish().await;
}

#[tokio::test]
async fn suspending_the_account_locks_its_users_and_deleting_removes_them() {
    let site = Site::new();
    let harness = panel(&site).await;
    let a1 = harness.fixtures().accounts.a1;
    let mine = [
        create(&harness, Who::A1, "alice").await,
        create(&harness, Who::A1, "bob").await,
    ];
    let theirs = create(&harness, Who::A2, "carol").await;
    for id in mine.iter().chain([&theirs]) {
        state_becomes(&harness, id, "active").await;
    }

    account(&harness, "suspend", a1).await;
    eventually(
        "the users of the suspended account are locked",
        async || site.locked("hp_alice") && site.locked("hp_bob"),
    )
    .await;
    assert!(!site.locked("hp_carol"));
    assert_eq!(
        states(&harness).await,
        ["hp_alice suspended", "hp_bob suspended", "hp_carol active"]
    );
    let calls = site.calls();
    for user in ["hp_alice", "hp_bob"] {
        assert!(
            calls.contains(&format!("usermod --lock --expiredate 1 {user}")),
            "{calls:?}"
        );
    }

    account(&harness, "resume", a1).await;
    eventually(
        "the users of the resumed account are unlocked",
        async || !site.locked("hp_alice") && !site.locked("hp_bob"),
    )
    .await;
    assert_eq!(
        states(&harness).await,
        ["hp_alice active", "hp_bob active", "hp_carol active"]
    );

    let queued = harness
        .call(Who::Reseller, "keel.account.delete", json!({ "id": a1 }))
        .await;
    let task: Id = data(&queued)["task"]["id"]
        .as_str()
        .expect("the task id")
        .parse()
        .expect("a ulid");
    let drain = harness
        .registry()
        .services()
        .get::<Drain>()
        .expect("the tasks module provides the drain");
    let state = tokio::time::timeout(PATIENCE, drain.wait(task))
        .await
        .expect("the job ends in time")
        .expect("the state is read");
    let detail = harness
        .call(Who::Reseller, "task.get", json!({ "id": task }))
        .await;
    assert_eq!(state, TaskState::Done, "{}", data(&detail));
    assert!(
        data(&detail)["logTail"]
            .to_string()
            .contains(&format!("removed 2 hosting user(s) of account {a1}"))
    );
    eventually("the users of the deleted account are removed", async || {
        site.passwd_line("hp_alice").is_none()
            && site.passwd_line("hp_bob").is_none()
            && !site.home("alice").exists()
            && !site.home("bob").exists()
    })
    .await;
    assert!(site.passwd_line("hp_carol").is_some());
    assert!(site.home("carol").is_dir());
    assert_eq!(states(&harness).await, ["hp_carol active"]);
    harness.finish().await;
}

#[tokio::test]
async fn the_users_limit_is_enforced() {
    let site = Site::new();
    let harness = panel(&site).await;
    let a1 = harness.fixtures().accounts.a1;
    harness
        .db()
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO keel_limit (account_id, key, value) VALUES ($1, 'hosting.users', '1'::jsonb)",
            [a1.to_string().into()],
        ))
        .await
        .expect("the limit of one user");

    let first = create(&harness, Who::A1, "first").await;
    state_becomes(&harness, &first, "active").await;
    let refused = harness
        .call(Who::A1, "hosting.user.create", json!({ "name": "second" }))
        .await;
    let error = refused.error().expect("the second user is over the limit");
    assert_eq!(error.code, ErrorCode::LimitExceeded);
    assert_eq!(error.key, "keel.error.limit_exceeded");
    assert_eq!(error.params["limit"], json!("hosting.users"));
    assert_eq!(error.params["max"], json!(1));
    let theirs = create(&harness, Who::A2, "second").await;
    state_becomes(&harness, &theirs, "active").await;

    assert_eq!(
        rows(
            &harness,
            "SELECT name || ' ' || account_id FROM hosting_user ORDER BY name"
        )
        .await,
        [
            format!("hp_first {a1}"),
            format!("hp_second {}", harness.fixtures().accounts.a2),
        ]
    );
    let created = site
        .calls()
        .iter()
        .filter(|call| call.starts_with("useradd "))
        .count();
    assert_eq!(created, 2);
    harness.finish().await;
}
