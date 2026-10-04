use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use keel_sdk::prelude::*;
use keel_sdk::sea_orm::ConnectionTrait;
use keel_sdk::serde_json;
use keel_sdk::system::{AuditEvent, AuditOutcome, AuditSink};
use keel_sdk::types::Via;
use keel_system::auth::{KEY_HANDOVER, KEY_HANDOVER_REFUSED, LOGIN_KEY_PREFIX};
use keel_system::peers::{ERROR_CUSTOMER, ERROR_HEADER, ERROR_ORIGIN, LOGIN_PREFIX};
use keel_testkit::{Answer, Harness, TestHttp, Who, tokio};

use crate::peers::{
    Linked, Two, call_with, composed, data, installation, link_principal, mint, outbound, refused,
};

const ON_BEHALF_OF: &str = "x-keel-on-behalf-of";
const REFUSED: &str = "keel.error.on_behalf";
const OTHER_INSTANCE: &str = "01J9C00000000000000000000C";
const REDIRECT: &str = "/f/keel.account.list";

#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<AuditEvent>>,
}

impl AuditSink for Recorder {
    fn record(&self, event: AuditEvent) {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
    }
}

impl Recorder {
    fn of_request(&self, request_id: &str) -> Vec<AuditEvent> {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|event| event.request_id == request_id)
            .cloned()
            .collect()
    }
}

static RECORDER: LazyLock<Arc<Recorder>> = LazyLock::new(|| Arc::new(Recorder::default()));

static AUDITED: ModuleDef = ModuleDef {
    id: "onbehalfprobe",
    version: "0.1.0",
    sdk: VERSION,
    requires: &[],
    migrations: None,
    init: audited_init,
};

fn audited_init(r: &mut Registrar) -> Result<()> {
    r.provide::<dyn AuditSink>(RECORDER.clone());
    Ok(())
}

struct Linking {
    two: Two,
    linked: Linked,
    token: String,
    origin: String,
}

impl Linking {
    async fn new(two: Two, grants: Value) -> Self {
        let linked = two.pair(grants, json!({})).await;
        let token = outbound(&two.a, &linked.a_link).await;
        let origin = format!("bill:{}", two.a.instance());
        Self {
            two,
            linked,
            token,
            origin,
        }
    }

    async fn start(grants: Value) -> Self {
        Self::new(Two::start().await, grants).await
    }

    fn b(&self) -> &Harness {
        &self.two.b
    }

    fn http(&self) -> &TestHttp {
        &self.two.b_http
    }

    fn peer_login(&self) -> String {
        format!("{LOGIN_PREFIX}{}", self.linked.b_link.to_lowercase())
    }

    async fn as_customer(&self, external_id: &str, func: &str, params: Value) -> Answer {
        self.with_origin(&self.origin, external_id, func, params)
            .await
    }

    async fn with_origin(
        &self,
        origin: &str,
        external_id: &str,
        func: &str,
        params: Value,
    ) -> Answer {
        let header = format!("origin={origin}; external_id={external_id}");
        on_behalf(self.http(), &self.token, &[header.as_str()], func, params).await
    }

    async fn finish(self) {
        self.two.finish().await;
    }
}

async fn on_behalf(
    http: &TestHttp,
    token: &str,
    headers: &[&str],
    func: &str,
    params: Value,
) -> Answer {
    let bearer = format!("Bearer {token}");
    let mut sent = vec![
        ("authorization", bearer.as_str()),
        ("content-type", "application/json"),
    ];
    sent.extend(headers.iter().map(|header| (ON_BEHALF_OF, *header)));
    http.request(
        "POST",
        &format!("/api/v1/call/{func}"),
        &sent,
        Some(&params.to_string()),
    )
    .await
}

async fn issue_as_peer(linking: &Linking, target: Value) -> Answer {
    let mut params = json!({ "ttl": 60, "redirect": REDIRECT });
    if let (Some(params), Some(target)) = (params.as_object_mut(), target.as_object()) {
        params.extend(target.clone());
    }
    call_with(linking.http(), &linking.token, "auth.key.issue", params).await
}

async fn issue_for(linking: &Linking, customer: &str, target: Value) -> Answer {
    let mut params = json!({ "ttl": 60, "redirect": REDIRECT });
    if let (Some(params), Some(target)) = (params.as_object_mut(), target.as_object()) {
        params.extend(target.clone());
    }
    linking
        .as_customer(customer, "auth.key.issue", params)
        .await
}

async fn execute(harness: &Harness, sql: &str) {
    harness
        .db()
        .execute_unprepared(sql)
        .await
        .unwrap_or_else(|error| panic!("{sql}: {error}"));
}

async fn sync_principal(harness: &Harness, who: Who, origin: Option<&str>, external_id: &str) {
    let id = harness.principal(who).id;
    let origin = origin.map_or("NULL".to_owned(), |origin| format!("'{origin}'"));
    execute(
        harness,
        &format!("UPDATE keel_principal SET origin = {origin}, external_id = '{external_id}' WHERE id = '{id}'"),
    )
    .await;
}

async fn sync_account(harness: &Harness, who: Who, origin: &str, external_id: &str) {
    let account = harness.principal(who).account_id;
    execute(
        harness,
        &format!("UPDATE keel_account SET origin = '{origin}', external_id = '{external_id}' WHERE id = '{account}'"),
    )
    .await;
}

async fn sync(harness: &Harness, who: Who, origin: &str, external_id: &str) {
    sync_principal(harness, who, Some(origin), external_id).await;
    sync_account(harness, who, origin, &format!("acc-{external_id}")).await;
}

fn field_refused(answer: &Answer, path: &str, key: &str) {
    refused(answer, 422, "keel.error.validation");
    let field = &answer.json()["error"]["fields"][0];
    assert_eq!(field["path"], json!(path), "{}", answer.body);
    assert_eq!(field["key"], json!(key), "{}", answer.body);
}

fn customer_refused(answer: &Answer, origin: &str, external_id: &str) {
    refused(answer, 404, ERROR_CUSTOMER);
    let params = &answer.json()["error"]["params"];
    assert_eq!(params["origin"], json!(origin), "{}", answer.body);
    assert_eq!(params["externalId"], json!(external_id), "{}", answer.body);
}

#[tokio::test]
async fn an_on_behalf_call_runs_as_the_customer_with_the_peer_as_actor() {
    let two = Two::of(
        installation().await,
        composed(Harness::new().module(&AUDITED)).boot().await,
    )
    .await;
    let linking = Linking::new(
        two,
        json!({ "scopes": ["keel.account.*", "keel.principal.*"], "level": "admin", "onBehalf": true }),
    )
    .await;
    let b = linking.b();
    let a1 = b.principal(Who::A1).clone();
    let a2 = b.principal(Who::A2).clone();
    sync(b, Who::A1, &linking.origin, "client-42").await;
    let peer = link_principal(b, &linking.linked.b_link).await;
    let link: Id = linking.linked.b_link.parse().unwrap();

    let direct = call_with(
        linking.http(),
        &linking.token,
        "keel.account.list",
        json!({}),
    )
    .await;
    assert_eq!(direct.status, 200, "{}", direct.body);
    assert!(
        direct.json()["data"]["total"].as_u64().unwrap() >= 5,
        "{}",
        direct.body
    );
    assert_eq!(
        direct.json()["meta"]["principal"]["login"],
        json!(linking.peer_login())
    );

    let listed = linking
        .as_customer("client-42", "keel.account.list", json!({}))
        .await;
    assert_eq!(listed.status, 200, "{}", listed.body);
    let body = listed.json();
    assert_eq!(body["data"]["total"], json!(1), "{}", listed.body);
    assert_eq!(body["data"]["data"][0]["id"], json!(a1.account_id));
    assert_eq!(
        body["meta"]["principal"],
        json!({
            "id": a1.id, "login": "a1", "level": "user", "account": a1.account_id,
            "actor": { "id": peer, "login": linking.peer_login(), "level": "admin", "via": "peer" }
        })
    );

    let above = linking
        .as_customer(
            "client-42",
            "keel.account.create",
            json!({ "kind": "user" }),
        )
        .await;
    refused(&above, 403, "keel.error.level");
    let other_account = linking
        .as_customer(
            "client-42",
            "keel.account.get",
            json!({ "id": a2.account_id }),
        )
        .await;
    refused(&other_account, 404, "keel.error.not_found");
    let other_principal = linking
        .as_customer(
            "client-42",
            "keel.principal.update",
            json!({ "id": a2.id, "lang": "ru" }),
        )
        .await;
    refused(&other_principal, 404, "keel.error.not_found");

    let updated = linking
        .as_customer(
            "client-42",
            "keel.principal.update",
            json!({ "id": a1.id, "lang": "ru" }),
        )
        .await;
    assert_eq!(updated.status, 200, "{}", updated.body);
    assert_eq!(
        b.database()
            .rows(&format!(
                "SELECT lang FROM keel_principal WHERE id = '{}'",
                a1.id
            ))
            .await,
        ["ru"]
    );
    let request_id = updated
        .header("x-keel-request-id")
        .expect("the request id")
        .to_owned();
    let audited = RECORDER.of_request(&request_id);
    assert_eq!(audited.len(), 1, "{audited:?}");
    let event = &audited[0];
    assert_eq!(event.function, "keel.principal.update");
    assert_eq!(event.outcome, AuditOutcome::Committed);
    assert_eq!(event.principal.id, a1.id);
    assert_eq!(event.principal.level, Level::User);
    assert_eq!(event.principal.account_id, a1.account_id);
    assert_eq!(event.principal.via, Via::Peer(link));
    let actor = event.principal.actor.as_deref().expect("the peer acts");
    assert_eq!(actor.id.to_string(), peer);
    assert_eq!(actor.login, linking.peer_login());
    assert_eq!(actor.via, Via::Peer(link));
    let refused_request = other_principal
        .header("x-keel-request-id")
        .expect("the request id")
        .to_owned();
    let failed = RECORDER.of_request(&refused_request);
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(failed[0].principal.id, a1.id);
    assert!(!failed[0].outcome.is_committed());

    let denied = b
        .call(
            Who::Admin,
            "auth.grant.set",
            json!({ "principal": a1.id, "key": "keel.account.read", "granted": false }),
        )
        .await;
    assert_eq!(denied.status(), 200, "{denied:?}");
    let withdrawn = linking
        .as_customer("client-42", "keel.account.list", json!({}))
        .await;
    refused(&withdrawn, 403, "keel.error.permission");
    let still = call_with(
        linking.http(),
        &linking.token,
        "keel.account.list",
        json!({}),
    )
    .await;
    assert_eq!(still.status, 200, "{}", still.body);

    linking.finish().await;
}

#[tokio::test]
async fn a_principal_created_by_hand_is_out_of_reach() {
    let linking = Linking::start(json!({
        "scopes": ["keel.account.*", "auth.key.issue"], "level": "admin", "onBehalf": true
    }))
    .await;
    let b = linking.b();
    let origin = linking.origin.clone();
    let other = format!("bill:{OTHER_INSTANCE}");
    let id_of = |who: Who| json!({ "principal": b.principal(who).id });

    sync_principal(b, Who::A2, None, "client-7").await;
    let by_hand = linking
        .as_customer("client-7", "keel.account.list", json!({}))
        .await;
    customer_refused(&by_hand, &origin, "client-7");
    let key_by_hand = issue_as_peer(&linking, id_of(Who::A2)).await;
    refused(&key_by_hand, 404, "keel.error.not_found");

    let created = b
        .call(
            Who::Admin,
            "keel.principal.create",
            json!({ "login": "byhand", "level": "user", "account": b.principal(Who::A1).account_id }),
        )
        .await;
    let created_id = data(&created)["id"].as_str().unwrap().to_owned();
    for external_id in ["byhand", created_id.as_str()] {
        let fresh = linking
            .as_customer(external_id, "keel.account.list", json!({}))
            .await;
        customer_refused(&fresh, &origin, external_id);
    }
    let fresh_key = issue_as_peer(&linking, json!({ "principal": created_id })).await;
    refused(&fresh_key, 404, "keel.error.not_found");

    sync(b, Who::Unrelated, &other, "client-9").await;
    let foreign = linking
        .with_origin(&other, "client-9", "keel.account.list", json!({}))
        .await;
    refused(&foreign, 403, ERROR_ORIGIN);
    assert_eq!(foreign.json()["error"]["params"]["origin"], json!(other));
    let under_ours = linking
        .as_customer("client-9", "keel.account.list", json!({}))
        .await;
    customer_refused(&under_ours, &origin, "client-9");
    let foreign_key = issue_as_peer(&linking, id_of(Who::Unrelated)).await;
    refused(&foreign_key, 404, "keel.error.not_found");
    let foreign_external = issue_as_peer(&linking, json!({ "externalId": "client-9" })).await;
    field_refused(&foreign_external, "externalId", "keel.error.not_found");

    sync_principal(b, Who::Reseller, Some(&origin), "client-r").await;
    let hand_made_account = linking
        .as_customer("client-r", "keel.account.list", json!({}))
        .await;
    customer_refused(&hand_made_account, &origin, "client-r");
    let hand_made_account_key = issue_as_peer(&linking, id_of(Who::Reseller)).await;
    refused(&hand_made_account_key, 404, "keel.error.not_found");

    sync_principal(b, Who::A1, Some(&origin), "client-42").await;
    sync_account(b, Who::A1, &other, "acc-42").await;
    let foreign_account = linking
        .as_customer("client-42", "keel.account.list", json!({}))
        .await;
    customer_refused(&foreign_account, &origin, "client-42");
    let foreign_account_key = issue_as_peer(&linking, id_of(Who::A1)).await;
    refused(&foreign_account_key, 404, "keel.error.not_found");

    let crm = format!("crm:{}", linking.two.a.instance());
    sync_principal(b, Who::A1, Some(&crm), "client-42").await;
    sync_account(b, Who::A1, &origin, "acc-42").await;
    let another_name = linking
        .as_customer("client-42", "keel.account.list", json!({}))
        .await;
    customer_refused(&another_name, &origin, "client-42");
    let named = linking
        .with_origin(&crm, "client-42", "keel.account.list", json!({}))
        .await;
    assert_eq!(named.status, 200, "{}", named.body);
    assert_eq!(named.json()["meta"]["principal"]["login"], json!("a1"));
    let named_key = issue_as_peer(&linking, id_of(Who::A1)).await;
    assert_eq!(named_key.status, 200, "{}", named_key.body);

    sync(b, Who::Reseller, &origin, "client-r").await;
    for target in [id_of(Who::A2), json!({ "principal": created_id })] {
        let for_hand_made = issue_for(&linking, "client-r", target).await;
        refused(&for_hand_made, 404, "keel.error.not_found");
    }
    sync(b, Who::A2, &other, "client-8").await;
    let for_foreign = issue_for(&linking, "client-r", id_of(Who::A2)).await;
    refused(&for_foreign, 404, "keel.error.not_found");
    let for_foreign_external =
        issue_for(&linking, "client-r", json!({ "externalId": "client-8" })).await;
    field_refused(&for_foreign_external, "externalId", "keel.error.not_found");
    let for_ours = issue_for(&linking, "client-r", id_of(Who::A1)).await;
    assert_eq!(for_ours.status, 200, "{}", for_ours.body);
    let a1 = b.principal(Who::A1).id;
    let mut issued = b
        .database()
        .rows("SELECT principal_id || ' ' || issued_by FROM auth_login_key")
        .await;
    issued.sort();
    let mut expected = vec![
        format!("{a1} {}", link_principal(b, &linking.linked.b_link).await),
        format!("{a1} {}", b.principal(Who::Reseller).id),
    ];
    expected.sort();
    assert_eq!(issued, expected);

    linking.finish().await;
}

#[tokio::test]
async fn a_token_without_on_behalf_is_refused() {
    let linking = Linking::start(json!({
        "scopes": ["keel.account.*", "auth.token.create"], "level": "admin", "onBehalf": false
    }))
    .await;
    let b = linking.b();
    sync(b, Who::A1, &linking.origin, "client-42").await;

    let as_customer = linking
        .as_customer("client-42", "keel.account.list", json!({}))
        .await;
    refused(&as_customer, 403, REFUSED);
    let malformed = on_behalf(
        linking.http(),
        &linking.token,
        &["nonsense"],
        "keel.account.list",
        json!({}),
    )
    .await;
    refused(&malformed, 403, REFUSED);
    let unknown = linking
        .as_customer("client-0", "keel.account.list", json!({}))
        .await;
    refused(&unknown, 403, REFUSED);

    let direct = call_with(
        linking.http(),
        &linking.token,
        "keel.account.list",
        json!({}),
    )
    .await;
    assert_eq!(direct.status, 200, "{}", direct.body);
    assert_eq!(
        direct.json()["meta"]["principal"]["login"],
        json!(linking.peer_login())
    );

    let header = format!("origin={}; external_id=client-42", linking.origin);
    let minted = mint(linking.http(), &linking.token).await;
    let admin_token = b
        .call(Who::Admin, "auth.token.create", json!({ "name": "ops" }))
        .await;
    let admin_token = data(&admin_token)["token"].as_str().unwrap().to_owned();
    for token in [minted.as_str(), admin_token.as_str()] {
        let refused_token = on_behalf(
            linking.http(),
            token,
            &[header.as_str()],
            "keel.account.list",
            json!({}),
        )
        .await;
        refused(&refused_token, 403, REFUSED);
    }
    let guest = linking
        .http()
        .request(
            "POST",
            "/api/v1/call/keel.account.list",
            &[
                ("content-type", "application/json"),
                (ON_BEHALF_OF, header.as_str()),
            ],
            Some("{}"),
        )
        .await;
    refused(&guest, 403, REFUSED);

    linking.finish().await;
}

#[tokio::test]
async fn the_token_mask_and_level_ceiling_apply_on_top_of_the_customer() {
    let linking = Linking::start(
        json!({ "scopes": ["keel.account.list"], "level": "user", "onBehalf": true }),
    )
    .await;
    let b = linking.b();
    sync(b, Who::A1, &linking.origin, "client-42").await;
    sync(b, Who::Reseller, &linking.origin, "client-r").await;

    let within = linking
        .as_customer("client-42", "keel.account.list", json!({}))
        .await;
    assert_eq!(within.status, 200, "{}", within.body);
    assert_eq!(within.json()["data"]["total"], json!(1));
    let masked = linking
        .as_customer("client-42", "keel.principal.list", json!({}))
        .await;
    refused(&masked, 403, "keel.error.token_scope");
    assert_eq!(
        masked.json()["error"]["params"]["function"],
        json!("keel.principal.list")
    );
    let above = linking
        .as_customer("client-r", "keel.account.list", json!({}))
        .await;
    refused(&above, 403, "keel.error.token_level");
    let params = &above.json()["error"]["params"];
    assert_eq!(params["level"], json!("reseller"));
    assert_eq!(params["ceiling"], json!("user"));

    linking.finish().await;
}

#[tokio::test]
async fn an_on_behalf_header_out_of_its_grammar_is_refused() {
    let linking = Linking::start(
        json!({ "scopes": ["keel.account.list"], "level": "admin", "onBehalf": true }),
    )
    .await;
    let b = linking.b();
    sync(b, Who::A1, &linking.origin, "client-42").await;
    let whole = format!("origin={}; external_id=client-42", linking.origin);
    let origin_only = format!("origin={}", linking.origin);
    let lowered = format!(
        "origin=bill:{}; external_id=client-42",
        linking.two.a.instance().to_string().to_lowercase()
    );
    for headers in [
        vec!["nonsense"],
        vec![""],
        vec![origin_only.as_str()],
        vec!["external_id=client-42"],
        vec![lowered.as_str()],
        vec![origin_only.as_str(), "external_id=client-42"],
        vec![whole.as_str(), whole.as_str()],
    ] {
        let answer = on_behalf(
            linking.http(),
            &linking.token,
            &headers,
            "keel.account.list",
            json!({}),
        )
        .await;
        refused(&answer, 422, ERROR_HEADER);
    }
    let accepted = on_behalf(
        linking.http(),
        &linking.token,
        &[whole.as_str()],
        "keel.account.list",
        json!({}),
    )
    .await;
    assert_eq!(accepted.status, 200, "{}", accepted.body);

    linking.finish().await;
}

#[tokio::test]
async fn a_peer_names_the_target_of_a_login_key_by_its_external_id_as_itself_or_for_a_customer() {
    let linking = Linking::start(json!({
        "scopes": ["auth.key.issue", "keel.account.list"], "level": "admin", "onBehalf": true
    }))
    .await;
    let b = linking.b();
    let a1 = b.principal(Who::A1).clone();
    let reseller = b.principal(Who::Reseller).clone();
    sync(b, Who::Reseller, &linking.origin, "client-r").await;
    sync(b, Who::A1, &linking.origin, "client-42").await;
    sync_principal(b, Who::A2, None, "client-7").await;

    let issued = linking
        .as_customer(
            "client-r",
            "auth.key.issue",
            json!({ "externalId": "client-42", "ttl": 60, "redirect": REDIRECT }),
        )
        .await;
    assert_eq!(issued.status, 200, "{}", issued.body);
    let key = issued.json()["data"]["key"].as_str().unwrap().to_owned();
    assert_eq!(
        issued.json()["data"]["url"],
        json!(format!("/login?key={key}"))
    );
    assert_eq!(
        b.database()
            .rows("SELECT principal_id || ' ' || issued_by || ' ' || redirect FROM auth_login_key")
            .await,
        [format!("{} {} {REDIRECT}", a1.id, reseller.id)]
    );

    let by_id = linking
        .as_customer(
            "client-r",
            "auth.key.issue",
            json!({ "principal": a1.id, "ttl": 60, "redirect": REDIRECT }),
        )
        .await;
    assert_eq!(by_id.status, 200, "{}", by_id.body);
    for external_id in ["client-7", "client-0"] {
        let missing = linking
            .as_customer(
                "client-r",
                "auth.key.issue",
                json!({ "externalId": external_id, "ttl": 60, "redirect": REDIRECT }),
            )
            .await;
        field_refused(&missing, "externalId", "keel.error.not_found");
    }
    let self_issue = linking
        .as_customer(
            "client-r",
            "auth.key.issue",
            json!({ "externalId": "client-r", "ttl": 60, "redirect": REDIRECT }),
        )
        .await;
    refused(&self_issue, 403, "auth.error.not_below");
    let both = linking
        .as_customer(
            "client-r",
            "auth.key.issue",
            json!({ "principal": a1.id, "externalId": "client-42", "ttl": 60, "redirect": REDIRECT }),
        )
        .await;
    field_refused(&both, "externalId", "auth.error.key_target");
    let neither = linking
        .as_customer(
            "client-r",
            "auth.key.issue",
            json!({ "ttl": 60, "redirect": REDIRECT }),
        )
        .await;
    field_refused(&neither, "principal", "auth.error.key_target");

    let peer = link_principal(b, &linking.linked.b_link).await;
    let direct = issue_as_peer(&linking, json!({ "externalId": "client-42" })).await;
    assert_eq!(direct.status, 200, "{}", direct.body);
    let direct_key = direct.json()["data"]["key"].as_str().unwrap().to_owned();
    assert_eq!(
        direct.json()["data"]["url"],
        json!(format!("/login?key={direct_key}"))
    );
    assert_eq!(
        b.database()
            .rows(&format!(
                "SELECT principal_id FROM auth_login_key WHERE issued_by = '{peer}'"
            ))
            .await,
        [a1.id.to_string()]
    );
    let reseller_key = issue_as_peer(&linking, json!({ "externalId": "client-r" })).await;
    assert_eq!(reseller_key.status, 200, "{}", reseller_key.body);
    for external_id in ["client-7", "client-0"] {
        let missing = issue_as_peer(&linking, json!({ "externalId": external_id })).await;
        field_refused(&missing, "externalId", "keel.error.not_found");
    }

    let crm = format!("crm:{}", linking.two.a.instance());
    sync(b, Who::Unrelated, &crm, "client-42").await;
    let ambiguous = issue_as_peer(&linking, json!({ "externalId": "client-42" })).await;
    field_refused(&ambiguous, "externalId", "keel.error.not_found");
    let for_customer = issue_for(&linking, "client-r", json!({ "externalId": "client-42" })).await;
    assert_eq!(for_customer.status, 200, "{}", for_customer.body);
    sync(
        b,
        Who::Unrelated,
        &format!("bill:{OTHER_INSTANCE}"),
        "client-42",
    )
    .await;
    let alone = issue_as_peer(&linking, json!({ "externalId": "client-42" })).await;
    assert_eq!(alone.status, 200, "{}", alone.body);

    let local = b
        .call(
            Who::Admin,
            "auth.key.issue",
            json!({ "externalId": "client-42", "ttl": 60, "redirect": REDIRECT }),
        )
        .await;
    let error = local.error().unwrap_or_else(|| panic!("{local:?}"));
    assert_eq!(error.fields[0].path, "externalId", "{local:?}");
    assert_eq!(
        error.fields[0].key.as_deref(),
        Some("auth.error.external_id"),
        "{local:?}"
    );
    let direct_by_id = issue_as_peer(&linking, json!({ "principal": a1.id })).await;
    assert_eq!(direct_by_id.status, 200, "{}", direct_by_id.body);

    linking.finish().await;
}

#[cfg(feature = "settings")]
#[tokio::test]
async fn the_public_address_makes_the_login_url_of_a_peer_absolute() {
    let two = Two::of(
        installation().await,
        composed(Harness::new().module(&keel_system::settings::MODULE))
            .boot()
            .await,
    )
    .await;
    let linking = Linking::new(
        two,
        json!({ "scopes": ["auth.key.issue", "auth.key.handover"], "level": "admin", "onBehalf": true }),
    )
    .await;
    let b = linking.b();
    let a1 = b.principal(Who::A1).id;
    sync(b, Who::Reseller, &linking.origin, "client-r").await;
    sync(b, Who::A1, &linking.origin, "client-42").await;
    let public = "https://dns.example:8443";
    let set = b
        .call(
            Who::Admin,
            "settings.set",
            json!({ "key": keel_system::auth::SETTING_SHELL_URL, "value": format!("{public}/") }),
        )
        .await;
    assert_eq!(set.status(), 200, "{set:?}");

    let issued = linking
        .as_customer(
            "client-r",
            "auth.key.issue",
            json!({ "externalId": "client-42", "ttl": 60, "redirect": REDIRECT }),
        )
        .await;
    assert_eq!(issued.status, 200, "{}", issued.body);
    let key = issued.json()["data"]["key"].as_str().unwrap().to_owned();
    assert_eq!(
        issued.json()["data"]["url"],
        json!(format!("{public}/login?key={key}"))
    );
    let direct = call_with(
        linking.http(),
        &linking.token,
        "auth.key.issue",
        json!({ "principal": a1, "ttl": 60, "redirect": REDIRECT }),
    )
    .await;
    assert_eq!(direct.status, 200, "{}", direct.body);
    let direct_key = direct.json()["data"]["key"].as_str().unwrap().to_owned();
    assert_eq!(
        direct.json()["data"]["url"],
        json!(format!("{public}/login?key={direct_key}"))
    );
    let handed = linking
        .as_customer(
            "client-42",
            KEY_HANDOVER,
            json!({ "ttl": 60, "redirect": REDIRECT }),
        )
        .await;
    assert_eq!(handed.status, 200, "{}", handed.body);
    let handed_key = handed.json()["data"]["key"].as_str().unwrap().to_owned();
    assert_eq!(
        handed.json()["data"]["url"],
        json!(format!("{public}/login?key={handed_key}"))
    );
    let local = b
        .call(
            Who::Admin,
            "auth.key.issue",
            json!({ "principal": a1, "ttl": 60, "redirect": REDIRECT }),
        )
        .await;
    let local_key = data(&local)["key"].as_str().unwrap().to_owned();
    assert_eq!(
        data(&local)["url"],
        json!(format!("/login?key={local_key}"))
    );

    let unusable = b
        .call(
            Who::Admin,
            "settings.set",
            json!({ "key": keel_system::auth::SETTING_SHELL_URL, "value": "dns.example" }),
        )
        .await;
    assert_eq!(unusable.status(), 200, "{unusable:?}");
    let fallback = linking
        .as_customer(
            "client-r",
            "auth.key.issue",
            json!({ "externalId": "client-42", "ttl": 60, "redirect": REDIRECT }),
        )
        .await;
    let fallback_key = fallback.json()["data"]["key"].as_str().unwrap().to_owned();
    assert_eq!(
        fallback.json()["data"]["url"],
        json!(format!("/login?key={fallback_key}"))
    );

    linking.finish().await;
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

async fn handover(linking: &Linking, customer: &str, params: Value) -> Answer {
    linking.as_customer(customer, KEY_HANDOVER, params).await
}

#[tokio::test]
async fn a_peer_hands_its_customer_over_with_a_one_time_key_and_the_peer_as_actor() {
    let two = Two::of(
        installation().await,
        composed(Harness::new().module(&AUDITED)).boot().await,
    )
    .await;
    let linking = Linking::new(
        two,
        json!({ "scopes": [KEY_HANDOVER], "level": "reseller", "onBehalf": true }),
    )
    .await;
    let b = linking.b();
    let a1 = b.principal(Who::A1).clone();
    sync(b, Who::A1, &linking.origin, "client-42").await;
    let peer = link_principal(b, &linking.linked.b_link).await;
    let link: Id = linking.linked.b_link.parse().unwrap();

    let handed = handover(
        &linking,
        "client-42",
        json!({ "ttl": 60, "redirect": REDIRECT }),
    )
    .await;
    assert_eq!(handed.status, 200, "{}", handed.body);
    let key = handed.json()["data"]["key"].as_str().unwrap().to_owned();
    assert!(key.starts_with(LOGIN_KEY_PREFIX), "{key}");
    assert_eq!(
        handed.json()["data"]["url"],
        json!(format!("/login?key={key}"))
    );
    assert_eq!(
        b.database()
            .rows("SELECT principal_id || ' ' || issued_by || ' ' || redirect || ' ' || (used_at IS NULL)::text FROM auth_login_key")
            .await,
        [format!("{} {peer} {REDIRECT} true", a1.id)]
    );
    let request_id = handed
        .header("x-keel-request-id")
        .expect("the request id")
        .to_owned();
    let audited = RECORDER.of_request(&request_id);
    assert_eq!(audited.len(), 1, "{audited:?}");
    let event = &audited[0];
    assert_eq!(event.function, KEY_HANDOVER);
    assert_eq!(event.outcome, AuditOutcome::Committed);
    assert_eq!(event.principal.id, a1.id);
    assert_eq!(event.principal.via, Via::Peer(link));
    let actor = event.principal.actor.as_deref().expect("the peer acts");
    assert_eq!(actor.id.to_string(), peer);
    assert_eq!(actor.login, linking.peer_login());
    assert_eq!(actor.via, Via::Peer(link));
    assert_eq!(event.result, None);
    assert!(!serde_json::to_string(&audited).unwrap().contains(&key));

    let redeemed = redeem(linking.http(), &key).await;
    assert_eq!(redeemed.status, 200, "{}", redeemed.body);
    let answer = &redeemed.json()["data"];
    assert_eq!(answer["redirect"], json!(REDIRECT));
    assert_eq!(answer["issuedBy"], json!(peer));
    assert_eq!(answer["principal"]["id"], json!(a1.id));
    assert_eq!(answer["principal"]["login"], json!("a1"));
    assert_eq!(answer["principal"]["actor"], Value::Null);
    assert!(redeemed.session_cookie().is_some(), "{}", redeemed.body);
    let again = redeem(linking.http(), &key).await;
    refused(&again, 401, "auth.error.key");

    linking.finish().await;
}

#[tokio::test]
async fn a_handover_outside_a_call_for_a_customer_is_refused() {
    let linking = Linking::start(json!({
        "scopes": [KEY_HANDOVER, "auth.token.create"], "level": "reseller", "onBehalf": true
    }))
    .await;
    let b = linking.b();
    sync(b, Who::A1, &linking.origin, "client-42").await;
    let params = json!({ "ttl": 60, "redirect": REDIRECT });

    for who in [Who::A1, Who::Reseller, Who::Admin] {
        let local = b.call(who, KEY_HANDOVER, params.clone()).await;
        let error = local.error().unwrap_or_else(|| panic!("{who}: {local:?}"));
        assert_eq!(error.key, KEY_HANDOVER_REFUSED, "{who}: {local:?}");
        assert_eq!(local.status(), 403, "{who}: {local:?}");
    }
    let minted = mint(linking.http(), &linking.token).await;
    for (token, key) in [
        (linking.token.as_str(), KEY_HANDOVER_REFUSED),
        (minted.as_str(), "keel.error.level"),
    ] {
        let direct = call_with(linking.http(), token, KEY_HANDOVER, params.clone()).await;
        refused(&direct, 403, key);
    }
    assert!(
        b.database()
            .rows("SELECT key_hash FROM auth_login_key")
            .await
            .is_empty()
    );

    let narrow = Linking::start(json!({
        "scopes": ["keel.account.list"], "level": "reseller", "onBehalf": true
    }))
    .await;
    sync(narrow.b(), Who::A1, &narrow.origin, "client-42").await;
    let masked = handover(&narrow, "client-42", params.clone()).await;
    refused(&masked, 403, "keel.error.token_scope");
    narrow.finish().await;

    let handed = handover(&linking, "client-42", params).await;
    assert_eq!(handed.status, 200, "{}", handed.body);
    linking.finish().await;
}

#[tokio::test]
async fn a_customer_handed_over_lies_below_its_link_and_lands_inside_the_installation() {
    let linking = Linking::start(json!({
        "scopes": [KEY_HANDOVER], "level": "reseller", "onBehalf": true
    }))
    .await;
    let b = linking.b();
    sync(b, Who::A1, &linking.origin, "client-42").await;
    sync(b, Who::Reseller, &linking.origin, "client-r").await;

    let level = handover(
        &linking,
        "client-r",
        json!({ "ttl": 60, "redirect": REDIRECT }),
    )
    .await;
    refused(&level, 403, "auth.error.not_below");
    let params = &level.json()["error"]["params"];
    assert_eq!(params["level"], json!("reseller"));
    assert_eq!(params["caller"], json!("reseller"));
    for redirect in ["//evil.example", "https://evil.example", "/a\\b"] {
        let foreign = handover(
            &linking,
            "client-42",
            json!({ "ttl": 60, "redirect": redirect }),
        )
        .await;
        field_refused(&foreign, "redirect", "auth.error.redirect");
    }
    for ttl in [0, 301] {
        let out_of_range = handover(
            &linking,
            "client-42",
            json!({ "ttl": ttl, "redirect": REDIRECT }),
        )
        .await;
        assert_eq!(out_of_range.status, 422, "{}", out_of_range.body);
        assert_eq!(
            out_of_range.json()["error"]["fields"][0]["path"],
            json!("ttl"),
            "{}",
            out_of_range.body
        );
    }
    assert!(
        b.database()
            .rows("SELECT key_hash FROM auth_login_key")
            .await
            .is_empty()
    );
    let within = handover(
        &linking,
        "client-42",
        json!({ "ttl": 300, "redirect": "/f/dns.zone.list?search=a.example" }),
    )
    .await;
    assert_eq!(within.status, 200, "{}", within.body);
    assert_eq!(
        b.database()
            .rows("SELECT redirect FROM auth_login_key")
            .await,
        ["/f/dns.zone.list?search=a.example"]
    );

    linking.finish().await;
}
