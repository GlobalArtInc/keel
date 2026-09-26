use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use keel::prelude::*;
use keel::sdk::chrono::{DateTime, Duration, Utc};
use keel::sdk::serde_json;
use keel_system::cron::api::{Outcome, Scheduler};
use keel_testkit::{Harness, Reply, Who, tokio};
use tar::{Archive, EntryType};

use crate::mail::mail;
use crate::support::{Site, composed, create, data, eventually, rows, state_becomes};

const DOMAIN: &str = "shop.example.org";
const ADDRESS: &str = "info@shop.example.org";
const DATABASE: &str = "hp_shop_wp";
const PASSWORD: &str = "correct horse battery";
const SCHEDULED_JOB: &str = "hosting.backup.scheduled";
const DOWNLOAD_ROUTE: &str = "/api/v1/x/hosting_backup/download";

async fn panel(site: &Site, at: Option<DateTime<Utc>>) -> Harness {
    let builder = mail(site, composed(site)).section(
        "hosting_backup",
        json!({ "archives_dir": site.path("backups").display().to_string() }),
    );
    match at {
        Some(at) => builder.clock(at),
        None => builder,
    }
    .boot()
    .await
}

async fn created(harness: &Harness, function: &str, params: Value) -> String {
    let reply = harness.call(Who::A1, function, params).await;
    data(&reply)["id"].as_str().expect("an id").to_owned()
}

async fn state_is(harness: &Harness, table: &str, id: &str, state: &str) {
    let sql = format!("SELECT state FROM {table} WHERE id = '{id}'");
    eventually(&format!("{table} {id} in state {state}"), async || {
        rows(harness, &sql).await == [state]
    })
    .await;
}

async fn hosted(harness: &Harness, site: &Site) -> (String, PathBuf) {
    let user = create(harness, Who::A1, "shop").await;
    state_becomes(harness, &user, "active").await;
    let id = created(
        harness,
        "hosting.site.create",
        json!({ "user": user, "domain": DOMAIN, "root": "public" }),
    )
    .await;
    state_is(harness, "hosting_site", &id, "active").await;
    let root = site.home("shop").join("public");
    fs::create_dir_all(root.join("css")).expect("the document root");
    fs::write(root.join("index.php"), "<?php echo 'shop';").expect("an index");
    fs::write(root.join("css/site.css"), "body { color: navy }").expect("a stylesheet");
    symlink("index.php", root.join("home.php")).expect("a link");
    (id, root)
}

async fn finished(harness: &Harness, reply: &Reply) -> Value {
    let task = data(reply)["task"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("a task was expected, got {:?}", reply.data()))
        .to_owned();
    let sql = format!(
        "SELECT state || ' ' || coalesce(result_json::text, error_json::text, '') FROM task_job WHERE id = '{task}'"
    );
    let mut seen = String::new();
    eventually(&format!("the task {task} ends"), async || {
        seen = rows(harness, &sql).await.join("");
        seen.starts_with("done ") || seen.starts_with("failed ")
    })
    .await;
    let result = seen
        .strip_prefix("done ")
        .unwrap_or_else(|| panic!("the task {task} failed: {seen}"));
    serde_json::from_str(result).expect("the task result")
}

async fn backed_up(harness: &Harness, site: &str) -> Value {
    let reply = harness
        .call(Who::A1, "hosting.backup.create", json!({ "site": site }))
        .await;
    finished(harness, &reply).await
}

fn entries(archive: &Path) -> BTreeMap<String, (EntryType, Vec<u8>)> {
    let file = fs::File::open(archive).expect("the archive");
    let mut archive = Archive::new(zstd::Decoder::new(file).expect("a zstd stream"));
    archive
        .entries()
        .expect("the entries")
        .map(|entry| {
            let mut entry = entry.expect("an entry");
            let path = entry.path().expect("a path").display().to_string();
            let kind = entry.header().entry_type();
            let mut content = Vec::new();
            if kind == EntryType::Symlink {
                content = entry
                    .link_name_bytes()
                    .map(|link| link.into_owned())
                    .unwrap_or_default();
            } else {
                entry.read_to_end(&mut content).expect("the content");
            }
            (path.trim_end_matches('/').to_owned(), (kind, content))
        })
        .collect()
}

fn archive(site: &Site, id: &str) -> PathBuf {
    site.path("backups")
        .join(format!("keel-panel-{id}.tar.zst"))
}

fn leftovers(site: &Site, root: &str) -> Vec<String> {
    fs::read_dir(site.path(root))
        .expect("the root")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

async fn nudge(harness: &Harness, job: &str) {
    let scheduler = harness
        .registry()
        .services()
        .get::<Scheduler>()
        .expect("the cron module provides the scheduler");
    let waiting = Arc::clone(&scheduler);
    eventually("the cron leader takes the lock", async || {
        waiting.is_leader()
    })
    .await;
    if let Ok(started) = scheduler.run_now(job).await {
        assert_eq!(
            scheduler.wait(started.run).await.expect("the run ends"),
            Outcome::Ok,
            "{job}"
        );
    }
}

#[tokio::test]
async fn a_site_backup_contains_files_databases_and_mail_and_restores() {
    let site = Site::new();
    let harness = panel(&site, None).await;
    let (site_id, root) = hosted(&harness, &site).await;
    let database = created(
        &harness,
        "hosting.database.create",
        json!({ "name": DATABASE }),
    )
    .await;
    state_is(&harness, "hosting_database", &database, "active").await;
    let domain = created(
        &harness,
        "hosting.mail.domain.create",
        json!({ "name": DOMAIN }),
    )
    .await;
    state_is(&harness, "hosting_mail_domain", &domain, "active").await;
    let mailbox = created(
        &harness,
        "hosting.mail.mailbox.create",
        json!({ "domain": domain, "local": "info", "password": PASSWORD }),
    )
    .await;
    state_is(&harness, "hosting_mailbox", &mailbox, "active").await;
    let maildir = site.path("maildirs").join(format!("keel-panel-{mailbox}"));
    fs::create_dir_all(maildir.join("Maildir/cur")).expect("a maildir");
    fs::write(
        maildir.join("Maildir/cur/1.eml"),
        "Subject: order 1\n\npaid\n",
    )
    .expect("a message");

    let backup = backed_up(&harness, &site_id).await;
    let id = backup["id"].as_str().expect("the backup id").to_owned();
    assert_eq!(backup["state"], "active", "{backup}");
    assert_eq!(backup["domain"], DOMAIN);
    assert_eq!(backup["databases"], json!([DATABASE]));
    assert_eq!(backup["mailboxes"], json!([ADDRESS]));
    assert_eq!(backup["scheduled"], false);
    assert!(backup.get("file").is_none(), "{backup}");
    let packed = entries(&archive(&site, &id));
    let content = |path: &str| {
        packed
            .get(path)
            .map(|(_, bytes)| String::from_utf8_lossy(bytes).into_owned())
            .unwrap_or_else(|| panic!("{path} is not in {:?}", packed.keys()))
    };
    assert_eq!(content("files/index.php"), "<?php echo 'shop';");
    assert_eq!(content("files/css/site.css"), "body { color: navy }");
    assert_eq!(packed["files/home.php"].0, EntryType::Symlink);
    assert_eq!(content("files/home.php"), "index.php");
    assert_eq!(
        content(&format!("databases/{DATABASE}.sql")),
        format!("-- dump of {DATABASE}\n")
    );
    assert_eq!(
        content(&format!("mail/{ADDRESS}/Maildir/cur/1.eml")),
        "Subject: order 1\n\npaid\n"
    );
    let manifest: Value = serde_json::from_str(&content("manifest.json")).expect("a manifest");
    assert_eq!(manifest["domain"], DOMAIN);
    assert_eq!(
        backup["bytes"],
        json!(
            fs::metadata(archive(&site, &id))
                .expect("the archive")
                .len()
        )
    );
    assert!(
        site.calls()
            .iter()
            .any(|call| call.starts_with("mariadb-dump ") && call.ends_with(DATABASE)),
        "{:?}",
        site.calls()
    );
    assert!(
        leftovers(&site, "dumps").is_empty(),
        "{:?}",
        leftovers(&site, "dumps")
    );

    let http = harness.http().await;
    let answer = http
        .get(Who::A1, &format!("{DOWNLOAD_ROUTE}?id={id}"))
        .await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    let disposition = answer.header("content-disposition").unwrap_or_default();
    assert!(
        disposition.contains(&format!("{DOMAIN}-")) && disposition.contains(".tar.zst"),
        "{disposition}"
    );
    let unrelated = http
        .get(Who::Unrelated, &format!("{DOWNLOAD_ROUTE}?id={id}"))
        .await;
    assert_eq!(unrelated.status, 404, "{}", unrelated.body);
    let refused = harness
        .call(
            Who::Unrelated,
            "hosting.backup.restore",
            json!({ "id": id }),
        )
        .await;
    assert!(refused.error().is_some(), "{:?}", refused.data());

    fs::write(root.join("index.php"), "<?php echo 'defaced';").expect("a changed index");
    fs::remove_dir_all(root.join("css")).expect("the stylesheets are gone");
    fs::remove_file(root.join("home.php")).expect("the link is gone");
    fs::remove_file(maildir.join("Maildir/cur/1.eml")).expect("the message is gone");
    let before = site.calls().len();
    let reply = harness
        .call(Who::A1, "hosting.backup.restore", json!({ "id": id }))
        .await;
    let restored = finished(&harness, &reply).await;
    assert_eq!(restored["files"], 4, "{restored}");
    assert_eq!(restored["databases"], json!([DATABASE]));
    assert_eq!(restored["mailboxes"], json!([ADDRESS]));
    assert_eq!(
        fs::read_to_string(root.join("index.php")).expect("the index"),
        "<?php echo 'shop';"
    );
    assert_eq!(
        fs::read_to_string(root.join("css/site.css")).expect("the stylesheet"),
        "body { color: navy }"
    );
    assert_eq!(
        fs::read_link(root.join("home.php")).expect("the link"),
        Path::new("index.php")
    );
    assert_eq!(
        fs::read_to_string(maildir.join("Maildir/cur/1.eml")).expect("the message"),
        "Subject: order 1\n\npaid\n"
    );
    let calls = site.calls()[before..].to_vec();
    let import = calls
        .iter()
        .position(|call| {
            call == &format!("mariadb --batch --sandbox --skip-column-names {DATABASE}")
        })
        .unwrap_or_else(|| panic!("no import in {calls:?}"));
    assert_eq!(calls[import + 1], format!("-- dump of {DATABASE}"));
    assert!(
        leftovers(&site, "dumps").is_empty(),
        "{:?}",
        leftovers(&site, "dumps")
    );
    harness.finish().await;
}

#[tokio::test]
async fn retention_keeps_the_newest_archives() {
    let site = Site::new();
    let harness = panel(&site, None).await;
    let (site_id, _) = hosted(&harness, &site).await;
    let schedule = harness
        .call(
            Who::A1,
            "hosting.backup.schedule.create",
            json!({ "site": site_id, "schedule": "0 3 * * *", "keep": 2 }),
        )
        .await;
    assert_eq!(data(&schedule)["keep"], 2);

    let mut taken = Vec::new();
    for _ in 0..3 {
        let backup = backed_up(&harness, &site_id).await;
        taken.push(backup["id"].as_str().expect("an id").to_owned());
        harness.clock().advance(Duration::minutes(1));
    }
    let kept = rows(
        &harness,
        "SELECT id FROM hosting_backup ORDER BY created_at, id",
    )
    .await;
    assert_eq!(kept, taken[1..]);
    assert!(!archive(&site, &taken[0]).exists());
    for id in &taken[1..] {
        assert!(archive(&site, id).exists(), "{id}");
    }
    let listed = harness
        .call(Who::A1, "hosting.backup.list", json!({}))
        .await;
    assert_eq!(data(&listed)["total"], 2, "{:?}", listed.data());

    let deleted = harness
        .call(
            Who::A1,
            "hosting.backup.delete",
            json!({ "ids": [taken[2]] }),
        )
        .await;
    assert_eq!(data(&deleted)["deleted"], 1);
    eventually("the deleted archive leaves the backups root", async || {
        !archive(&site, &taken[2]).exists()
    })
    .await;

    let removed = harness
        .call(Who::A1, "hosting.site.delete", json!({ "ids": [site_id] }))
        .await;
    data(&removed);
    eventually(
        "the archives of a deleted site leave the backups root with their digests",
        async || leftovers(&site, "backups").is_empty(),
    )
    .await;
    assert!(
        rows(
            &harness,
            "SELECT id FROM hosting_backup UNION ALL SELECT id FROM hosting_backup_schedule"
        )
        .await
        .is_empty()
    );
    harness.finish().await;
}

#[tokio::test]
async fn a_due_schedule_is_backed_up_by_the_cron_job_and_moves_to_its_next_run() {
    let site = Site::new();
    let start: DateTime<Utc> = "2026-09-26T02:30:00Z".parse().expect("an instant");
    let harness = panel(&site, Some(start)).await;
    let (site_id, _) = hosted(&harness, &site).await;
    let refused = harness
        .call(
            Who::A1,
            "hosting.backup.schedule.create",
            json!({ "site": site_id, "schedule": "every night" }),
        )
        .await;
    let error = refused.error().expect("an unparsable schedule is refused");
    assert!(
        error.fields.iter().any(|field| field.path == "schedule"
            && field.key.as_deref() == Some("hosting.backup.schedule.error.expression")),
        "{error:?}"
    );
    let schedule = created(
        &harness,
        "hosting.backup.schedule.create",
        json!({ "site": site_id, "schedule": "0 3 * * *" }),
    )
    .await;
    let next = format!(
        "SELECT to_char(next_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS') || ' ' || keep FROM hosting_backup_schedule WHERE id = '{schedule}'"
    );
    assert_eq!(rows(&harness, &next).await, ["2026-09-26T03:00:00 7"]);

    nudge(&harness, SCHEDULED_JOB).await;
    assert!(
        rows(&harness, "SELECT id FROM hosting_backup")
            .await
            .is_empty()
    );

    harness
        .clock()
        .set("2026-09-26T03:00:00Z".parse().expect("an instant"));
    nudge(&harness, SCHEDULED_JOB).await;
    eventually("the scheduled backup is taken", async || {
        rows(&harness, "SELECT state FROM hosting_backup WHERE scheduled").await == ["active"]
    })
    .await;
    assert_eq!(rows(&harness, &next).await, ["2026-09-27T03:00:00 7"]);
    harness.finish().await;
}
