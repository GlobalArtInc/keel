use std::fs;
use std::os::unix::fs::PermissionsExt;

use keel::prelude::*;
use keel::sdk::sea_orm::{ConnectionTrait, DbBackend, Statement};
use keel::types::{ErrorCode, TaskState};
use keel_system::tasks::api::Drain;
use keel_testkit::{Harness, Who, tokio};

use crate::support::{PATIENCE, Site, create, data, eventually, panel, rows, state_becomes};

const CREATE: &str = "hosting.ftp.create";
const EDIT: &str = "hosting.ftp.edit";
const PASSWORD: &str = "correct horse battery";

async fn user(harness: &Harness, who: Who, name: &str) -> String {
    let id = create(harness, who, name).await;
    state_becomes(harness, &id, "active").await;
    id
}

async fn site_of(harness: &Harness, who: Who, user: &str, domain: &str) -> String {
    let reply = harness
        .call(
            who,
            "hosting.site.create",
            json!({ "user": user, "domain": domain, "root": "public" }),
        )
        .await;
    data(&reply)["id"].as_str().expect("an id").to_owned()
}

async fn account(harness: &Harness, who: Who, params: Value) -> String {
    let reply = harness.call(who, CREATE, params).await;
    data(&reply)["id"].as_str().expect("an id").to_owned()
}

async fn state_is(harness: &Harness, id: &str, state: &str) {
    let sql = format!("SELECT state FROM hosting_ftp_user WHERE id = '{id}'");
    eventually(
        &format!("the FTP account {id} in state {state}"),
        async || rows(harness, &sql).await == [state],
    )
    .await;
}

fn user_file(site: &Site, login: &str) -> Option<String> {
    fs::read_to_string(site.path("ftp-users").join(login)).ok()
}

fn passwd(site: &Site) -> String {
    fs::read_to_string(site.path("ftp").join("keel-panel-passwd")).unwrap_or_default()
}

fn logins(site: &Site) -> Vec<String> {
    passwd(site)
        .lines()
        .filter_map(|line| line.split(':').next())
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn an_ftp_account_is_chrooted_under_its_site() {
    let site = Site::new();
    let harness = panel(&site).await;
    let alice = user(&harness, Who::A1, "alice").await;
    let web = site_of(&harness, Who::A1, &alice, "example.com").await;
    let home = site.home("alice").display().to_string();

    let reply = harness
        .call(
            Who::A1,
            CREATE,
            json!({ "site": web, "name": "upload", "password": PASSWORD, "directory": "uploads" }),
        )
        .await;
    let created = data(&reply);
    assert_eq!(created["login"], json!("hp_upload"));
    assert_eq!(created["domain"], json!("example.com"));
    assert!(created.get("passwordHash").is_none(), "{created}");
    let id = created["id"].as_str().expect("an id").to_owned();
    state_is(&harness, &id, "active").await;

    assert_eq!(
        user_file(&site, "hp_upload").as_deref(),
        Some(format!("guest_username=hp_alice\nlocal_root={home}/public/uploads\n").as_str())
    );
    let text = passwd(&site);
    let (login, hash) = text
        .trim_end()
        .split_once(':')
        .expect("a line of the user file");
    assert_eq!(login, "hp_upload");
    assert!(hash.starts_with("$6$rounds=5000$"), "{hash}");
    assert_eq!(text.lines().count(), 1, "{text}");
    for file in [
        site.path("ftp").join("keel-panel-passwd"),
        site.path("ftp-users").join("hp_upload"),
    ] {
        let mode = fs::metadata(&file)
            .expect("a placed file")
            .permissions()
            .mode();
        assert_eq!(mode & 0o7777, 0o600, "{}", file.display());
    }
    assert!(
        site.calls()
            .iter()
            .any(|call| call == "vsftpd /etc/vsftpd.conf -v"),
        "{:?}",
        site.calls()
    );

    let edited = harness
        .call(Who::A1, EDIT, json!({ "id": id, "directory": "" }))
        .await;
    assert_eq!(data(&edited)["directory"], json!(""));
    state_is(&harness, &id, "active").await;
    assert_eq!(
        user_file(&site, "hp_upload").as_deref(),
        Some(format!("guest_username=hp_alice\nlocal_root={home}/public\n").as_str())
    );
    assert_eq!(passwd(&site), text);

    let moved = harness
        .call(
            Who::A1,
            "hosting.site.edit",
            json!({ "id": web, "root": "htdocs" }),
        )
        .await;
    data(&moved);
    eventually(
        "the chroot follows the document root of the site",
        async || {
            user_file(&site, "hp_upload")
                .is_some_and(|text| text.ends_with(&format!("local_root={home}/htdocs\n")))
        },
    )
    .await;

    let second = account(
        &harness,
        Who::A1,
        json!({ "site": web, "name": "deploy", "password": PASSWORD, "directory": "releases/current" }),
    )
    .await;
    state_is(&harness, &second, "active").await;
    assert_eq!(logins(&site), ["hp_deploy", "hp_upload"]);
    let deleted = harness
        .call(Who::A1, "hosting.ftp.delete", json!({ "ids": [id] }))
        .await;
    assert_eq!(data(&deleted)["deleted"], json!(1));
    eventually(
        "the deleted account leaves the passwd file and its user file",
        async || logins(&site) == ["hp_deploy"] && user_file(&site, "hp_upload").is_none(),
    )
    .await;

    let gone = harness
        .call(Who::A1, "hosting.site.delete", json!({ "ids": [web] }))
        .await;
    data(&gone);
    eventually("the accounts of a deleted site go with it", async || {
        passwd(&site).is_empty() && user_file(&site, "hp_deploy").is_none()
    })
    .await;
    assert!(
        rows(&harness, "SELECT id FROM hosting_ftp_user")
            .await
            .is_empty()
    );
    harness.finish().await;
}

#[tokio::test]
async fn a_directory_outside_the_site_is_refused() {
    let site = Site::new();
    let harness = panel(&site).await;
    let alice = user(&harness, Who::A1, "alice").await;
    let carol = user(&harness, Who::A2, "carol").await;
    let web = site_of(&harness, Who::A1, &alice, "example.com").await;
    let theirs = site_of(&harness, Who::A2, &carol, "example.net").await;

    for directory in [
        "../other",
        "/etc",
        "public/../../etc",
        ".ssh",
        "a//b",
        "uploads\nlocal_root=/",
    ] {
        let refused = harness
            .call(
                Who::A1,
                CREATE,
                json!({ "site": web, "name": "upload", "password": PASSWORD, "directory": directory }),
            )
            .await;
        let error = refused.error().expect("the directory is refused");
        assert_eq!(
            error.code,
            ErrorCode::Validation,
            "{directory:?}: {error:?}"
        );
        assert_eq!(
            error.fields[0].path, "directory",
            "{directory:?}: {error:?}"
        );
    }
    let foreign = harness
        .call(
            Who::A1,
            CREATE,
            json!({ "site": theirs, "name": "upload", "password": PASSWORD }),
        )
        .await;
    let error = foreign
        .error()
        .expect("a site of another account is refused");
    assert_eq!(error.code, ErrorCode::Validation, "{error:?}");
    assert_eq!(error.fields[0].path, "site", "{error:?}");
    assert_eq!(
        error.fields[0].key.as_deref(),
        Some("hosting.ftp.error.site"),
        "{error:?}"
    );
    assert!(
        rows(&harness, "SELECT id FROM hosting_ftp_user")
            .await
            .is_empty()
    );
    assert_eq!(
        fs::read_dir(site.path("ftp-users"))
            .expect("the root")
            .count(),
        0
    );
    assert_eq!(passwd(&site), "");

    let id = account(
        &harness,
        Who::A1,
        json!({ "site": web, "name": "upload", "password": PASSWORD, "directory": "uploads" }),
    )
    .await;
    state_is(&harness, &id, "active").await;
    let placed = user_file(&site, "hp_upload").expect("the placed file");
    let refused = harness
        .call(Who::A1, EDIT, json!({ "id": id, "directory": "../../.." }))
        .await;
    let error = refused.error().expect("the edit is refused");
    assert_eq!(error.code, ErrorCode::Validation, "{error:?}");
    assert_eq!(error.fields[0].path, "directory", "{error:?}");
    assert_eq!(
        rows(
            &harness,
            &format!("SELECT directory FROM hosting_ftp_user WHERE id = '{id}'")
        )
        .await,
        ["uploads"]
    );
    assert_eq!(user_file(&site, "hp_upload"), Some(placed));
    harness.finish().await;
}

#[tokio::test]
async fn the_ftp_accounts_follow_the_limit_and_the_lifecycle_of_their_account() {
    let site = Site::new();
    let harness = panel(&site).await;
    let a1 = harness.fixtures().accounts.a1;
    harness
        .db()
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO keel_limit (account_id, key, value) VALUES ($1, 'hosting.ftp.max', '1'::jsonb)",
            [a1.to_string().into()],
        ))
        .await
        .expect("the limit of one FTP account");
    let alice = user(&harness, Who::A1, "alice").await;
    let carol = user(&harness, Who::A2, "carol").await;
    let web = site_of(&harness, Who::A1, &alice, "example.com").await;
    let theirs = site_of(&harness, Who::A2, &carol, "example.net").await;
    let mine = account(
        &harness,
        Who::A1,
        json!({ "site": web, "name": "upload", "password": PASSWORD }),
    )
    .await;
    let over = harness
        .call(
            Who::A1,
            CREATE,
            json!({ "site": web, "name": "second", "password": PASSWORD }),
        )
        .await;
    let error = over.error().expect("the second account is over the limit");
    assert_eq!(error.code, ErrorCode::LimitExceeded, "{error:?}");
    assert_eq!(error.params["limit"], json!("hosting.ftp.max"));
    let taken = harness
        .call(
            Who::A2,
            CREATE,
            json!({ "site": theirs, "name": "upload", "password": PASSWORD }),
        )
        .await;
    let error = taken.error().expect("a login is taken across accounts");
    assert_eq!(error.code, ErrorCode::Validation, "{error:?}");
    assert_eq!(error.fields[0].path, "name", "{error:?}");
    assert_eq!(
        error.fields[0].key.as_deref(),
        Some("hosting.ftp.error.exists"),
        "{error:?}"
    );
    let other = account(
        &harness,
        Who::A2,
        json!({ "site": theirs, "name": "carol", "password": PASSWORD }),
    )
    .await;
    state_is(&harness, &mine, "active").await;
    state_is(&harness, &other, "active").await;
    assert_eq!(logins(&site), ["hp_carol", "hp_upload"]);

    for (verb, expected) in [
        ("suspend", vec!["hp_carol"]),
        ("resume", vec!["hp_carol", "hp_upload"]),
    ] {
        let reply = harness
            .call(
                Who::Reseller,
                &format!("keel.account.{verb}"),
                json!({ "id": a1 }),
            )
            .await;
        assert_eq!(data(&reply)["changed"], json!(true), "{verb}");
        eventually(&format!("the logins after {verb}"), async || {
            logins(&site) == expected
        })
        .await;
        state_is(&harness, &mine, "active").await;
        assert!(user_file(&site, "hp_upload").is_some(), "{verb}");
    }

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
            .contains(&format!("removed 1 FTP account(s) of account {a1}"))
    );
    eventually(
        "the account of the deleted owner leaves the user file",
        async || logins(&site) == ["hp_carol"] && user_file(&site, "hp_upload").is_none(),
    )
    .await;
    assert_eq!(
        rows(&harness, "SELECT login FROM hosting_ftp_user").await,
        ["hp_carol"]
    );
    harness.finish().await;
}

#[tokio::test]
async fn a_failing_vsftpd_check_keeps_the_previous_files_and_marks_the_account() {
    let site = Site::new();
    let harness = panel(&site).await;
    let alice = user(&harness, Who::A1, "alice").await;
    let web = site_of(&harness, Who::A1, &alice, "example.com").await;
    let first = account(
        &harness,
        Who::A1,
        json!({ "site": web, "name": "upload", "password": PASSWORD }),
    )
    .await;
    state_is(&harness, &first, "active").await;
    let before = passwd(&site);

    site.break_check("vsftpd", true);
    let second = account(
        &harness,
        Who::A1,
        json!({ "site": web, "name": "deploy", "password": PASSWORD, "directory": "releases" }),
    )
    .await;
    state_is(&harness, &second, "error").await;
    let error = rows(
        &harness,
        &format!("SELECT error FROM hosting_ftp_user WHERE id = '{second}'"),
    )
    .await;
    assert_eq!(
        error,
        ["vsftpd: [emerg] the configuration of the test is broken"]
    );
    assert_eq!(user_file(&site, "hp_deploy"), None);
    assert_eq!(passwd(&site), before);

    site.break_check("vsftpd", false);
    let edited = harness
        .call(
            Who::A1,
            EDIT,
            json!({ "id": second, "directory": "releases" }),
        )
        .await;
    data(&edited);
    state_is(&harness, &second, "active").await;
    assert_eq!(logins(&site), ["hp_deploy", "hp_upload"]);
    assert!(user_file(&site, "hp_deploy").is_some_and(|text| text.ends_with("/public/releases\n")));
    harness.finish().await;
}
