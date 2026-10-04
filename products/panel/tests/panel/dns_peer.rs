use keel::prelude::*;
use keel::sdk::sea_orm::ConnectionTrait;
use keel::sdk::system::{Caller, CallerOptions};
use keel::types::ErrorCode;
use keel_dns_peer::{
    CREATE as ZONE_TASK, EXISTS as ZONE_EXISTS, FAILED, ORIGIN_NAME, UNPAIRED, UNREACHABLE,
};
use keel_system::audit::STATUS_COMMITTED;
use keel_system::notify::POLL_JOB;
use keel_testkit::{Builder, Harness, Reply, TestHttp, Who, tokio};

use crate::bridge::with_dns;
use crate::support::{Site, composed, create, data, eventually, rows, run_job, state_becomes};

const SITE_CREATE: &str = "hosting.site.create";
const SITE_DELETE: &str = "hosting.site.delete";
const ZONE_CREATE: &str = "dns.zone.create";
const PAIR_CODE: &str = "peer.pair_code";
const PEER_CREATE: &str = "peer.create";
const REFUSED_ZONE: &str = "refused.example";
const PROBE_REFUSED: &str = "probe.error.refused";

static PROBE: ModuleDef = ModuleDef {
    id: "probe",
    version: "0.0.1",
    sdk: VERSION,
    requires: &["dns"],
    migrations: None,
    init: probe,
};

fn probe(r: &mut Registrar) -> Result<()> {
    r.within(ZONE_CREATE, |call: Call<Value>| async move {
        if call.params()["name"] == json!(REFUSED_ZONE) {
            return Err(KeelError::conflict(PROBE_REFUSED).param("name", REFUSED_ZONE));
        }
        Ok(())
    });
    Ok(())
}

fn peered(builder: Builder) -> Builder {
    builder
        .module(&keel_system::auth::MODULE)
        .module(&keel_system::audit::MODULE)
        .module(&keel_system::webhooks::MODULE)
        .module(&keel_system::peers::MODULE)
}

fn panel_side(site: &Site) -> Builder {
    peered(
        composed(site)
            .product(&keel_dns::MANIFEST)
            .enabled(["panel"]),
    )
}

fn dns_side(site: &Site) -> Builder {
    peered(with_dns(site).enabled(["dns"]))
}

struct Neighbours {
    panel: Harness,
    dns: Harness,
    dns_http: TestHttp,
    _sites: [Site; 2],
}

impl Neighbours {
    async fn paired() -> Self {
        let panel_site = Site::new();
        let dns_site = Site::new();
        let panel = panel_side(&panel_site).boot().await;
        let dns = dns_side(&dns_site).boot().await;
        Self::pair(panel, dns, [panel_site, dns_site]).await
    }

    async fn with_probe() -> Self {
        let panel_site = Site::new();
        let dns_site = Site::new();
        let panel = panel_side(&panel_site).boot().await;
        let dns = dns_side(&dns_site).module(&PROBE).boot().await;
        Self::pair(panel, dns, [panel_site, dns_site]).await
    }

    async fn pair(panel: Harness, dns: Harness, sites: [Site; 2]) -> Self {
        let dns_http = dns.http().await;
        let modules = panel.modules();
        assert!(modules.contains(&"dns_peer".to_owned()), "{modules:?}");
        assert!(!modules.contains(&"dns".to_owned()), "{modules:?}");
        assert!(dns.modules().contains(&"dns".to_owned()));
        pair(&panel, &dns, &dns_http).await;
        Self {
            panel,
            dns,
            dns_http,
            _sites: sites,
        }
    }

    async fn finish(self) {
        self.dns_http.stop().await.expect("the server stops");
        self.panel.finish().await;
        self.dns.finish().await;
    }
}

async fn pair(panel: &Harness, other: &Harness, http: &TestHttp) {
    let issued = other
        .call(
            Who::Admin,
            PAIR_CODE,
            json!({
                "scopes": ["dns.zone.*", "keel.principal.upsert"],
                "level": "admin",
                "onBehalf": true,
            }),
        )
        .await;
    let code = data(&issued)["code"].as_str().expect("a code").to_owned();
    let created = panel
        .call(
            Who::Admin,
            PEER_CREATE,
            json!({ "url": http.url(""), "code": code }),
        )
        .await;
    data(&created);
}

async fn user(harness: &Harness, who: Who, name: &str) -> String {
    let id = create(harness, who, name).await;
    state_becomes(harness, &id, "active").await;
    id
}

async fn one(harness: &Harness, sql: &str) -> String {
    let found = rows(harness, sql).await;
    assert_eq!(found.len(), 1, "{sql}: {found:?}");
    found[0].clone()
}

async fn site(harness: &Harness, user: &str, domain: &str) -> Reply {
    harness
        .call(
            Who::A1,
            SITE_CREATE,
            json!({ "user": user, "domain": domain, "dnsZone": true }),
        )
        .await
}

fn origin(panel: &Harness) -> String {
    format!("{ORIGIN_NAME}:{}", panel.instance())
}

fn customer_login(panel: &Harness, account: &str) -> String {
    format!(
        "{ORIGIN_NAME}-{}-{}",
        panel.instance().to_string().to_ascii_lowercase(),
        account.to_ascii_lowercase()
    )
}

fn translated(harness: &Harness, lang: &str, key: &str) -> bool {
    let mut node = harness.registry().i18n(lang).cloned().map(Value::Object);
    for segment in key.split('.') {
        node = node.and_then(|value| value.get(segment).cloned());
    }
    node.is_some_and(|value| value.is_string())
}

async fn synced(dns: &Harness, panel: &Harness) -> Vec<String> {
    rows(
        dns,
        &format!(
            "SELECT login FROM keel_principal WHERE origin = '{}'",
            origin(panel)
        ),
    )
    .await
}

async fn zone_owner(dns: &Harness, name: &str) -> Vec<String> {
    rows(
        dns,
        &format!(
            "SELECT coalesce(a.origin, '-') || ' ' || coalesce(a.external_id, '-') FROM dns_zone z JOIN keel_account a ON a.id = z.account_id WHERE z.name = '{name}'"
        ),
    )
    .await
}

async fn tasks(panel: &Harness) -> Vec<String> {
    rows(
        panel,
        &format!("SELECT state FROM task_job WHERE function = '{ZONE_TASK}' ORDER BY created_at"),
    )
    .await
}

#[tokio::test]
async fn a_site_on_one_installation_creates_its_zone_on_the_other_on_behalf_of_the_customer() {
    let neighbours = Neighbours::paired().await;
    let (panel, dns) = (&neighbours.panel, &neighbours.dns);
    let alice = user(panel, Who::A1, "alice").await;
    let a1 = panel.fixtures().accounts.a1.to_string();
    let customer = format!("{} {a1}", origin(panel));

    let created = site(panel, &alice, "Zone.Example").await;
    let id = data(&created)["id"].as_str().expect("an id").to_owned();
    assert_eq!(data(&created)["zoneError"], Value::Null);
    eventually("the zone on the neighbour", async || {
        zone_owner(dns, "zone.example").await == [customer.clone()]
    })
    .await;
    let login = customer_login(panel, &a1);
    assert_eq!(
        rows(
            dns,
            &format!(
                "SELECT login || ' ' || level FROM keel_principal WHERE origin = '{}' AND external_id = '{a1}'",
                origin(panel)
            )
        )
        .await,
        [format!("{login} user")]
    );
    let link = one(dns, "SELECT id FROM peer_product").await;
    let audited = format!(
        "{login} peer-{} peer:{link} {STATUS_COMMITTED}",
        link.to_ascii_lowercase()
    );
    eventually("the audit of the create on the neighbour", async || {
        rows(
            dns,
            "SELECT principal_login || ' ' || coalesce(actor_login, '-') || ' ' || via || ' ' || status FROM audit_call WHERE function = 'dns.zone.create'",
        )
        .await
            == [audited.clone()]
    })
    .await;
    eventually("the task of the zone", async || {
        tasks(panel).await == ["done"]
    })
    .await;
    assert_eq!(
        rows(
            panel,
            &format!("SELECT coalesce(zone_error, '-') FROM hosting_site WHERE id = '{id}'")
        )
        .await,
        ["-"]
    );
    eventually("the zone published on the neighbour", async || {
        rows(
            dns,
            "SELECT state FROM dns_zone WHERE name = 'zone.example'",
        )
        .await
            == ["active"]
    })
    .await;

    let token = panel
        .call(
            Who::A1,
            "auth.token.create",
            json!({ "name": "sites", "scope": { "functions": [SITE_CREATE] } }),
        )
        .await;
    let token = data(&token)["token"].as_str().expect("a token").to_owned();
    let panel_http = panel.http().await;
    let bearer = format!("Bearer {token}");
    let scoped = panel_http
        .request(
            "POST",
            &format!("/api/v1/call/{SITE_CREATE}"),
            &[
                ("authorization", bearer.as_str()),
                ("content-type", "application/json"),
            ],
            Some(
                &json!({ "user": alice, "domain": "scoped.example", "dnsZone": true }).to_string(),
            ),
        )
        .await;
    assert_eq!(scoped.status, 200, "{}", scoped.body);
    eventually(
        "the zone of a site created by a narrowed token",
        async || zone_owner(dns, "scoped.example").await == [customer.clone()],
    )
    .await;
    eventually("both tasks of the zones", async || {
        tasks(panel).await == ["done", "done"]
    })
    .await;
    assert_eq!(
        rows(
            dns,
            &format!(
                "SELECT count(*)::text FROM keel_principal WHERE origin = '{}'",
                origin(panel)
            )
        )
        .await,
        ["1"]
    );

    let plain = panel
        .call(
            Who::A1,
            SITE_CREATE,
            json!({ "user": alice, "domain": "plain.example" }),
        )
        .await;
    data(&plain);
    assert!(zone_owner(dns, "plain.example").await.is_empty());
    assert_eq!(tasks(panel).await, ["done", "done"]);
    panel_http.stop().await.expect("the server stops");
    neighbours.finish().await;
}

#[tokio::test]
async fn a_dns_error_reaches_the_hosting_form_as_a_field_key() {
    let neighbours = Neighbours::paired().await;
    let (panel, dns) = (&neighbours.panel, &neighbours.dns);
    let alice = user(panel, Who::A1, "alice").await;
    let theirs = dns
        .call(Who::A2, ZONE_CREATE, json!({ "name": "taken.example" }))
        .await;
    data(&theirs);

    let refused = site(panel, &alice, "Taken.Example").await;
    assert_eq!(refused.status(), 502, "{refused:?}");
    let error = refused.error().expect("the neighbour holds the name");
    assert_eq!(error.code, ErrorCode::Peer, "{error:?}");
    assert_eq!(error.fields.len(), 1, "{error:?}");
    assert_eq!(error.fields[0].path, "domain");
    assert_eq!(error.fields[0].key.as_deref(), Some(ZONE_EXISTS));
    assert_eq!(
        error.fields[0].code.as_deref(),
        Some("dns.zone.error.exists")
    );
    assert_eq!(error.fields[0].params["name"], json!("taken.example"));
    for lang in ["en", "ru"] {
        assert!(translated(panel, lang, ZONE_EXISTS), "{lang}");
        assert!(!translated(panel, lang, "dns.zone.error.exists"), "{lang}");
        assert!(translated(dns, lang, "dns.zone.error.exists"), "{lang}");
    }
    assert!(rows(panel, "SELECT id FROM hosting_site").await.is_empty());
    assert!(tasks(panel).await.is_empty());
    assert_eq!(
        rows(dns, "SELECT account_id FROM dns_zone").await,
        [dns.fixtures().accounts.a2.to_string()]
    );
    neighbours.finish().await;
}

#[tokio::test]
async fn an_unreachable_neighbour_answers_peer() {
    let neighbours = Neighbours::paired().await;
    let alice = user(&neighbours.panel, Who::A1, "alice").await;
    let url = one(&neighbours.panel, "SELECT url FROM peer_product").await;
    let Neighbours {
        panel,
        dns,
        dns_http,
        _sites,
    } = neighbours;
    dns_http
        .stop()
        .await
        .expect("the neighbour stops answering");

    let refused = site(&panel, &alice, "zone.example").await;
    assert_eq!(refused.status(), 502, "{refused:?}");
    let error = refused.error().expect("the neighbour is out of reach");
    assert_eq!(error.code, ErrorCode::Peer, "{error:?}");
    assert_eq!(error.key, UNREACHABLE);
    assert_eq!(error.params["url"], json!(url));
    assert!(error.params["reason"].as_str().is_some(), "{error:?}");
    assert!(rows(&panel, "SELECT id FROM hosting_site").await.is_empty());
    assert!(tasks(&panel).await.is_empty());

    let foreign = panel
        .call(
            Who::A2,
            SITE_CREATE,
            json!({ "user": alice, "domain": "foreign.example", "dnsZone": true }),
        )
        .await;
    let error = foreign
        .error()
        .expect("the hosting user lies in another account");
    assert_eq!(error.code, ErrorCode::Validation, "{error:?}");
    assert_eq!(error.fields[0].path, "user");
    assert_eq!(
        error.fields[0].key.as_deref(),
        Some("hosting.site.error.user")
    );

    let plain = panel
        .call(
            Who::A1,
            SITE_CREATE,
            json!({ "user": alice, "domain": "zone.example" }),
        )
        .await;
    data(&plain);
    panel.finish().await;
    dns.finish().await;
}

#[tokio::test]
async fn a_refusal_the_site_form_is_certain_to_give_comes_before_the_neighbour() {
    let neighbours = Neighbours::paired().await;
    let alice = user(&neighbours.panel, Who::A1, "alice").await;
    let bob = user(&neighbours.panel, Who::A1, "bob").await;
    let held = neighbours
        .panel
        .call(
            Who::A1,
            SITE_CREATE,
            json!({ "user": alice, "domain": "held.example", "aliases": ["www.held.example"] }),
        )
        .await;
    data(&held);
    neighbours
        .panel
        .db()
        .execute_unprepared(&format!(
            "UPDATE hosting_user SET home = NULL WHERE id = '{bob}'"
        ))
        .await
        .expect("bob is not ready yet");

    let refusals = async |panel: &Harness| {
        let pending = panel
            .call(
                Who::A1,
                SITE_CREATE,
                json!({ "user": bob, "domain": "zone.example", "dnsZone": true }),
            )
            .await;
        let taken = site(panel, &alice, "Held.Example").await;
        let alias = panel
            .call(
                Who::A1,
                SITE_CREATE,
                json!({
                    "user": alice,
                    "domain": "other.example",
                    "aliases": ["WWW.held.example"],
                    "dnsZone": true,
                }),
            )
            .await;
        [pending, taken, alias]
            .iter()
            .map(|reply| {
                let error = reply.error().expect("the site form refuses");
                let field = error.fields.first().expect("a field error");
                format!(
                    "{} {:?} {} {}",
                    reply.status(),
                    error.code,
                    field.path,
                    field.key.as_deref().unwrap_or("-")
                )
            })
            .collect::<Vec<_>>()
    };
    let expected = [
        "422 Validation user hosting.site.error.user_pending",
        "422 Validation domain hosting.site.error.exists",
        "422 Validation aliases hosting.site.error.exists",
    ];
    assert_eq!(refusals(&neighbours.panel).await, expected);
    assert!(synced(&neighbours.dns, &neighbours.panel).await.is_empty());

    let Neighbours {
        panel,
        dns,
        dns_http,
        _sites,
    } = neighbours;
    dns_http
        .stop()
        .await
        .expect("the neighbour stops answering");
    assert_eq!(refusals(&panel).await, expected);
    assert_eq!(
        rows(&panel, "SELECT domain FROM hosting_site").await,
        ["held.example"]
    );
    assert!(tasks(&panel).await.is_empty());
    panel.finish().await;
    dns.finish().await;
}

#[tokio::test]
async fn a_silent_neighbour_answers_peer_before_the_hook_times_out() {
    let neighbours = Neighbours::paired().await;
    let alice = user(&neighbours.panel, Who::A1, "alice").await;
    let silent = std::net::TcpListener::bind("127.0.0.1:0").expect("a port that never answers");
    let url = format!("http://{}", silent.local_addr().expect("its address"));
    neighbours
        .panel
        .db()
        .execute_unprepared(&format!("UPDATE peer_product SET url = '{url}'"))
        .await
        .expect("the link points at the silent port");

    let started = std::time::Instant::now();
    let refused = site(&neighbours.panel, &alice, "zone.example").await;
    let error = refused.error().expect("the neighbour never answers");
    assert_eq!(error.code, ErrorCode::Peer, "{error:?}");
    assert_eq!(error.key, UNREACHABLE);
    assert_eq!(error.params["url"], json!(url));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert!(
        rows(&neighbours.panel, "SELECT id FROM hosting_site")
            .await
            .is_empty()
    );
    drop(silent);
    neighbours.finish().await;
}

#[tokio::test]
async fn the_task_of_a_site_gone_before_it_runs_creates_no_zone() {
    let site = Site::new();
    let panel = panel_side(&site).boot().await;
    let gone = Id::generate();
    let caller = panel
        .registry()
        .services()
        .get::<dyn Caller>()
        .expect("the kernel calls in process");
    let answer = caller
        .call(
            panel.principal(Who::A1).clone(),
            ZONE_TASK,
            json!({ "site": gone }),
            CallerOptions::default(),
        )
        .await
        .expect("the task ends quietly");
    assert_eq!(answer, json!({ "site": gone, "zone": null }));
    let entered = panel
        .call(Who::A1, ZONE_TASK, json!({ "site": gone }))
        .await;
    assert_eq!(
        entered.error().map(|error| error.key.as_str()),
        Some("keel.error.internal_only"),
        "{entered:?}"
    );
    panel.finish().await;
}

#[tokio::test]
async fn a_zone_refused_after_commit_marks_the_site_row_and_raises_a_problem() {
    let neighbours = Neighbours::with_probe().await;
    let (panel, dns) = (&neighbours.panel, &neighbours.dns);
    let alice = user(panel, Who::A1, "alice").await;
    let a1 = panel.fixtures().accounts.a1.to_string();

    let created = site(panel, &alice, REFUSED_ZONE).await;
    let id = data(&created)["id"].as_str().expect("an id").to_owned();
    eventually("the failed task of the zone", async || {
        tasks(panel).await == ["failed"]
    })
    .await;
    assert_eq!(
        rows(
            panel,
            &format!("SELECT error_json->>'code' || ' ' || (error_json->>'key') FROM task_job WHERE function = '{ZONE_TASK}'")
        )
        .await,
        [format!("peer {PROBE_REFUSED}")]
    );
    let marked = format!(
        "SELECT coalesce(zone_error, '-') || ' ' || state FROM hosting_site WHERE id = '{id}'"
    );
    eventually("the site served with the error of its zone", async || {
        rows(panel, &marked).await == [format!("{PROBE_REFUSED} active")]
    })
    .await;
    let listed = panel.call(Who::A1, "hosting.site.list", json!({})).await;
    assert_eq!(data(&listed)["data"][0]["zoneError"], json!(PROBE_REFUSED));
    assert!(zone_owner(dns, REFUSED_ZONE).await.is_empty());

    let open = "SELECT key || ' ' || subject || ' ' || coalesce(account_id, '-') || ' ' || severity || ' ' || (params_json->>'zone') || ' ' || (params_json->>'error') FROM notify_problem WHERE cleared_at IS NULL AND key LIKE 'dns_peer.%'";
    run_job(panel, POLL_JOB).await;
    assert_eq!(
        rows(panel, open).await,
        [format!(
            "{FAILED} {id} {a1} critical {REFUSED_ZONE} {PROBE_REFUSED}"
        )]
    );

    let deleted = panel
        .call(Who::A1, SITE_DELETE, json!({ "ids": [id] }))
        .await;
    assert_eq!(data(&deleted)["deleted"], json!(1));
    run_job(panel, POLL_JOB).await;
    assert!(rows(panel, open).await.is_empty());
    neighbours.finish().await;
}

#[tokio::test]
async fn a_neighbour_without_dns_leaves_the_zone_unpaired() {
    let panel_site = Site::new();
    let panel = panel_side(&panel_site).boot().await;
    let other = peered(
        Harness::new()
            .module(&keel_system::cron::MODULE)
            .module(&keel_system::notify::MODULE),
    )
    .boot()
    .await;
    let other_http = other.http().await;
    pair(&panel, &other, &other_http).await;
    let alice = user(&panel, Who::A1, "alice").await;

    let refused = site(&panel, &alice, "zone.example").await;
    let error = refused.error().expect("the neighbour offers no dns");
    assert_eq!(error.code, ErrorCode::Unavailable, "{error:?}");
    assert_eq!(error.key, UNPAIRED);
    assert_eq!(error.params["zone"], json!("zone.example"));
    assert!(rows(&panel, "SELECT id FROM hosting_site").await.is_empty());
    assert!(
        rows(
            &other,
            "SELECT login FROM keel_principal WHERE origin IS NOT NULL"
        )
        .await
        .is_empty()
    );
    other_http.stop().await.expect("the server stops");
    panel.finish().await;
    other.finish().await;
}
