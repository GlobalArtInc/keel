use std::path::Path;
use std::time::Duration;

use keel_sdk::DashboardWidget;
use keel_sdk::prelude::*;
use keel_sdk::system::Extensions;
use keel_sdk::types::{ErrorCode, WidgetKind};
use keel_system::cron::ERROR_RUNNING;
use keel_system::cron::api::{CronJob, Outcome, Scheduler};
use keel_system::notify::POLL_JOB;
use keel_system::notify::api::ProblemSource;
use keel_system::plugins::{
    DIRECTORY, DISABLE, ENABLE, ERROR_STOPPED, ERROR_UNKNOWN, FILE, GET, LIST, LOGIN_PREFIX,
    MODULE, PROBLEM_REFUSED,
};
use keel_testkit::{Harness, Reply, Who, tokio};
use tempfile::TempDir;

const PATIENCE: Duration = Duration::from_secs(20);
const ID: &str = "acme";

fn manifest() -> Value {
    json!({
        "protocol": 1,
        "id": ID,
        "version": "1.2.0",
        "requires": { "keel": ">=0.4", "modules": { "notify": ">=0.4" } },
        "run": { "mode": "daemon", "cmd": ["./bin/acme", "--serve"], "restart": "always" },
        "level": "user",
        "storage": "kv",
        "rights": { "observe": ["keel.account.*"], "hook": [], "patch": [] },
        "functions": [
            {
                "name": "acme.sync.list",
                "kind": "list",
                "access": { "level": "user" },
                "key": "id",
                "params": { "$ref": "keel://ListQuery" },
                "ui": {
                    "title": "acme.sync.list.title",
                    "columns": [{ "key": "id", "type": "text" }]
                }
            },
            {
                "name": "acme.renew.run",
                "kind": "action",
                "access": { "level": "admin" }
            }
        ],
        "settings": [
            { "key": "acme.email", "schema": { "type": "string" }, "default": "ops@example.org", "level": "admin" }
        ],
        "cron": [{ "id": "acme.renew", "schedule": "0 3 * * *", "func": "acme.renew.run" }],
        "menu": [{ "path": "system/acme", "func": "acme.sync.list", "order": 90, "icon": "refresh-cw" }],
        "problems": [{ "source": "acme.queue", "func": "acme.sync.list" }],
        "widgets": [{ "kind": "list", "func": "acme.sync.list", "limit": 5 }],
        "i18n": { "en": "i18n/en.json", "ru": "i18n/ru.json" }
    })
}

fn install(dir: &Path, manifest: &Value) {
    let home = dir.join(DIRECTORY).join(ID);
    std::fs::create_dir_all(home.join("i18n")).expect("the plugin directory is created");
    std::fs::create_dir_all(home.join("bin")).expect("the bin directory is created");
    std::fs::write(home.join("bin/acme"), "#!/bin/sh\n").expect("the program is written");
    std::fs::write(home.join(FILE), manifest.to_string()).expect("the manifest is written");
    let en = json!({ "acme": {
        "sync": { "column": { "id": "Id" }, "list": { "title": "Synchronised zones" } },
        "renew": { "run": { "title": "Renew" } }
    } });
    let ru = json!({ "acme": {
        "sync": { "column": { "id": "Ид" }, "list": { "title": "Синхронизированные зоны" } },
        "renew": { "run": { "title": "Продлить" } }
    } });
    std::fs::write(home.join("i18n/en.json"), en.to_string()).expect("en is written");
    std::fs::write(home.join("i18n/ru.json"), ru.to_string()).expect("ru is written");
}

async fn boot(dir: &Path) -> Harness {
    Harness::new()
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .module(&MODULE)
        .data_dir(dir)
        .boot()
        .await
}

async fn restart(harness: Harness, dir: &Path) -> Harness {
    let database = harness.stop().await;
    Harness::new()
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .module(&MODULE)
        .data_dir(dir)
        .database(database)
        .boot()
        .await
}

async fn enabled(dir: &Path) -> Harness {
    let harness = boot(dir).await;
    let reply = harness.call(Who::Admin, ENABLE, json!({ "id": ID })).await;
    assert_eq!(data(&reply)["changed"], json!(true));
    assert_eq!(data(&reply)["restart"], json!(true));
    restart(harness, dir).await
}

fn data(reply: &Reply) -> &Value {
    reply
        .data()
        .unwrap_or_else(|| panic!("a result was expected, got {:?}", reply.error()))
}

fn field_key(reply: &Reply) -> Option<&str> {
    reply
        .error()
        .and_then(|error| error.fields.iter().find(|field| field.path == "id"))
        .and_then(|field| field.key.as_deref())
}

async fn row(harness: &Harness) -> Value {
    let listed = harness.call(Who::Admin, LIST, json!({})).await;
    data(&listed)["data"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["id"] == json!(ID)))
        .cloned()
        .unwrap_or_else(|| panic!("no row for {ID} in {listed:?}"))
}

async fn run(harness: &Harness, job: &str) {
    let scheduler = harness
        .registry()
        .services()
        .get::<Scheduler>()
        .expect("the cron module provides the scheduler");
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while !scheduler.is_leader() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the scheduler did not lead"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let started = loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{job} did not start in time"
        );
        match scheduler.run_now(job).await {
            Ok(started) => break started,
            Err(error) if error.key() == ERROR_RUNNING => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) => panic!("{job} did not start: {error:?}"),
        }
    };
    let outcome = tokio::time::timeout(PATIENCE, scheduler.wait(started.run))
        .await
        .expect("the run ends in time")
        .expect("the outcome is read");
    assert_eq!(outcome, Outcome::Ok, "{job}");
}

async fn open_problems(harness: &Harness) -> Vec<String> {
    harness
        .database()
        .rows("SELECT key || ' ' || subject || ' ' || severity FROM notify_problem WHERE cleared_at IS NULL ORDER BY key, subject")
        .await
}

fn refusal(harness: &Harness) -> Option<String> {
    harness
        .registry()
        .services()
        .get::<Extensions>()
        .and_then(|extensions| {
            extensions
                .all()
                .iter()
                .find(|extension| extension.id == ID)
                .cloned()
        })
        .and_then(|extension| extension.refused)
}

#[tokio::test]
async fn a_manifest_registers_its_functions_menu_settings_and_i18n() {
    let dir = TempDir::new().expect("a data dir");
    install(dir.path(), &manifest());
    let harness = boot(dir.path()).await;
    assert!(harness.registry().function("acme.sync.list").is_none());
    let before = row(&harness).await;
    assert_eq!(before["enabled"], json!(false));
    assert_eq!(before["status"], json!("unloaded"));
    assert_eq!(before["version"], json!("1.2.0"));
    harness.finish().await;

    let harness = enabled(dir.path()).await;
    assert_eq!(refusal(&harness), None);
    let registry = harness.registry();
    let list = registry
        .function("acme.sync.list")
        .expect("the plugin's list is registered");
    let descriptor = list.descriptor();
    assert_eq!(descriptor.module, ID);
    assert_eq!(descriptor.kind, Kind::List);
    assert_eq!(descriptor.mode, Mode::Read);
    assert_eq!(descriptor.access.level, Level::User);
    assert_eq!(
        descriptor.ui.as_ref().map(|ui| ui.title.as_str()),
        Some("acme.sync.list.title")
    );
    let run = registry
        .function("acme.renew.run")
        .expect("the plugin's action is registered");
    assert_eq!(run.descriptor().mode, Mode::Write);
    assert_eq!(run.descriptor().access.level, Level::Admin);
    let menu: Vec<(&str, &str)> = registry
        .menu()
        .iter()
        .filter(|node| node.func().starts_with("acme."))
        .map(|node| (node.path(), node.func()))
        .collect();
    assert_eq!(menu, [("system/acme", "acme.sync.list")]);
    let contributions = registry.contributions();
    let settings: Vec<(&str, Level)> = contributions
        .of::<SettingDef>()
        .filter(|def| def.key().starts_with("acme."))
        .map(|def| (def.key(), def.level_value()))
        .collect();
    assert_eq!(settings, [("acme.email", Level::Admin)]);
    let jobs: Vec<(&str, &str)> = contributions
        .of::<CronJob>()
        .filter(|job| job.module() == ID)
        .map(|job| (job.id(), job.schedule()))
        .collect();
    assert_eq!(jobs, [("acme.renew", "0 3 * * *")]);
    assert!(
        contributions
            .of::<ProblemSource>()
            .any(|source| source.id() == "acme.queue")
    );
    let widgets: Vec<&DashboardWidget> = contributions
        .of::<DashboardWidget>()
        .filter(|widget| widget.module() == ID)
        .collect();
    assert_eq!(widgets.len(), 1);
    assert_eq!(widgets[0].kind(), WidgetKind::List);
    assert_eq!(widgets[0].limit_value(), Some(5));
    for (lang, title) in [
        ("en", "Synchronised zones"),
        ("ru", "Синхронизированные зоны"),
    ] {
        let messages = registry.i18n(lang).expect("the language is served");
        assert_eq!(
            messages["acme"]["sync"]["list"]["title"],
            json!(title),
            "{lang}"
        );
    }
    let answered = harness
        .call(
            Who::A1,
            "acme.sync.list",
            json!({ "page": 1, "pageSize": 25 }),
        )
        .await;
    let error = answered.error().expect("the plugin does not answer");
    assert_eq!(error.code, ErrorCode::Unavailable);
    assert_eq!(error.key, ERROR_STOPPED);
    let refused = harness.call(Who::A1, "acme.renew.run", json!({})).await;
    assert_eq!(
        refused.error().map(|error| error.code),
        Some(ErrorCode::Forbidden)
    );
    let after = row(&harness).await;
    assert_eq!(after["enabled"], json!(true));
    assert_eq!(after["status"], json!("loaded"));
    let principal = harness
        .database()
        .rows(&format!(
            "SELECT level || ' ' || account_id || ' ' || role FROM keel_principal WHERE login = '{LOGIN_PREFIX}{ID}'"
        ))
        .await;
    assert_eq!(principal, [format!("user {} plugin", Id::ROOT_ACCOUNT)]);
    let got = harness.call(Who::Admin, GET, json!({ "id": ID })).await;
    assert_eq!(data(&got)["mode"], json!("daemon"));
    assert_eq!(data(&got)["storage"], json!(true));
    assert_eq!(
        data(&got)["functions"],
        json!(["acme.sync.list", "acme.renew.run"])
    );
    assert_eq!(data(&got)["observe"], json!(["keel.account.*"]));
    harness.finish().await;
}

#[tokio::test]
async fn a_hook_on_auth_is_rejected_at_start_and_the_kernel_starts_without_the_plugin() {
    let dir = TempDir::new().expect("a data dir");
    let mut hooked = manifest();
    hooked["rights"]["hook"] = json!(["auth.session.list"]);
    install(dir.path(), &hooked);
    let harness = enabled(dir.path()).await;
    let reason = refusal(&harness).expect("the plugin is refused");
    assert!(reason.contains("auth.session.list"), "{reason}");
    assert!(harness.registry().function("acme.sync.list").is_none());
    assert!(
        harness
            .registry()
            .menu()
            .iter()
            .all(|node| node.path() != "system/acme")
    );
    let row = row(&harness).await;
    assert_eq!(row["enabled"], json!(true));
    assert_eq!(row["status"], json!("refused"));
    assert!(
        row["reason"]
            .as_str()
            .is_some_and(|text| text.contains("auth.session.list")),
        "{row}"
    );
    run(&harness, POLL_JOB).await;
    assert_eq!(
        open_problems(&harness).await,
        [format!("{PROBLEM_REFUSED} {ID} warning")]
    );
    harness.finish().await;
}

#[tokio::test]
async fn a_run_command_that_is_no_file_of_the_plugin_directory_is_refused_at_start() {
    for program in ["python3", "bin", "bin/missing"] {
        let dir = TempDir::new().expect("a data dir");
        let mut bare = manifest();
        bare["run"]["cmd"] = json!([program, "main.py"]);
        install(dir.path(), &bare);
        let harness = enabled(dir.path()).await;
        let reason = refusal(&harness).expect("the plugin is refused");
        assert!(
            reason.contains(&format!(
                "the run command {program} is not a file inside the plugin directory"
            )),
            "{reason}"
        );
        assert!(harness.registry().function("acme.sync.list").is_none());
        assert_eq!(row(&harness).await["status"], json!("refused"));
        run(&harness, POLL_JOB).await;
        assert_eq!(
            open_problems(&harness).await,
            [format!("{PROBLEM_REFUSED} {ID} warning")]
        );
        harness.finish().await;
    }
}

#[tokio::test]
async fn a_plugin_requiring_a_missing_module_is_refused_with_a_problem() {
    let dir = TempDir::new().expect("a data dir");
    let mut needy = manifest();
    needy["requires"]["modules"] = json!({ "dns": ">=1" });
    install(dir.path(), &needy);
    let harness = enabled(dir.path()).await;
    let reason = refusal(&harness).expect("the plugin is refused");
    assert!(
        reason.contains("it requires the module dns, which is not loaded"),
        "{reason}"
    );
    assert!(harness.registry().function("acme.sync.list").is_none());
    run(&harness, POLL_JOB).await;
    assert_eq!(
        open_problems(&harness).await,
        [format!("{PROBLEM_REFUSED} {ID} warning")]
    );
    let problem = harness
        .database()
        .rows(&format!(
            "SELECT params_json->>'reason' FROM notify_problem WHERE key = '{PROBLEM_REFUSED}'"
        ))
        .await;
    assert_eq!(problem, [reason]);
    harness.finish().await;
}

#[tokio::test]
async fn a_plugin_whose_registration_fails_the_freeze_is_refused_and_the_kernel_starts() {
    let dir = TempDir::new().expect("a data dir");
    let mut homeless = manifest();
    homeless["menu"][0]["path"] = json!("nowhere/acme");
    install(dir.path(), &homeless);
    let harness = enabled(dir.path()).await;
    let reason = refusal(&harness).expect("the plugin is refused");
    assert!(reason.contains("nowhere"), "{reason}");
    assert!(harness.registry().function("acme.sync.list").is_none());
    assert!(harness.registry().function(LIST).is_some());
    assert_eq!(row(&harness).await["status"], json!("refused"));
    harness.finish().await;
}

#[tokio::test]
async fn a_hook_above_the_plugins_level_and_an_unknown_hook_target_are_refused() {
    let dir = TempDir::new().expect("a data dir");
    let mut reaching = manifest();
    reaching["rights"]["hook"] = json!(["plugin.enable", "nobody.thing.delete"]);
    install(dir.path(), &reaching);
    let harness = enabled(dir.path()).await;
    let reason = refusal(&harness).expect("the plugin is refused");
    assert!(
        reason.contains(
            "it may not hook plugin.enable: the function asks for admin, above the plugin's user"
        ),
        "{reason}"
    );
    assert!(
        reason.contains("it may not hook nobody.thing.delete: no loaded module registers it"),
        "{reason}"
    );
    harness.finish().await;
}

#[tokio::test]
async fn a_new_level_in_the_manifest_waits_until_an_admin_enables_the_plugin_again() {
    let dir = TempDir::new().expect("a data dir");
    install(dir.path(), &manifest());
    let harness = enabled(dir.path()).await;
    assert_eq!(refusal(&harness), None);
    let mut raised = manifest();
    raised["level"] = json!("admin");
    install(dir.path(), &raised);
    let harness = restart(harness, dir.path()).await;
    let reason = refusal(&harness).expect("the raised level is not granted by itself");
    assert!(
        reason.contains("it asks for the level admin, and it was enabled at user"),
        "{reason}"
    );
    let again = harness.call(Who::Admin, ENABLE, json!({ "id": ID })).await;
    assert_eq!(data(&again)["changed"], json!(true));
    let harness = restart(harness, dir.path()).await;
    assert_eq!(refusal(&harness), None);
    let principal = harness
        .database()
        .rows(&format!(
            "SELECT level FROM keel_principal WHERE login = '{LOGIN_PREFIX}{ID}'"
        ))
        .await;
    assert_eq!(principal, ["admin"]);
    let unchanged = harness.call(Who::Admin, ENABLE, json!({ "id": ID })).await;
    assert_eq!(data(&unchanged)["changed"], json!(false));
    harness.finish().await;
}

#[tokio::test]
async fn disabling_a_plugin_unloads_it_at_the_next_start_and_keeps_its_principal() {
    let dir = TempDir::new().expect("a data dir");
    install(dir.path(), &manifest());
    let harness = enabled(dir.path()).await;
    assert!(harness.registry().function("acme.sync.list").is_some());
    let disabled = harness.call(Who::Admin, DISABLE, json!({ "id": ID })).await;
    assert_eq!(data(&disabled)["changed"], json!(true));
    let again = harness.call(Who::Admin, DISABLE, json!({ "id": ID })).await;
    assert_eq!(data(&again)["changed"], json!(false));
    let harness = restart(harness, dir.path()).await;
    assert!(harness.registry().function("acme.sync.list").is_none());
    assert_eq!(row(&harness).await["status"], json!("unloaded"));
    let principals = harness
        .database()
        .rows(&format!(
            "SELECT login FROM keel_principal WHERE login = '{LOGIN_PREFIX}{ID}'"
        ))
        .await;
    assert_eq!(principals.len(), 1);
    harness.finish().await;
}

#[tokio::test]
async fn the_plugin_functions_are_for_an_admin_and_refuse_unknown_or_malformed_ids() {
    let dir = TempDir::new().expect("a data dir");
    install(dir.path(), &manifest());
    let harness = boot(dir.path()).await;
    let forbidden = harness
        .call(Who::Reseller, ENABLE, json!({ "id": ID }))
        .await;
    assert_eq!(
        forbidden.error().map(|error| error.code),
        Some(ErrorCode::Forbidden)
    );
    let missing = harness
        .call(Who::Admin, ENABLE, json!({ "id": "absent" }))
        .await;
    assert_eq!(field_key(&missing), Some(ERROR_UNKNOWN));
    let unknown = harness
        .call(Who::Admin, DISABLE, json!({ "id": "absent" }))
        .await;
    assert_eq!(field_key(&unknown), Some(ERROR_UNKNOWN));
    for id in ["../acme", "Acme", ""] {
        let escaped = harness.call(Who::Admin, ENABLE, json!({ "id": id })).await;
        assert_eq!(
            escaped.error().map(|error| error.code),
            Some(ErrorCode::Validation),
            "{id:?}"
        );
    }
    harness.finish().await;
}
