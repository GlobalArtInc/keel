use std::fs;

use keel::prelude::*;
use keel::types::{ErrorCode, TaskState};
use keel_host::api::{HostExecutor, Origin};
use keel_system::tasks::api::Drain;
use keel_testkit::{Harness, Who, tokio};

use crate::support::{
    PATIENCE, Site, composed, create, data, eventually, panel, rows, state_becomes,
};

const CLIENT: &str =
    "mariadb --batch --sandbox --skip-column-names --default-character-set=utf8mb4";
const PASSWORD: &str = "it's a \"pass\\word\"";

fn statements(site: &Site) -> Vec<String> {
    site.calls()
        .into_iter()
        .filter(|line| {
            line.starts_with("mariadb ")
                || (line.starts_with(|c: char| c.is_ascii_uppercase())
                    && !line.starts_with("SET SESSION"))
        })
        .collect()
}

async fn statements_reach(site: &Site, count: usize) {
    eventually(&format!("{count} mariadb lines"), async || {
        statements(site).len() >= count
    })
    .await;
}

async fn created(harness: &Harness, who: Who, function: &str, params: Value) -> String {
    let reply = harness.call(who, function, params).await;
    data(&reply)["id"].as_str().expect("an id").to_owned()
}

async fn state_of(harness: &Harness, table: &str, id: &str, state: &str) {
    let sql = format!("SELECT state FROM {table} WHERE id = '{id}'");
    eventually(&format!("{table} {id} in state {state}"), async || {
        rows(harness, &sql).await == [state]
    })
    .await;
}

fn covers(scope: &str, database: &str) -> bool {
    let mut chars = scope.chars();
    let mut pattern = Vec::new();
    while let Some(c) = chars.next() {
        pattern.push(match c {
            '\\' => chars.next(),
            '_' => None,
            other => Some(other),
        });
    }
    pattern.len() == database.chars().count()
        && pattern
            .iter()
            .zip(database.chars())
            .all(|(expected, c)| expected.is_none_or(|expected| expected == c))
}

fn refused_name(reply: &keel_testkit::Reply, key: &str) {
    let error = reply.error().expect("the name is refused");
    assert_eq!(error.code, ErrorCode::Validation, "{error:?}");
    assert_eq!(error.fields[0].path, "name", "{error:?}");
    assert_eq!(error.fields[0].key.as_deref(), Some(key), "{error:?}");
}

async fn account(harness: &Harness, verb: &str, account: Id) -> Value {
    harness
        .call(
            Who::Reseller,
            &format!("keel.account.{verb}"),
            json!({ "id": account }),
        )
        .await
        .data()
        .cloned()
        .unwrap_or_else(|| panic!("keel.account.{verb} answers"))
}

#[tokio::test]
async fn a_database_and_user_are_created_with_quoted_identifiers() {
    let site = Site::new();
    let harness = composed(&site)
        .section(
            "hosting_database",
            json!({ "dumps_dir": site.path("dumps").display().to_string() }),
        )
        .boot()
        .await;
    let alice = create(&harness, Who::A1, "alice").await;
    state_becomes(&harness, &alice, "active").await;

    let wp = created(
        &harness,
        Who::A1,
        "hosting.database.create",
        json!({ "name": "hp_alice_wp" }),
    )
    .await;
    state_of(&harness, "hosting_database", &wp, "active").await;
    let app = created(
        &harness,
        Who::A1,
        "hosting.database.user.create",
        json!({ "name": "hp_alice_app", "password": PASSWORD, "databases": [wp] }),
    )
    .await;
    state_of(&harness, "hosting_database_user", &app, "active").await;
    assert_eq!(
        statements(&site),
        [
            CLIENT,
            "CREATE DATABASE `hp_alice_wp` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;",
            CLIENT,
            "CREATE USER 'hp_alice_app'@'localhost' IDENTIFIED BY 'it\\'s a \\\"pass\\\\word\\\"';",
            CLIENT,
            "GRANT ALL PRIVILEGES ON `hp\\_alice\\_wp`.* TO 'hp_alice_app'@'localhost';",
        ]
    );
    let read = harness
        .call(Who::A1, "hosting.database.user.get", json!({ "id": app }))
        .await;
    assert_eq!(data(&read)["databases"], json!([wp]));
    assert!(data(&read).get("password").is_none(), "{:?}", data(&read));

    let blog = created(
        &harness,
        Who::A1,
        "hosting.database.create",
        json!({ "name": "hp_alice_blog" }),
    )
    .await;
    state_of(&harness, "hosting_database", &blog, "active").await;
    for password in ["short".to_owned(), "a".repeat(129)] {
        let refused = harness
            .call(
                Who::A1,
                "hosting.database.user.edit",
                json!({ "id": app, "password": password, "databases": [wp] }),
            )
            .await;
        let error = refused.error().expect("the password is out of bounds");
        assert_eq!(error.code, ErrorCode::Validation, "{error:?}");
        assert_eq!(error.fields[0].path, "password", "{error:?}");
    }
    let edited = harness
        .call(
            Who::A1,
            "hosting.database.user.edit",
            json!({ "id": app, "password": "an0ther pass", "databases": [blog] }),
        )
        .await;
    data(&edited);
    statements_reach(&site, 15).await;
    assert_eq!(
        statements(&site)[8..],
        [
            CLIENT,
            "ALTER USER 'hp_alice_app'@'localhost' IDENTIFIED BY 'an0ther pass';",
            CLIENT,
            "GRANT ALL PRIVILEGES ON `hp\\_alice\\_blog`.* TO 'hp_alice_app'@'localhost';",
            CLIENT,
            "GRANT SELECT ON `hp\\_alice\\_wp`.* TO 'hp_alice_app'@'localhost';",
            "REVOKE ALL PRIVILEGES ON `hp\\_alice\\_wp`.* FROM 'hp_alice_app'@'localhost';",
        ]
    );

    let foreign = harness
        .call(
            Who::A2,
            "hosting.database.user.edit",
            json!({ "id": app, "databases": [] }),
        )
        .await;
    assert!(foreign.error().is_some());
    let busy = harness
        .call(Who::A1, "hosting.user.delete", json!({ "ids": [alice] }))
        .await;
    let error = busy.error().expect("a user with databases stays");
    assert_eq!(
        error.fields[0].key.as_deref(),
        Some("hosting.database.error.user_busy")
    );

    let backup = tempfile::tempdir().expect("a backup directory");
    let source = harness
        .registry()
        .contributions()
        .of::<BackupSource>()
        .find(|source| source.name() == "hosting_database.dumps")
        .expect("the databases are a backup source")
        .write_hook();
    source(BackupDir::new(
        "hosting_database.dumps",
        backup.path().to_owned(),
        Db::from_connection(harness.db().clone()),
        harness.data_dir(),
    ))
    .await
    .expect("every database is dumped");
    for name in ["hp_alice_blog", "hp_alice_wp"] {
        assert_eq!(
            fs::read_to_string(backup.path().join(format!("{name}.sql"))).expect("the dump"),
            format!("-- dump of {name}\n")
        );
        assert!(
            site.calls()
                .contains(&format!(
                    "mariadb-dump --single-transaction --skip-comments --default-character-set=utf8mb4 {name}"
                ))
        );
    }
    assert_eq!(
        fs::read_dir(site.path("dumps"))
            .expect("the dump root")
            .count(),
        0
    );

    let deleted = harness
        .call(Who::A1, "hosting.database.delete", json!({ "ids": [blog] }))
        .await;
    assert_eq!(data(&deleted)["deleted"], json!(1));
    eventually("hp_alice_blog is dropped", async || {
        statements(&site)
            .last()
            .is_some_and(|line| line == "DROP DATABASE IF EXISTS `hp_alice_blog`;")
    })
    .await;
    assert_eq!(
        statements(&site)[15..],
        [
            CLIENT,
            "GRANT SELECT ON `hp\\_alice\\_blog`.* TO 'hp_alice_app'@'localhost';",
            "REVOKE ALL PRIVILEGES ON `hp\\_alice\\_blog`.* FROM 'hp_alice_app'@'localhost';",
            CLIENT,
            "DROP DATABASE IF EXISTS `hp_alice_blog`;",
        ]
    );
    harness.finish().await;
}

#[tokio::test]
async fn a_name_outside_the_account_prefix_is_refused() {
    let site = Site::new();
    let harness = panel(&site).await;
    let alice = create(&harness, Who::A1, "alice").await;
    let bob = create(&harness, Who::A2, "bob").await;
    state_becomes(&harness, &alice, "active").await;
    state_becomes(&harness, &bob, "active").await;

    for name in ["hp_bob_wp", "hp_nobody_wp", "hp_alice_w_p"] {
        let reply = harness
            .call(Who::A1, "hosting.database.create", json!({ "name": name }))
            .await;
        refused_name(&reply, "hosting.database.error.prefix");
        let reply = harness
            .call(
                Who::A1,
                "hosting.database.user.create",
                json!({ "name": name, "password": "long enough" }),
            )
            .await;
        refused_name(&reply, "hosting.database.error.prefix");
    }
    for name in [
        "wp",
        "mysql",
        "hp_alice_wp`; DROP DATABASE mysql; --",
        "hp_alice_WP",
    ] {
        let reply = harness
            .call(Who::A1, "hosting.database.create", json!({ "name": name }))
            .await;
        assert_eq!(
            reply.error().expect("the name is refused").code,
            ErrorCode::Validation,
            "{name}"
        );
    }
    let theirs = created(
        &harness,
        Who::A2,
        "hosting.database.create",
        json!({ "name": "hp_bob_wp" }),
    )
    .await;
    let reply = harness
        .call(
            Who::A1,
            "hosting.database.user.create",
            json!({ "name": "hp_alice_app", "password": "long enough", "databases": [theirs] }),
        )
        .await;
    let error = reply.error().expect("a foreign database is refused");
    assert_eq!(error.fields[0].path, "databases", "{error:?}");
    let taken = harness
        .call(
            Who::A2,
            "hosting.database.create",
            json!({ "name": "hp_bob_wp" }),
        )
        .await;
    refused_name(&taken, "hosting.database.error.exists");
    state_of(&harness, "hosting_database", &theirs, "active").await;
    assert_eq!(
        statements(&site),
        [
            CLIENT,
            "CREATE DATABASE `hp_bob_wp` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;",
        ]
    );

    let host = harness
        .registry()
        .services()
        .get::<dyn HostExecutor>()
        .expect("the host module provides the executor");
    let origin = Origin::installation("panel", "outside-the-prefix");
    for database in ["mysql", "hp_", "hp_a`b"] {
        let refused = host
            .on(origin.clone())
            .op::<_, Value>(
                "mariadb.exec",
                &json!({ "statement": "database.drop", "database": database }),
            )
            .await
            .expect_err("the agent refuses a name outside the product prefix");
        assert_eq!(refused.key(), "host.error.policy", "{database}");
    }
    let refused = host
        .on(Origin::installation("dns", "outside-the-prefix"))
        .op::<_, Value>(
            "mariadb.exec",
            &json!({ "statement": "database.create", "database": "hp_bob_x" }),
        )
        .await
        .expect_err("a product without a user prefix reaches no database");
    assert_eq!(refused.key(), "host.error.policy");
    assert_eq!(statements(&site).len(), 2);
    harness.finish().await;
}

#[tokio::test]
async fn a_grant_reaches_no_database_of_another_account() {
    let site = Site::new();
    let harness = panel(&site).await;
    let a_b = create(&harness, Who::A1, "a_b").await;
    let aab = create(&harness, Who::A2, "aab").await;
    state_becomes(&harness, &a_b, "active").await;
    state_becomes(&harness, &aab, "active").await;

    let mine = created(
        &harness,
        Who::A1,
        "hosting.database.create",
        json!({ "name": "hp_a_b_c" }),
    )
    .await;
    state_of(&harness, "hosting_database", &mine, "active").await;
    let theirs = created(
        &harness,
        Who::A2,
        "hosting.database.create",
        json!({ "name": "hp_aab_c" }),
    )
    .await;
    state_of(&harness, "hosting_database", &theirs, "active").await;
    let login = created(
        &harness,
        Who::A1,
        "hosting.database.user.create",
        json!({ "name": "hp_a_b_x", "password": "long enough", "databases": [mine] }),
    )
    .await;
    state_of(&harness, "hosting_database_user", &login, "active").await;

    let grant = statements(&site)
        .into_iter()
        .find(|line| line.starts_with("GRANT "))
        .expect("the grant");
    assert_eq!(
        grant,
        "GRANT ALL PRIVILEGES ON `hp\\_a\\_b\\_c`.* TO 'hp_a_b_x'@'localhost';"
    );
    let scope = grant
        .split('`')
        .nth(1)
        .expect("the database scope is quoted");
    assert!(covers(scope, "hp_a_b_c"));
    assert!(!covers(scope, "hp_aab_c"));
    assert!(covers("hp_a_b_c", "hp_aab_c"));
    harness.finish().await;
}

#[tokio::test]
async fn deleting_the_account_drops_its_databases() {
    let site = Site::new();
    let harness = panel(&site).await;
    let a1 = harness.fixtures().accounts.a1;
    let alice = create(&harness, Who::A1, "alice").await;
    let bob = create(&harness, Who::A2, "bob").await;
    state_becomes(&harness, &alice, "active").await;
    state_becomes(&harness, &bob, "active").await;
    let wp = created(
        &harness,
        Who::A1,
        "hosting.database.create",
        json!({ "name": "hp_alice_wp" }),
    )
    .await;
    let app = created(
        &harness,
        Who::A1,
        "hosting.database.user.create",
        json!({ "name": "hp_alice_app", "password": PASSWORD, "databases": [wp] }),
    )
    .await;
    let theirs = created(
        &harness,
        Who::A2,
        "hosting.database.create",
        json!({ "name": "hp_bob_wp" }),
    )
    .await;
    state_of(&harness, "hosting_database", &wp, "active").await;
    state_of(&harness, "hosting_database_user", &app, "active").await;
    state_of(&harness, "hosting_database", &theirs, "active").await;

    assert_eq!(
        account(&harness, "suspend", a1).await["changed"],
        json!(true)
    );
    state_of(&harness, "hosting_database_user", &app, "suspended").await;
    eventually("hp_alice_app is locked", async || {
        statements(&site)
            .contains(&"ALTER USER 'hp_alice_app'@'localhost' ACCOUNT LOCK;".to_owned())
    })
    .await;
    assert_eq!(
        account(&harness, "resume", a1).await["changed"],
        json!(true)
    );
    state_of(&harness, "hosting_database_user", &app, "active").await;
    eventually("hp_alice_app is unlocked", async || {
        statements(&site)
            .contains(&"ALTER USER 'hp_alice_app'@'localhost' ACCOUNT UNLOCK;".to_owned())
    })
    .await;

    let queued = account(&harness, "delete", a1).await;
    let task: Id = queued["task"]["id"]
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
        data(&detail)["logTail"].to_string().contains(&format!(
            "removed 1 database(s) and 1 database user(s) of account {a1}"
        )),
        "{}",
        data(&detail)
    );
    eventually(
        "the databases and users of the account are dropped",
        async || {
            let lines = statements(&site);
            lines.contains(&"DROP USER IF EXISTS 'hp_alice_app'@'localhost';".to_owned())
                && lines.contains(&"DROP DATABASE IF EXISTS `hp_alice_wp`;".to_owned())
        },
    )
    .await;
    assert!(
        !statements(&site)
            .iter()
            .any(|line| line.contains("hp_bob_wp") && line.starts_with("DROP"))
    );
    eventually("the hosting user of the account is removed", async || {
        site.passwd_line("hp_alice").is_none()
    })
    .await;
    assert_eq!(
        rows(&harness, "SELECT name FROM hosting_database ORDER BY name").await,
        ["hp_bob_wp"]
    );
    assert!(
        rows(&harness, "SELECT name FROM hosting_database_user")
            .await
            .is_empty()
    );
    harness.finish().await;
}
