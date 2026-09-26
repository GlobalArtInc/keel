use keel_sdk::prelude::*;

use super::Host;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(crate = "keel_sdk::serde")]
pub struct ServiceState {
    pub name: String,
    pub state: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(crate = "keel_sdk::serde")]
pub struct ServiceStatus {
    pub name: String,
    pub state: String,
    pub installed: bool,
    pub memory: Option<u64>,
    pub since: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(crate = "keel_sdk::serde")]
pub struct PackageChange {
    pub name: String,
    pub from: Option<String>,
    pub to: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(crate = "keel_sdk::serde")]
pub struct PackageChanges {
    pub changed: bool,
    pub changes: Vec<PackageChange>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(crate = "keel_sdk::serde")]
pub struct CronJob {
    pub schedule: String,
    pub user: String,
    pub exec: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(crate = "keel_sdk::serde")]
pub struct Limits {
    pub soft: u64,
    pub hard: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(crate = "keel_sdk::serde")]
pub struct Usage {
    pub used: u64,
    pub soft: u64,
    pub hard: u64,
    pub grace: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(crate = "keel_sdk::serde")]
pub struct UserQuota {
    pub user: String,
    pub blocks: Usage,
    pub inodes: Usage,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(crate = "keel_sdk::serde")]
pub struct QuotaReport {
    pub filesystem: String,
    pub quotas: Vec<UserQuota>,
}

#[derive(Deserialize)]
#[serde(crate = "keel_sdk::serde")]
struct ServiceList {
    services: Vec<ServiceState>,
}

#[derive(Deserialize)]
#[serde(crate = "keel_sdk::serde")]
struct ServiceStatuses {
    services: Vec<ServiceStatus>,
}

#[derive(Deserialize)]
#[serde(crate = "keel_sdk::serde")]
struct Version {
    version: Option<String>,
}

#[derive(Deserialize)]
#[serde(crate = "keel_sdk::serde")]
struct Changed {
    changed: bool,
}

#[derive(Deserialize)]
#[serde(crate = "keel_sdk::serde")]
struct Removed {
    removed: bool,
}

#[derive(Deserialize)]
#[serde(crate = "keel_sdk::serde")]
struct Entries {
    entries: Vec<String>,
}

pub struct Services<'a> {
    host: &'a Host<'a>,
}

impl<'a> Services<'a> {
    pub(super) fn new(host: &'a Host<'a>) -> Self {
        Self { host }
    }

    pub async fn reload(&self, name: &str, validate: Option<&str>) -> Result<()> {
        self.act("host.service.reload", name, validate).await
    }

    pub async fn restart(&self, name: &str, validate: Option<&str>) -> Result<()> {
        self.act("host.service.restart", name, validate).await
    }

    pub async fn start(&self, name: &str, validate: Option<&str>) -> Result<()> {
        self.act("host.service.start", name, validate).await
    }

    pub async fn stop(&self, name: &str) -> Result<()> {
        self.act("host.service.stop", name, None).await
    }

    pub async fn list(&self) -> Result<Vec<ServiceState>> {
        let answer: ServiceList = self.host.call("host.service.list", json!({})).await?;
        Ok(answer.services)
    }

    pub async fn status(&self) -> Result<Vec<ServiceStatus>> {
        let answer: ServiceStatuses = self.host.call("host.service.status", json!({})).await?;
        Ok(answer.services)
    }

    async fn act(&self, op: &str, name: &str, validate: Option<&str>) -> Result<()> {
        let mut args = json!({ "name": name });
        if let Some(check) = validate {
            args["validate"] = json!(check);
        }
        self.host.call::<Value>(op, args).await.map(drop)
    }
}

pub struct Packages<'a> {
    host: &'a Host<'a>,
}

impl<'a> Packages<'a> {
    pub(super) fn new(host: &'a Host<'a>) -> Self {
        Self { host }
    }

    pub async fn install(&self, packages: &[&str]) -> Result<PackageChanges> {
        self.host
            .call("host.pkg.install", json!({ "packages": packages }))
            .await
    }

    pub async fn remove(&self, packages: &[&str]) -> Result<PackageChanges> {
        self.host
            .call("host.pkg.remove", json!({ "packages": packages }))
            .await
    }

    pub async fn get(&self, name: &str) -> Result<Option<String>> {
        let answer: Version = self
            .host
            .call("host.pkg.get", json!({ "name": name }))
            .await?;
        Ok(answer.version)
    }

    pub async fn refresh(&self) -> Result<()> {
        self.host
            .call::<Value>("host.pkg.refresh", json!({}))
            .await
            .map(drop)
    }
}

pub struct Cron<'a> {
    host: &'a Host<'a>,
}

impl<'a> Cron<'a> {
    pub(super) fn new(host: &'a Host<'a>) -> Self {
        Self { host }
    }

    pub async fn write(&self, name: &str, jobs: &[CronJob]) -> Result<bool> {
        let answer: Changed = self
            .host
            .call("host.cron.write", json!({ "name": name, "jobs": jobs }))
            .await?;
        Ok(answer.changed)
    }

    pub async fn remove(&self, name: &str) -> Result<bool> {
        let answer: Removed = self
            .host
            .call("host.cron.remove", json!({ "name": name }))
            .await?;
        Ok(answer.removed)
    }

    pub async fn list(&self) -> Result<Vec<String>> {
        let answer: Entries = self.host.call("host.cron.list", json!({})).await?;
        Ok(answer.entries)
    }
}

pub struct Quota<'a> {
    host: &'a Host<'a>,
}

impl<'a> Quota<'a> {
    pub(super) fn new(host: &'a Host<'a>) -> Self {
        Self { host }
    }

    pub async fn set(&self, user: &str, root: &str, blocks: Limits, inodes: Limits) -> Result<()> {
        self.host
            .call::<Value>(
                "host.quota.set",
                json!({ "user": user, "root": root, "blocks": blocks, "inodes": inodes }),
            )
            .await
            .map(drop)
    }

    pub async fn list(&self, root: &str) -> Result<QuotaReport> {
        self.host
            .call("host.quota.list", json!({ "root": root }))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::super::HostExecutor;
    use super::super::testing::{Recorder, origin};
    use super::*;

    #[tokio::test]
    async fn packages_and_quotas_send_the_arguments_of_the_catalogue_and_read_its_answers() {
        let usage = json!({ "used": 2100, "soft": 1000, "hard": 4000, "grace": 604800 });
        let recorder = Recorder::answering(vec![
            json!({ "changed": true, "changes": [{ "name": "hello", "from": null, "to": "2.10-3" }] }),
            json!({ "changed": false, "changes": [] }),
            json!({ "name": "hello", "version": null }),
            json!({ "refreshed": true }),
            json!({ "user": "hp_site", "filesystem": "/" }),
            json!({ "filesystem": "/", "quotas": [{ "user": "hp_site", "blocks": usage, "inodes": usage }] }),
        ]);
        let executor: &dyn HostExecutor = &recorder;
        let host = executor.on(origin());
        let pkg = host.pkg();
        let installed = pkg.install(&["hello"]).await.unwrap();
        assert!(installed.changed);
        assert_eq!(
            installed.changes,
            [PackageChange {
                name: "hello".to_owned(),
                from: None,
                to: Some("2.10-3".to_owned())
            }]
        );
        assert!(!pkg.remove(&["hello"]).await.unwrap().changed);
        assert_eq!(pkg.get("hello").await.unwrap(), None);
        pkg.refresh().await.unwrap();
        let quota = host.quota();
        let limits = Limits {
            soft: 1000,
            hard: 4000,
        };
        quota.set("hp_site", "www", limits, limits).await.unwrap();
        let report = quota.list("www").await.unwrap();
        assert_eq!(report.filesystem, "/");
        assert_eq!(report.quotas[0].blocks.grace, 604800);
        assert_eq!(
            recorder.calls(),
            [
                (
                    "host.pkg.install".to_owned(),
                    json!({ "packages": ["hello"] })
                ),
                (
                    "host.pkg.remove".to_owned(),
                    json!({ "packages": ["hello"] })
                ),
                ("host.pkg.get".to_owned(), json!({ "name": "hello" })),
                ("host.pkg.refresh".to_owned(), json!({})),
                (
                    "host.quota.set".to_owned(),
                    json!({ "user": "hp_site", "root": "www", "blocks": { "soft": 1000, "hard": 4000 }, "inodes": { "soft": 1000, "hard": 4000 } })
                ),
                ("host.quota.list".to_owned(), json!({ "root": "www" })),
            ]
        );
    }

    #[tokio::test]
    async fn start_and_stop_send_the_unit_and_status_reads_memory_and_start() {
        let recorder = Recorder::answering(vec![
            json!({ "name": "nginx", "start": true }),
            json!({ "name": "nginx", "stop": true }),
            json!({ "services": [
                { "name": "nginx", "state": "active", "installed": true, "memory": 7340032, "since": 1790000000 },
                { "name": "vsftpd", "state": "inactive", "installed": false, "memory": null, "since": null }
            ] }),
        ]);
        let executor: &dyn HostExecutor = &recorder;
        let host = executor.on(origin());
        let services = host.services();
        services.start("nginx", Some("nginx.test")).await.unwrap();
        services.stop("nginx").await.unwrap();
        assert_eq!(
            services.status().await.unwrap(),
            [
                ServiceStatus {
                    name: "nginx".to_owned(),
                    state: "active".to_owned(),
                    installed: true,
                    memory: Some(7_340_032),
                    since: Some(1_790_000_000),
                },
                ServiceStatus {
                    name: "vsftpd".to_owned(),
                    state: "inactive".to_owned(),
                    installed: false,
                    memory: None,
                    since: None,
                }
            ]
        );
        assert_eq!(
            recorder.calls(),
            [
                (
                    "host.service.start".to_owned(),
                    json!({ "name": "nginx", "validate": "nginx.test" })
                ),
                ("host.service.stop".to_owned(), json!({ "name": "nginx" })),
                ("host.service.status".to_owned(), json!({})),
            ]
        );
    }
}
