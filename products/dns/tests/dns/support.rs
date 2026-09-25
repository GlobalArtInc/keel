use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use keel::prelude::*;
use keel_system::cron::api::{Outcome, Scheduler};
use keel_system::notify::POLL_JOB;
use keel_testkit::{Builder, Harness, Reply, Who, tokio};

pub(crate) const PATIENCE: Duration = Duration::from_secs(20);
pub(crate) const NAMESERVER: &str = "ns1.example.net";
pub(crate) const FAILED: &str = "dns.zone.failed";

pub(crate) struct Site {
    dir: tempfile::TempDir,
}

impl Site {
    pub(crate) fn new() -> Self {
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

    pub(crate) fn path(&self, name: &str) -> PathBuf {
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

    pub(crate) fn zone_file(&self, zone: &str) -> PathBuf {
        self.path("zones").join(format!("keel-dns-{zone}.zone"))
    }

    pub(crate) fn include(&self, zone: &str) -> PathBuf {
        self.path("named").join(format!("keel-dns-{zone}.conf"))
    }

    pub(crate) fn index(&self) -> String {
        fs::read_to_string(self.path("named").join("keel-dns-zones.conf")).unwrap_or_default()
    }

    pub(crate) fn read(&self, name: &str) -> String {
        fs::read_to_string(self.path(name)).unwrap_or_default()
    }

    pub(crate) fn fail_checks(&self, broken: bool) {
        let flag = self.path("broken");
        if broken {
            fs::write(flag, "").expect("the flag");
        } else {
            fs::remove_file(flag).expect("the flag is removed");
        }
    }
}

pub(crate) async fn dns(site: &Site) -> Harness {
    composed(site).boot().await
}

pub(crate) fn composed(site: &Site) -> Builder {
    Harness::new()
        .product(&keel_dns::MANIFEST)
        .module(&keel_system::tasks::MODULE)
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .section("dns", site.section())
        .agent(&site.policy())
}

pub(crate) fn data(reply: &Reply) -> &Value {
    reply
        .data()
        .unwrap_or_else(|| panic!("a result was expected, got {:?}", reply.error()))
}

pub(crate) async fn create(harness: &Harness, who: Who, name: &str) -> String {
    let reply = harness
        .call(who, "dns.zone.create", json!({ "name": name }))
        .await;
    data(&reply)["id"].as_str().expect("an id").to_owned()
}

pub(crate) async fn rows(harness: &Harness, sql: &str) -> Vec<String> {
    harness.database().rows(sql).await
}

pub(crate) async fn eventually<F>(what: &str, mut done: F)
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

pub(crate) async fn zone_file_has(site: &Site, zone: &str, lines: &[String]) {
    eventually(
        &format!("the zone file of {zone} with {lines:?}"),
        async || {
            fs::read_to_string(site.zone_file(zone))
                .is_ok_and(|file| lines.iter().all(|line| file.lines().any(|at| at == line)))
        },
    )
    .await;
}

pub(crate) async fn state_becomes(harness: &Harness, id: &str, state: &str) {
    let sql = format!("SELECT state FROM dns_zone WHERE id = '{id}'");
    eventually(&format!("the zone {id} in state {state}"), async || {
        rows(harness, &sql).await == [state]
    })
    .await;
}

pub(crate) async fn run_poll(harness: &Harness) {
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

pub(crate) fn names(reply: &Reply) -> Vec<String> {
    data(reply)["data"]
        .as_array()
        .expect("a page")
        .iter()
        .map(|row| row["name"].as_str().expect("a name").to_owned())
        .collect()
}
