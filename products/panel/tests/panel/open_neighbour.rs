use keel::prelude::*;
use keel::types::{ErrorCode, PeerScope, Scope};
use keel_dns_peer::{HANDOVER_TTL, UNPAIRED_OPEN, UNREACHABLE, ZONE_LIST};
use keel_system::audit::STATUS_COMMITTED;
use keel_system::auth::{KEY_HANDOVER, LOGIN_KEY_PREFIX, SETTING_SHELL_URL};
use keel_testkit::{Answer, Harness, Reply, TestHttp, Who, tokio};

use crate::bridge::with_dns;
use crate::dns_peer::{
    PAIR_CODE, PEER_CREATE, customer_login, dns_side, one, origin, panel_side, user, zone_owner,
};
use crate::support::{Site, data, eventually, rows};

const OPEN_ZONE: &str = "hosting.site.open_zone";
const TOKEN_CREATE: &str = "auth.token.create";
const READ_ONLY: &str = "keel.error.token_read_only";
const SCOPE: &str = "keel.error.token_scope";
const SITE_MASK: &str = "hosting.site.*";
const SITE_CREATE: &str = "hosting.site.create";
const PUBLIC: &str = "https://dns.example:8443";
const ZONE: &str = "zone.example";
const GRANTED: [&str; 3] = ["dns.zone.*", "keel.principal.upsert", KEY_HANDOVER];

struct Neighbours {
    panel: Harness,
    dns: Harness,
    dns_http: TestHttp,
    _sites: [Site; 2],
}

impl Neighbours {
    async fn paired(scopes: &[&str]) -> Self {
        let panel_site = Site::new();
        let dns_site = Site::new();
        let panel = panel_side(&panel_site).boot().await;
        let dns = dns_side(&dns_site).boot().await;
        let dns_http = dns.http().await;
        let issued = dns
            .call(
                Who::Admin,
                PAIR_CODE,
                json!({ "scopes": scopes, "level": "reseller", "onBehalf": true }),
            )
            .await;
        let code = data(&issued)["code"].as_str().expect("a code").to_owned();
        let created = panel
            .call(
                Who::Admin,
                PEER_CREATE,
                json!({ "url": dns_http.url(""), "code": code }),
            )
            .await;
        data(&created);
        Self {
            panel,
            dns,
            dns_http,
            _sites: [panel_site, dns_site],
        }
    }

    async fn published(scopes: &[&str]) -> Self {
        let neighbours = Self::paired(scopes).await;
        let set = neighbours
            .dns
            .call(
                Who::Admin,
                "settings.set",
                json!({ "key": SETTING_SHELL_URL, "value": format!("{PUBLIC}/") }),
            )
            .await;
        data(&set);
        neighbours
    }

    async fn finish(self) {
        self.dns_http.stop().await.expect("the server stops");
        self.panel.finish().await;
        self.dns.finish().await;
    }
}

async fn site(harness: &Harness, who: Who, user: &str, domain: &str, zone: bool) -> String {
    let created = harness
        .call(
            who,
            SITE_CREATE,
            json!({ "user": user, "domain": domain, "dnsZone": zone }),
        )
        .await;
    data(&created)["id"].as_str().expect("an id").to_owned()
}

async fn open(harness: &Harness, who: Who, site: &str) -> Reply {
    harness.call(who, OPEN_ZONE, json!({ "id": site })).await
}

fn opened(reply: &Reply) -> String {
    data(reply)["url"].as_str().expect("a url").to_owned()
}

fn key_of(url: &str, base: &str) -> String {
    let key = url
        .strip_prefix(&format!("{base}/login?key="))
        .unwrap_or_else(|| panic!("{url} is not a login link under {base}"));
    assert!(key.starts_with(LOGIN_KEY_PREFIX), "{url}");
    key.to_owned()
}

async fn redeem(http: &TestHttp, key: &str) -> Answer {
    http.request(
        "POST",
        "/api/v1/call/auth.key.redeem",
        &[("content-type", "application/json")],
        Some(&json!({ "key": key }).to_string()),
    )
    .await
}

async fn token(harness: &Harness, name: &str, scope: Value) -> String {
    let issued = harness
        .call(
            Who::A1,
            TOKEN_CREATE,
            json!({ "name": name, "scope": scope }),
        )
        .await;
    data(&issued)["token"]
        .as_str()
        .expect("the plaintext")
        .to_owned()
}

async fn open_by_token(http: &TestHttp, token: &str, site: &str) -> Answer {
    http.request(
        "POST",
        &format!("/api/v1/call/{OPEN_ZONE}"),
        &[
            ("authorization", &format!("Bearer {token}")),
            ("content-type", "application/json"),
        ],
        Some(&json!({ "id": site }).to_string()),
    )
    .await
}

fn refused(reply: &Reply, status: u16, code: ErrorCode, key: &str) {
    assert_eq!(reply.status(), status, "{reply:?}");
    let error = reply.error().expect("a refusal");
    assert_eq!((error.code, error.key.as_str()), (code, key), "{reply:?}");
}

#[tokio::test]
async fn the_neighbour_opens_by_a_one_time_key_with_the_peer_as_actor_in_the_audit() {
    let neighbours = Neighbours::published(&GRANTED).await;
    let (panel, dns) = (&neighbours.panel, &neighbours.dns);
    let alice = user(panel, Who::A1, "alice").await;
    let a1 = panel.fixtures().accounts.a1.to_string();
    let id = site(panel, Who::A1, &alice, ZONE, true).await;
    let owner = format!("{} {a1}", origin(panel));
    eventually("the zone on the neighbour", async || {
        zone_owner(dns, ZONE).await == [owner.clone()]
    })
    .await;

    let url = opened(&open(panel, Who::A1, &id).await);
    let key = key_of(&url, PUBLIC);
    let login = customer_login(panel, &a1);
    let customer = one(
        dns,
        &format!(
            "SELECT id FROM keel_principal WHERE login = '{login}' AND origin = '{}' AND external_id = '{a1}'",
            origin(panel)
        ),
    )
    .await;
    let link = one(dns, "SELECT id FROM peer_product").await;
    let peer = one(dns, "SELECT principal_id FROM peer_product").await;
    let landing = format!("{ZONE_LIST}?search={ZONE}");
    assert_eq!(
        rows(
            dns,
            "SELECT principal_id || ' ' || issued_by || ' ' || redirect || ' ' || (used_at IS NULL)::text || ' ' || extract(epoch FROM expires_at - created_at)::int FROM auth_login_key"
        )
        .await,
        [format!("{customer} {peer} {landing} true {HANDOVER_TTL}")]
    );

    let redeemed = redeem(&neighbours.dns_http, &key).await;
    assert_eq!(redeemed.status, 200, "{}", redeemed.body);
    let answer = &redeemed.json()["data"];
    assert_eq!(answer["redirect"], json!(landing));
    assert_eq!(answer["issuedBy"], json!(peer));
    assert_eq!(answer["principal"]["login"], json!(login));
    assert_eq!(answer["principal"]["level"], json!("user"));
    assert_eq!(answer["principal"]["actor"], Value::Null);
    let cookie = redeemed
        .session_cookie()
        .expect("the session of the customer");
    let zones = neighbours
        .dns_http
        .request(
            "POST",
            "/api/v1/call/dns.zone.list",
            &[("cookie", &cookie), ("content-type", "application/json")],
            Some(&json!({ "search": ZONE }).to_string()),
        )
        .await;
    assert_eq!(zones.status, 200, "{}", zones.body);
    assert_eq!(zones.json()["data"]["data"][0]["name"], json!(ZONE));
    assert_eq!(zones.json()["meta"]["principal"]["login"], json!(login));
    let again = redeem(&neighbours.dns_http, &key).await;
    assert_eq!(again.status, 401, "{}", again.body);

    let audited = format!(
        "{KEY_HANDOVER} {login} peer-{} peer:{link} {STATUS_COMMITTED}",
        link.to_ascii_lowercase()
    );
    eventually("the audit of the key on the neighbour", async || {
        rows(
            dns,
            &format!(
                "SELECT function || ' ' || principal_login || ' ' || coalesce(actor_login, '-') || ' ' || via || ' ' || status FROM audit_call WHERE function = '{KEY_HANDOVER}'"
            ),
        )
        .await
            == [audited.clone()]
    })
    .await;
    let trail = rows(dns, "SELECT params_redacted::text || coalesce(after::text, '') FROM audit_call WHERE function IN ('auth.key.handover', 'auth.key.redeem')").await;
    assert!(!trail.is_empty());
    assert!(trail.iter().all(|row| !row.contains(&key)), "{trail:?}");

    let bob = user(panel, Who::A2, "bob").await;
    let other = site(panel, Who::A2, &bob, "other.example", false).await;
    refused(
        &open(panel, Who::A1, &other).await,
        404,
        ErrorCode::NotFound,
        "keel.error.not_found",
    );
    assert_eq!(
        rows(dns, "SELECT count(*)::text FROM auth_login_key").await,
        ["1"]
    );
    neighbours.finish().await;
}

fn forbidden(answer: &Answer, key: &str) {
    assert_eq!(answer.status, 403, "{}", answer.body);
    let error = &answer.json()["error"];
    assert_eq!(error["code"], json!("forbidden"), "{}", answer.body);
    assert_eq!(error["key"], json!(key), "{}", answer.body);
    assert_eq!(
        error["params"]["function"],
        json!(OPEN_ZONE),
        "{}",
        answer.body
    );
}

#[tokio::test]
async fn a_token_of_the_customer_short_of_full_scope_opens_nothing_on_the_neighbour() {
    let neighbours = Neighbours::published(&GRANTED).await;
    let (panel, dns) = (&neighbours.panel, &neighbours.dns);
    let alice = user(panel, Who::A1, "alice").await;
    let id = site(panel, Who::A1, &alice, ZONE, false).await;
    let panel_http = panel.http().await;
    let synced = format!(
        "SELECT count(*)::text FROM keel_principal WHERE origin = '{}'",
        origin(panel)
    );
    let untouched = async || {
        assert_eq!(
            rows(dns, "SELECT count(*)::text FROM auth_login_key").await,
            ["0"]
        );
        assert_eq!(rows(dns, &synced).await, ["0"]);
    };

    let reader = token(panel, "reader", json!("read_only")).await;
    forbidden(&open_by_token(&panel_http, &reader, &id).await, READ_ONLY);
    untouched().await;

    let deployer = token(panel, "deployer", json!({ "functions": [SITE_MASK] })).await;
    forbidden(&open_by_token(&panel_http, &deployer, &id).await, SCOPE);
    untouched().await;

    let mut linked = panel.principal(Who::A1).clone();
    linked.scope = Scope::Peer(PeerScope {
        functions: vec![SITE_MASK.to_owned()],
        level: Level::User,
        on_behalf: true,
    });
    let through_a_link = panel
        .call_as_principal(&linked, OPEN_ZONE, json!({ "id": id }))
        .await;
    refused(&through_a_link, 403, ErrorCode::Forbidden, SCOPE);
    assert_eq!(
        through_a_link.error().expect("a refusal").params["function"],
        json!(OPEN_ZONE)
    );
    untouched().await;

    let writer = token(panel, "writer", json!("full")).await;
    let opened = open_by_token(&panel_http, &writer, &id).await;
    assert_eq!(opened.status, 200, "{}", opened.body);
    let url = opened.json()["data"]["url"]
        .as_str()
        .expect("a url")
        .to_owned();
    key_of(&url, PUBLIC);
    assert_eq!(
        rows(dns, "SELECT count(*)::text FROM auth_login_key").await,
        ["1"]
    );
    assert_eq!(rows(dns, &synced).await, ["1"]);
    panel_http.stop().await.expect("the server stops");
    neighbours.finish().await;
}

#[tokio::test]
async fn a_neighbour_without_a_public_address_opens_under_the_address_of_the_link() {
    let neighbours = Neighbours::paired(&GRANTED).await;
    let panel = &neighbours.panel;
    let alice = user(panel, Who::A1, "alice").await;
    let id = site(panel, Who::A1, &alice, "plain.example", false).await;
    let base = one(panel, "SELECT url FROM peer_product").await;

    let url = opened(&open(panel, Who::A1, &id).await);
    let key = key_of(&url, &base);
    let redeemed = redeem(&neighbours.dns_http, &key).await;
    assert_eq!(redeemed.status, 200, "{}", redeemed.body);
    assert_eq!(
        redeemed.json()["data"]["redirect"],
        json!(format!("{ZONE_LIST}?search=plain.example"))
    );
    neighbours.finish().await;
}

#[tokio::test]
async fn a_neighbour_that_refuses_the_key_or_cannot_be_reached_answers_peer() {
    let neighbours = Neighbours::published(&["dns.zone.*", "keel.principal.upsert"]).await;
    let panel = &neighbours.panel;
    let alice = user(panel, Who::A1, "alice").await;
    let id = site(panel, Who::A1, &alice, ZONE, false).await;

    let masked = open(panel, Who::A1, &id).await;
    refused(&masked, 502, ErrorCode::Peer, "keel.error.token_scope");
    assert_eq!(
        masked.error().expect("a refusal").params["function"],
        json!(KEY_HANDOVER)
    );
    assert!(
        rows(&neighbours.dns, "SELECT key_hash FROM auth_login_key")
            .await
            .is_empty()
    );

    let url = one(panel, "SELECT url FROM peer_product").await;
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
    let unreachable = open(&panel, Who::A1, &id).await;
    refused(&unreachable, 502, ErrorCode::Peer, UNREACHABLE);
    assert_eq!(
        unreachable.error().expect("a refusal").params["url"],
        json!(url)
    );
    panel.finish().await;
    dns.finish().await;
}

#[tokio::test]
async fn without_a_neighbour_a_zone_does_not_open() {
    let site_dir = Site::new();
    let panel = panel_side(&site_dir).boot().await;
    let alice = user(&panel, Who::A1, "alice").await;
    let id = site(&panel, Who::A1, &alice, ZONE, false).await;

    let unpaired = open(&panel, Who::A1, &id).await;
    refused(&unpaired, 503, ErrorCode::Unavailable, UNPAIRED_OPEN);
    assert_eq!(
        unpaired.error().expect("a refusal").params["zone"],
        json!(ZONE)
    );
    refused(
        &open(&panel, Who::A2, &id).await,
        404,
        ErrorCode::NotFound,
        "keel.error.not_found",
    );
    panel.finish().await;
}

#[tokio::test]
async fn with_dns_on_this_installation_a_zone_opens_on_its_own_zone_list() {
    let site_dir = Site::new();
    let harness = with_dns(&site_dir)
        .module(&keel_system::auth::MODULE)
        .boot()
        .await;
    assert!(harness.modules().contains(&"dns_local".to_owned()));
    let alice = user(&harness, Who::A1, "alice").await;
    let id = site(&harness, Who::A1, &alice, ZONE, true).await;

    let url = opened(&open(&harness, Who::A1, &id).await);
    assert_eq!(url, format!("{ZONE_LIST}?search={ZONE}"));
    let mut reader = harness.principal(Who::A1).clone();
    reader.scope = Scope::ReadOnly;
    let read_only = harness
        .call_as_principal(&reader, OPEN_ZONE, json!({ "id": id }))
        .await;
    refused(&read_only, 403, ErrorCode::Forbidden, READ_ONLY);
    assert_eq!(
        read_only.error().expect("a refusal").params["function"],
        json!(OPEN_ZONE)
    );
    reader.scope = Scope::Functions(vec![SITE_MASK.to_owned()]);
    let masked = harness
        .call_as_principal(&reader, OPEN_ZONE, json!({ "id": id }))
        .await;
    refused(&masked, 403, ErrorCode::Forbidden, SCOPE);
    assert_eq!(
        masked.error().expect("a refusal").params["function"],
        json!(OPEN_ZONE)
    );
    assert!(
        rows(&harness, "SELECT key_hash FROM auth_login_key")
            .await
            .is_empty()
    );
    harness.finish().await;
}
