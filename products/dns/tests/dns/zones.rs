use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use keel::prelude::*;
use keel::sdk::sea_orm::{ConnectionTrait, DbBackend, Statement};
use keel::types::{ErrorCode, TaskState};
use keel_system::cron::api::{Outcome, Scheduler};
use keel_system::notify::POLL_JOB;
use keel_system::tasks::api::Drain;
use keel_testkit::{Harness, Reply, Who, tokio};

const PATIENCE: Duration = Duration::from_secs(20);
const NAMESERVER: &str = "ns1.example.net";
const FAILED: &str = "dns.zone.failed";

struct Site {
    dir: tempfile::TempDir,
}

impl Site {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a temporary directory");
        for sub in ["zones", "named", "bin"] {
            fs::create_dir(dir.path().join(sub)).expect("a directory of the site");
        }
        let site = Self { dir };
        site.stub(
            "named-checkconf",
            &format!(
                "echo \"$@\" >> {calls}\nif [ -f {flag} ]; then echo \"zone kept.example/IN: loaded serial 1\"; echo \"zone broken.example/IN: the zone of the test is broken\" >&2; exit 1; fi\nexit 0",
                calls = site.path("named-checkconf.calls").display(),
                flag = site.path("broken").display(),
            ),
        );
        site.stub(
            "systemctl",
            &format!(
                "echo \"$@\" >> {}\nexit 0",
                site.path("systemctl.calls").display()
            ),
        );
        site
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn stub(&self, name: &str, body: &str) {
        let path = self.path("bin").join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("the stub");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("the stub mode");
    }

    fn policy(&self) -> String {
        let bin = self.path("bin");
        format!(
            "policy:
  file_roots: {{ zones: {zones}, named: {named} }}
  services: [named]
  checks: {{ named.check: [\"{bin}/named-checkconf\", \"-z\"] }}
  products:
    dns: {{ drop_in_prefix: keel-dns- }}
  tools: {{ systemctl: {bin}/systemctl }}
",
            zones = self.path("zones").display(),
            named = self.path("named").display(),
            bin = bin.display(),
        )
    }

    fn section(&self) -> Value {
        json!({
            "zones_dir": self.path("zones").display().to_string(),
            "include_dir": self.path("named").display().to_string(),
            "nameserver": NAMESERVER,
            "hostmaster": "hostmaster.example.net",
        })
    }

    fn zone_file(&self, zone: &str) -> PathBuf {
        self.path("zones").join(format!("keel-dns-{zone}.zone"))
    }

    fn include(&self, zone: &str) -> PathBuf {
        self.path("named").join(format!("keel-dns-{zone}.conf"))
    }

    fn index(&self) -> String {
        fs::read_to_string(self.path("named").join("keel-dns-zones.conf")).unwrap_or_default()
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.path(name)).unwrap_or_default()
    }

    fn fail_checks(&self, broken: bool) {
        let flag = self.path("broken");
        if broken {
            fs::write(flag, "").expect("the flag");
        } else {
            fs::remove_file(flag).expect("the flag is removed");
        }
    }
}

async fn dns(site: &Site) -> Harness {
    Harness::new()
        .product(&keel_dns::MANIFEST)
        .module(&keel_system::tasks::MODULE)
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .section("dns", site.section())
        .agent(&site.policy())
        .boot()
        .await
}

fn data(reply: &Reply) -> &Value {
    reply
        .data()
        .unwrap_or_else(|| panic!("a result was expected, got {:?}", reply.error()))
}

async fn create(harness: &Harness, who: Who, name: &str) -> String {
    let reply = harness
        .call(who, "dns.zone.create", json!({ "name": name }))
        .await;
    data(&reply)["id"].as_str().expect("an id").to_owned()
}

async fn rows(harness: &Harness, sql: &str) -> Vec<String> {
    harness.database().rows(sql).await
}

async fn eventually<F>(what: &str, mut done: F)
where
    F: AsyncFnMut() -> bool,
{
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while !done().await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what} did not happen within {PATIENCE:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn state_becomes(harness: &Harness, id: &str, state: &str) {
    let sql = format!("SELECT state FROM dns_zone WHERE id = '{id}'");
    eventually(&format!("the zone {id} in state {state}"), async || {
        rows(harness, &sql).await == [state]
    })
    .await;
}

async fn run_poll(harness: &Harness) {
    let scheduler = harness
        .registry()
        .services()
        .get::<Scheduler>()
        .expect("the cron module provides the scheduler");
    eventually("the leader takes the lock", async || scheduler.is_leader()).await;
    let started = scheduler.run_now(POLL_JOB).await.expect("the poll starts");
    let outcome = tokio::time::timeout(PATIENCE, scheduler.wait(started.run))
        .await
        .expect("the poll ends in time")
        .expect("the outcome is read");
    assert_eq!(outcome, Outcome::Ok);
}

fn names(reply: &Reply) -> Vec<String> {
    data(reply)["data"]
        .as_array()
        .expect("a page")
        .iter()
        .map(|row| row["name"].as_str().expect("a name").to_owned())
        .collect()
}

#[tokio::test]
async fn a_zone_created_by_a_user_renders_its_file_and_include_after_commit_and_lists_under_its_account()
 {
    let site = Site::new();
    let harness = dns(&site).await;
    let a1 = harness.fixtures().accounts.a1;

    let reply = harness
        .call(Who::A1, "dns.zone.create", json!({ "name": "Example.ORG" }))
        .await;
    let created = data(&reply).clone();
    assert_eq!(created["name"], json!("example.org"));
    assert_eq!(created["accountId"], json!(a1.to_string()));
    assert_eq!(created["kind"], json!("primary"));
    assert_eq!(created["state"], json!("pending"));
    assert_eq!(created["served"], json!(false));
    let id = created["id"].as_str().expect("an id").to_owned();
    let serial = created["serial"].as_i64().expect("a serial");

    state_becomes(&harness, &id, "active").await;
    let zone = fs::read_to_string(site.zone_file("example.org")).expect("the zone file");
    assert!(zone.starts_with("$ORIGIN example.org.\n"), "{zone}");
    assert!(
        zone.contains(&format!(
            "@ IN SOA {NAMESERVER}. hostmaster.example.net. ( {serial} 10800 3600 604800 3600 )"
        )),
        "{zone}"
    );
    assert!(zone.contains(&format!("@ IN NS {NAMESERVER}.")), "{zone}");
    let include = fs::read_to_string(site.include("example.org")).expect("the include");
    assert!(include.contains("zone \"example.org\" {"), "{include}");
    assert!(
        include.contains(&format!(
            "file \"{}\";",
            site.zone_file("example.org").display()
        )),
        "{include}"
    );
    assert_eq!(
        site.index(),
        format!("include \"{}\";\n", site.include("example.org").display())
    );
    assert!(site.read("systemctl.calls").contains("reload named"));
    assert!(site.read("named-checkconf.calls").contains("-z"));
    assert_eq!(
        rows(
            &harness,
            &format!("SELECT served::text FROM dns_zone WHERE id = '{id}'")
        )
        .await,
        ["true"]
    );

    let own = harness.call(Who::A1, "dns.zone.list", json!({})).await;
    assert_eq!(names(&own), ["example.org"]);
    assert_eq!(data(&own)["data"][0]["state"], json!("active"));
    let above = harness
        .call(Who::Reseller, "dns.zone.list", json!({}))
        .await;
    assert_eq!(names(&above), ["example.org"]);
    let other = harness.call(Who::A2, "dns.zone.list", json!({})).await;
    assert!(names(&other).is_empty());
    let hidden = harness
        .call(Who::A2, "dns.zone.get", json!({ "id": id }))
        .await;
    assert_eq!(
        hidden.error().map(|error| error.code),
        Some(ErrorCode::NotFound)
    );
    let taken = harness
        .call(Who::A2, "dns.zone.create", json!({ "name": "example.org" }))
        .await;
    let error = taken.error().expect("the name is taken");
    assert_eq!(error.fields[0].path, "name");
    assert_eq!(
        error.fields[0].key.as_deref(),
        Some("dns.zone.error.exists")
    );

    let edited = harness
        .call(
            Who::A1,
            "dns.zone.edit",
            json!({ "id": id, "refresh": 7200, "retry": 1800, "expire": 1209600, "minimum": 300 }),
        )
        .await;
    assert_eq!(data(&edited)["serial"], json!(serial + 1));
    eventually("the edit reaches the zone file", async || {
        fs::read_to_string(site.zone_file("example.org"))
            .is_ok_and(|zone| zone.contains(&format!("( {} 7200 1800 1209600 300 )", serial + 1)))
    })
    .await;
    state_becomes(&harness, &id, "active").await;

    let backup = tempfile::tempdir().expect("a backup directory");
    let source = harness
        .registry()
        .contributions()
        .of::<BackupSource>()
        .find(|source| source.name() == "dns.zonefiles")
        .expect("the zone files are a backup source")
        .write_hook();
    source(BackupDir::new(
        "dns.zonefiles",
        backup.path().to_owned(),
        Db::from_connection(harness.db().clone()),
        harness.data_dir(),
    ))
    .await
    .expect("the zone files are copied");
    assert_eq!(
        fs::read_to_string(backup.path().join("keel-dns-example.org.zone")).expect("the copy"),
        fs::read_to_string(site.zone_file("example.org")).expect("the zone file")
    );
    harness.finish().await;
}

#[tokio::test]
async fn a_zone_over_the_account_limit_is_refused_before_the_transaction() {
    let site = Site::new();
    let harness = dns(&site).await;
    let a1 = harness.fixtures().accounts.a1;
    harness
        .db()
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO keel_limit (account_id, key, value) VALUES ($1, 'dns.zones', '1'::jsonb)",
            [a1.to_string().into()],
        ))
        .await
        .expect("the limit of one zone");

    let first = create(&harness, Who::A1, "first.example").await;
    state_becomes(&harness, &first, "active").await;
    let refused = harness
        .call(
            Who::A1,
            "dns.zone.create",
            json!({ "name": "second.example" }),
        )
        .await;
    let error = refused.error().expect("the second zone is over the limit");
    assert_eq!(error.code, ErrorCode::LimitExceeded);
    assert_eq!(error.key, "keel.error.limit_exceeded");
    assert_eq!(error.params["limit"], json!("dns.zones"));
    assert_eq!(error.params["max"], json!(1));
    let theirs = create(&harness, Who::A2, "second.example").await;
    state_becomes(&harness, &theirs, "active").await;

    assert_eq!(
        rows(
            &harness,
            "SELECT name || ' ' || account_id FROM dns_zone ORDER BY name"
        )
        .await,
        [
            format!("first.example {a1}"),
            format!("second.example {}", harness.fixtures().accounts.a2),
        ]
    );
    let requests = site.read("named-checkconf.calls").lines().count();
    assert_eq!(
        requests, 6,
        "three checked applies for each of the two zones"
    );
    harness.finish().await;
}

#[tokio::test]
async fn a_failing_check_leaves_the_row_but_marks_the_zone_with_the_error_and_the_problem() {
    let site = Site::new();
    let harness = dns(&site).await;
    let a1 = harness.fixtures().accounts.a1;
    let kept = create(&harness, Who::A1, "kept.example").await;
    state_becomes(&harness, &kept, "active").await;

    site.fail_checks(true);
    let broken = create(&harness, Who::A1, "broken.example").await;
    state_becomes(&harness, &broken, "error").await;
    let marked = rows(
        &harness,
        &format!("SELECT served::text || ' ' || error FROM dns_zone WHERE id = '{broken}'"),
    )
    .await;
    assert!(
        marked[0].starts_with("false ") && marked[0].contains("the zone of the test is broken"),
        "{marked:?}"
    );
    assert!(!marked[0].contains("kept.example"), "{marked:?}");
    assert_eq!(
        site.index(),
        format!("include \"{}\";\n", site.include("kept.example").display())
    );
    assert!(!site.zone_file("broken.example").exists());
    let listed = harness.call(Who::A1, "dns.zone.list", json!({})).await;
    assert_eq!(names(&listed), ["broken.example", "kept.example"]);

    run_poll(&harness).await;
    assert_eq!(
        rows(
            &harness,
            "SELECT key || ' ' || subject || ' ' || severity || ' ' || (params_json->>'name') || ' ' || coalesce(account_id, '-') FROM notify_problem WHERE cleared_at IS NULL"
        )
        .await,
        [format!("{FAILED} {broken} critical broken.example {a1}")]
    );

    site.fail_checks(false);
    let fixed = harness
        .call(
            Who::A1,
            "dns.zone.edit",
            json!({ "id": broken, "refresh": 10800, "retry": 3600, "expire": 604800, "minimum": 3600 }),
        )
        .await;
    assert!(fixed.data().is_some(), "{:?}", fixed.error());
    state_becomes(&harness, &broken, "active").await;
    assert!(site.index().contains("keel-dns-broken.example.conf"));
    run_poll(&harness).await;
    assert!(
        rows(
            &harness,
            "SELECT subject FROM notify_problem WHERE cleared_at IS NULL"
        )
        .await
        .is_empty()
    );
    harness.finish().await;
}

#[tokio::test]
async fn deleting_the_account_removes_its_zones_and_their_files() {
    let site = Site::new();
    let harness = dns(&site).await;
    let a1 = harness.fixtures().accounts.a1;
    let mine = [
        create(&harness, Who::A1, "one.example").await,
        create(&harness, Who::A1, "two.example").await,
    ];
    let theirs = create(&harness, Who::A2, "three.example").await;
    for id in mine.iter().chain([&theirs]) {
        state_becomes(&harness, id, "active").await;
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
            .contains(&format!("dropped 2 zone(s) of account {a1}"))
    );

    eventually("the files of the deleted account are removed", async || {
        ["one.example", "two.example"]
            .iter()
            .all(|zone| !site.zone_file(zone).exists() && !site.include(zone).exists())
    })
    .await;
    assert_eq!(
        site.index(),
        format!("include \"{}\";\n", site.include("three.example").display())
    );
    assert!(site.zone_file("three.example").exists());
    assert_eq!(
        rows(&harness, "SELECT name FROM dns_zone").await,
        ["three.example"]
    );
    assert!(
        rows(
            &harness,
            &format!("SELECT id FROM keel_account WHERE id = '{a1}'")
        )
        .await
        .is_empty()
    );
    harness.finish().await;
}
