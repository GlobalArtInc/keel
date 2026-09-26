use std::fs;
use std::sync::Mutex;

use keel::prelude::*;
use keel::sdk::sea_orm::{ConnectionTrait, DbBackend, Statement};
use keel::types::{ErrorCode, TaskState};
use keel_hosting_web::api::{DnsProvider, Invoke, SiteZone};
use keel_system::tasks::api::Drain;
use keel_testkit::{Harness, Who, tokio};

use crate::support::{
    OLDER_PHP, PATIENCE, PHP, Site, create, data, eventually, panel, panel_with, rows,
    state_becomes,
};

const CREATE: &str = "hosting.site.create";
const USER_TOOLS: [&str; 5] = ["useradd ", "userdel ", "usermod ", "groupadd ", "groupdel "];

static ORDERS: Mutex<Vec<(String, String, u64)>> = Mutex::new(Vec::new());

static FAKE_DNS: ModuleDef = ModuleDef {
    id: "fake_dns",
    version: "0.0.1",
    sdk: VERSION,
    requires: &["hosting_site"],
    migrations: None,
    init: fake_dns,
};

struct FakeDns;

#[async_trait]
impl DnsProvider for FakeDns {
    async fn add_zone(&self, invoke: &Invoke<'_>, zone: &SiteZone) -> Result<()> {
        if zone.name.starts_with("refused.") {
            return Err(KeelError::validation("fake_dns.error.refused").field("domain"));
        }
        let seen = invoke("hosting.site.list", json!({ "search": zone.name })).await?;
        let total = seen["total"].as_u64().unwrap_or_default();
        ORDERS.lock().expect("the orders").push((
            zone.name.clone(),
            zone.account.to_string(),
            total,
        ));
        Ok(())
    }
}

fn fake_dns(r: &mut Registrar) -> Result<()> {
    r.provide::<dyn DnsProvider>(Arc::new(FakeDns));
    Ok(())
}

async fn user(harness: &Harness, who: Who, name: &str) -> String {
    let id = create(harness, who, name).await;
    state_becomes(harness, &id, "active").await;
    id
}

async fn site_of(harness: &Harness, who: Who, params: Value) -> String {
    let reply = harness.call(who, CREATE, params).await;
    data(&reply)["id"].as_str().expect("an id").to_owned()
}

async fn served(harness: &Harness, id: &str, state: &str) {
    let sql = format!("SELECT state FROM hosting_site WHERE id = '{id}'");

    eventually(&format!("the site {id} in state {state}"), async || {
        rows(harness, &sql).await == [state]
    })
    .await;
}

fn effects(site: &Site) -> Vec<String> {
    site.calls()
        .into_iter()
        .filter(|call| !USER_TOOLS.iter().any(|tool| call.starts_with(tool)))
        .collect()
}

fn read(path: &std::path::Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[tokio::test]
async fn a_site_renders_its_server_block_and_pool_after_commit_and_reloads() {
    let site = Site::new();
    let harness = panel(&site).await;
    let a1 = harness.fixtures().accounts.a1;
    let alice = user(&harness, Who::A1, "alice").await;

    let reply = harness
        .call(
            Who::A1,
            CREATE,
            json!({ "user": alice, "domain": "Example.COM", "aliases": ["WWW.example.com"], "php": PHP }),
        )
        .await;
    let created = data(&reply).clone();
    assert_eq!(created["domain"], json!("example.com"));
    assert_eq!(created["aliases"], json!(["www.example.com"]));
    assert_eq!(created["root"], json!("example.com"));
    assert_eq!(created["php"], json!(PHP));
    assert_eq!(created["state"], json!("pending"));
    assert_eq!(created["accountId"], json!(a1.to_string()));
    let id = created["id"].as_str().expect("an id").to_owned();

    served(&harness, &id, "active").await;
    let socket = format!("/run/php/keel-panel-{id}-{PHP}.sock");
    let vhost = read(&site.vhost("example.com"));
    assert!(
        vhost.contains("server_name example.com www.example.com;\n"),
        "{vhost}"
    );
    assert!(
        vhost.contains("disable_symlinks if_not_owner from=$document_root;\n"),
        "{vhost}"
    );
    assert!(
        vhost.contains(&format!(
            "root {}/example.com;\n",
            site.home("alice").display()
        )),
        "{vhost}"
    );
    assert!(
        vhost.contains(&format!("fastcgi_pass unix:{socket};\n")),
        "{vhost}"
    );
    assert!(!vhost.contains("return 403;"), "{vhost}");
    let pool = read(&site.pool("example.com"));
    assert!(pool.starts_with("[keel-panel-example.com]\n"), "{pool}");
    assert!(pool.contains("user = hp_alice\n"), "{pool}");
    assert!(pool.contains(&format!("listen = {socket}\n")), "{pool}");
    assert_eq!(
        effects(&site),
        [
            format!("php-fpm{PHP} -t"),
            format!("systemctl reload php{PHP}-fpm"),
            "nginx -t".to_owned(),
            "systemctl reload nginx".to_owned(),
        ]
    );

    let taken = harness
        .call(
            Who::A1,
            CREATE,
            json!({ "user": alice, "domain": "shop.example.com", "aliases": ["www.EXAMPLE.com"] }),
        )
        .await;
    let error = taken.error().expect("the alias is served already");
    assert_eq!(error.fields[0].path, "aliases", "{error:?}");
    assert_eq!(
        error.fields[0].key.as_deref(),
        Some("hosting.site.error.exists")
    );
    let foreign = harness
        .call(
            Who::A2,
            CREATE,
            json!({ "user": alice, "domain": "theirs.example.com" }),
        )
        .await;
    let error = foreign.error().expect("the user of another account");
    assert_eq!(error.fields[0].path, "user", "{error:?}");
    let theirs = harness.call(Who::A2, "hosting.site.list", json!({})).await;
    assert_eq!(data(&theirs)["total"], json!(0));
    harness.finish().await;
}

#[tokio::test]
async fn a_failing_nginx_check_leaves_the_previous_config_and_marks_the_site() {
    let site = Site::new();
    let harness = panel(&site).await;
    let alice = user(&harness, Who::A1, "alice").await;
    let id = site_of(
        &harness,
        Who::A1,
        json!({ "user": alice, "domain": "example.org" }),
    )
    .await;
    served(&harness, &id, "active").await;
    let before = read(&site.vhost("example.org"));
    assert!(before.contains("try_files $uri $uri/ =404;"), "{before}");
    assert!(!site.pool("example.org").exists());

    site.break_check("nginx", true);
    let edited = harness
        .call(
            Who::A1,
            "hosting.site.edit",
            json!({ "id": id, "aliases": ["www.example.org"], "root": "example.org/public" }),
        )
        .await;
    assert_eq!(data(&edited)["state"], json!("pending"));
    served(&harness, &id, "error").await;
    assert_eq!(read(&site.vhost("example.org")), before);
    let got = harness
        .call(Who::A1, "hosting.site.get", json!({ "id": id }))
        .await;
    let marked = data(&got)["error"]
        .as_str()
        .expect("the error of the check");
    assert!(
        marked.contains("[emerg] the configuration of the test is broken"),
        "{marked}"
    );
    assert_eq!(
        effects(&site),
        ["nginx -t", "systemctl reload nginx", "nginx -t"]
    );

    site.break_check("nginx", false);
    harness
        .call(
            Who::A1,
            "hosting.site.edit",
            json!({ "id": id, "aliases": ["www.example.org"], "root": "example.org/public" }),
        )
        .await;
    served(&harness, &id, "active").await;
    let after = read(&site.vhost("example.org"));
    assert!(
        after.contains("server_name example.org www.example.org;\n"),
        "{after}"
    );
    assert!(after.contains("/example.org/public;\n"), "{after}");
    let got = harness
        .call(Who::A1, "hosting.site.get", json!({ "id": id }))
        .await;
    assert_eq!(data(&got)["error"], Value::Null);
    harness.finish().await;
}

#[tokio::test]
async fn deleting_a_site_removes_its_files_and_reloads() {
    let site = Site::new();
    let harness = panel(&site).await;
    let alice = user(&harness, Who::A1, "alice").await;
    let id = site_of(
        &harness,
        Who::A1,
        json!({ "user": alice, "domain": "example.net", "php": PHP }),
    )
    .await;
    served(&harness, &id, "active").await;
    assert!(site.vhost("example.net").is_file());
    assert!(site.pool("example.net").is_file());

    let busy = harness
        .call(Who::A1, "hosting.user.delete", json!({ "ids": [alice] }))
        .await;
    let error = busy.error().expect("the user still serves a site");
    assert_eq!(
        error.fields[0].key.as_deref(),
        Some("hosting.site.error.user_busy"),
        "{error:?}"
    );
    let rendered = effects(&site).len();

    let deleted = harness
        .call(Who::A1, "hosting.site.delete", json!({ "ids": [id] }))
        .await;
    assert_eq!(data(&deleted)["deleted"], json!(1));
    eventually("the files of the site are removed", async || {
        !site.vhost("example.net").exists() && !site.pool("example.net").exists()
    })
    .await;
    eventually("php-fpm is reloaded without the pool", async || {
        effects(&site).len() == rendered + 4
    })
    .await;
    assert_eq!(
        effects(&site)[rendered..],
        [
            "nginx -t".to_owned(),
            "systemctl reload nginx".to_owned(),
            format!("php-fpm{PHP} -t"),
            format!("systemctl reload php{PHP}-fpm"),
        ]
    );
    assert!(
        rows(&harness, "SELECT id FROM hosting_site")
            .await
            .is_empty()
    );
    let removed = harness
        .call(Who::A1, "hosting.user.delete", json!({ "ids": [alice] }))
        .await;
    assert_eq!(data(&removed)["deleted"], json!(1));
    harness.finish().await;
}

#[tokio::test]
async fn suspending_the_account_refuses_its_sites_and_deleting_it_removes_them() {
    let site = Site::new();
    let harness = panel(&site).await;
    let a1 = harness.fixtures().accounts.a1;
    let alice = user(&harness, Who::A1, "alice").await;
    let carol = user(&harness, Who::A2, "carol").await;
    let mine = site_of(
        &harness,
        Who::A1,
        json!({ "user": alice, "domain": "a.example" }),
    )
    .await;
    let theirs = site_of(
        &harness,
        Who::A2,
        json!({ "user": carol, "domain": "c.example" }),
    )
    .await;
    served(&harness, &mine, "active").await;
    served(&harness, &theirs, "active").await;

    for (verb, refused) in [("suspend", true), ("resume", false)] {
        let reply = harness
            .call(
                Who::Reseller,
                &format!("keel.account.{verb}"),
                json!({ "id": a1 }),
            )
            .await;
        assert_eq!(data(&reply)["changed"], json!(true), "{verb}");
        eventually(&format!("the site after {verb}"), async || {
            fs::read_to_string(site.vhost("a.example"))
                .is_ok_and(|text| text.contains("return 403;") == refused)
        })
        .await;
        served(&harness, &mine, "active").await;
        assert!(!read(&site.vhost("c.example")).contains("return 403;"));
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
            .contains(&format!("removed 1 site(s) of account {a1}"))
    );
    eventually("the site of the deleted account is removed", async || {
        !site.vhost("a.example").exists()
    })
    .await;
    assert!(site.vhost("c.example").is_file());
    assert_eq!(
        rows(&harness, "SELECT domain FROM hosting_site").await,
        ["c.example"]
    );
    harness.finish().await;
}

#[tokio::test]
async fn the_sites_limit_is_enforced() {
    let site = Site::new();
    let harness = panel(&site).await;
    let a1 = harness.fixtures().accounts.a1;
    harness
        .db()
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO keel_limit (account_id, key, value) VALUES ($1, 'hosting.site.max', '1'::jsonb)",
            [a1.to_string().into()],
        ))
        .await
        .expect("the limit of one site");
    let alice = user(&harness, Who::A1, "alice").await;
    let carol = user(&harness, Who::A2, "carol").await;

    site_of(
        &harness,
        Who::A1,
        json!({ "user": alice, "domain": "one.example" }),
    )
    .await;
    let refused = harness
        .call(
            Who::A1,
            CREATE,
            json!({ "user": alice, "domain": "two.example" }),
        )
        .await;
    let error = refused.error().expect("the second site is over the limit");
    assert_eq!(error.code, ErrorCode::LimitExceeded);
    assert_eq!(error.params["limit"], json!("hosting.site.max"));
    assert_eq!(error.params["max"], json!(1));
    site_of(
        &harness,
        Who::A2,
        json!({ "user": carol, "domain": "two.example" }),
    )
    .await;
    assert_eq!(
        rows(&harness, "SELECT domain FROM hosting_site ORDER BY domain").await,
        ["one.example", "two.example"]
    );
    harness.finish().await;
}

#[tokio::test]
async fn a_name_that_is_not_a_host_name_never_reaches_a_template() {
    let site = Site::new();
    let harness = panel(&site).await;
    let alice = user(&harness, Who::A1, "alice").await;

    for (params, field) in [
        (
            json!({ "user": alice, "domain": "example.com;include /etc/passwd" }),
            "domain",
        ),
        (
            json!({ "user": alice, "domain": "example.com", "aliases": ["a b.example"] }),
            "aliases[0]",
        ),
        (
            json!({ "user": alice, "domain": "example.com", "root": "../../etc" }),
            "root",
        ),
        (
            json!({ "user": alice, "domain": "example.com", "php": "8.4;id" }),
            "php",
        ),
    ] {
        let refused = harness.call(Who::A1, CREATE, params).await;
        let error = refused.error().expect("the params are refused");
        assert_eq!(error.code, ErrorCode::Validation, "{error:?}");
        assert_eq!(error.fields[0].path, field, "{error:?}");
    }
    let without_dns = harness
        .call(
            Who::A1,
            CREATE,
            json!({ "user": alice, "domain": "example.com", "dnsZone": true }),
        )
        .await;
    let error = without_dns.error().expect("no DNS is provided");
    assert_eq!(error.fields[0].path, "dnsZone", "{error:?}");
    assert_eq!(
        error.fields[0].key.as_deref(),
        Some("hosting.site.error.dns_unavailable")
    );
    assert!(
        rows(&harness, "SELECT id FROM hosting_site")
            .await
            .is_empty()
    );
    assert!(effects(&site).is_empty(), "{:?}", effects(&site));
    assert_eq!(
        fs::read_dir(site.path("vhosts"))
            .expect("the vhosts root")
            .count(),
        0
    );
    harness.finish().await;
}

#[tokio::test]
async fn a_site_with_a_zone_asks_the_dns_provider_inside_its_call() {
    let site = Site::new();
    let harness = panel_with(&site, &[&FAKE_DNS]).await;
    let a1 = harness.fixtures().accounts.a1;
    let alice = user(&harness, Who::A1, "alice").await;

    let id = site_of(
        &harness,
        Who::A1,
        json!({ "user": alice, "domain": "zone.example", "dnsZone": true }),
    )
    .await;
    served(&harness, &id, "active").await;
    assert!(ORDERS.lock().expect("the orders").contains(&(
        "zone.example".to_owned(),
        a1.to_string(),
        1
    )));

    let refused = harness
        .call(
            Who::A1,
            CREATE,
            json!({ "user": alice, "domain": "refused.example", "dnsZone": true }),
        )
        .await;
    let error = refused.error().expect("the provider refuses the zone");
    assert_eq!(error.fields[0].path, "domain", "{error:?}");
    assert_eq!(
        error.fields[0].key.as_deref(),
        Some("fake_dns.error.refused")
    );
    assert_eq!(
        rows(&harness, "SELECT domain FROM hosting_site").await,
        ["zone.example"]
    );
    assert!(!site.vhost("refused.example").exists());
    harness.finish().await;
}

#[tokio::test]
async fn switching_php_off_removes_the_pool_after_the_server_block() {
    let site = Site::new();
    let harness = panel(&site).await;
    let alice = user(&harness, Who::A1, "alice").await;
    let id = site_of(
        &harness,
        Who::A1,
        json!({ "user": alice, "domain": "example.info", "php": PHP }),
    )
    .await;
    served(&harness, &id, "active").await;
    let rendered = effects(&site).len();

    let edited = harness
        .call(
            Who::A1,
            "hosting.site.edit",
            json!({ "id": id, "root": "example.info", "php": null }),
        )
        .await;
    assert_eq!(data(&edited)["php"], Value::Null);
    eventually("the pool is removed", async || {
        !site.pool("example.info").exists() && effects(&site).len() == rendered + 4
    })
    .await;
    served(&harness, &id, "active").await;
    let vhost = read(&site.vhost("example.info"));
    assert!(!vhost.contains("fastcgi_pass"), "{vhost}");
    assert_eq!(
        effects(&site)[rendered..],
        [
            "nginx -t".to_owned(),
            "systemctl reload nginx".to_owned(),
            format!("php-fpm{PHP} -t"),
            format!("systemctl reload php{PHP}-fpm"),
        ]
    );
    harness.finish().await;
}

#[tokio::test]
async fn a_failed_php_switch_leaves_no_pool_behind() {
    let site = Site::new();
    let harness = panel(&site).await;
    let alice = user(&harness, Who::A1, "alice").await;
    let domains = ["example.biz", "example.name"];
    let mut ids = Vec::new();
    for domain in domains {
        let id = site_of(
            &harness,
            Who::A1,
            json!({ "user": alice, "domain": domain, "php": OLDER_PHP }),
        )
        .await;
        served(&harness, &id, "active").await;
        assert!(site.pool_of(OLDER_PHP, domain).is_file());
        ids.push(id);
    }

    site.break_check("nginx", true);
    for (id, domain) in ids.iter().zip(domains) {
        harness
            .call(
                Who::A1,
                "hosting.site.edit",
                json!({ "id": id, "root": domain, "php": PHP }),
            )
            .await;
        served(&harness, id, "error").await;
        assert!(site.pool_of(OLDER_PHP, domain).is_file());
        assert!(!site.pool_of(PHP, domain).exists());
    }
    site.break_check("nginx", false);

    harness
        .call(
            Who::A1,
            "hosting.site.edit",
            json!({ "id": ids[0], "root": domains[0], "php": PHP }),
        )
        .await;
    served(&harness, &ids[0], "active").await;
    assert!(site.pool_of(PHP, domains[0]).is_file());
    assert!(!site.pool_of(OLDER_PHP, domains[0]).exists());
    let vhost = read(&site.vhost(domains[0]));
    assert!(
        vhost.contains(&format!("keel-panel-{}-{PHP}.sock", ids[0])),
        "{vhost}"
    );
    let rendered = effects(&site).len();

    let deleted = harness
        .call(Who::A1, "hosting.site.delete", json!({ "ids": ids }))
        .await;
    assert_eq!(data(&deleted)["deleted"], json!(2));
    eventually("both versions of php-fpm are reloaded", async || {
        effects(&site).len() == rendered + 6
    })
    .await;
    assert_eq!(
        effects(&site)[rendered..],
        [
            "nginx -t".to_owned(),
            "systemctl reload nginx".to_owned(),
            format!("php-fpm{OLDER_PHP} -t"),
            format!("systemctl reload php{OLDER_PHP}-fpm"),
            format!("php-fpm{PHP} -t"),
            format!("systemctl reload php{PHP}-fpm"),
        ]
    );
    for domain in domains {
        assert!(!site.vhost(domain).exists());
        for version in [PHP, OLDER_PHP] {
            assert!(
                !site.pool_of(version, domain).exists(),
                "{version} {domain}"
            );
        }
    }
    harness.finish().await;
}
