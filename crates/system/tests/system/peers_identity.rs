use std::future::Future;
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::Duration;

use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ConnectionTrait, DbBackend, Statement};
use keel_sdk::serde_json;
use keel_sdk::system::SecretCipher;
use keel_sdk::types::{Event, EventSubject};
use keel_system::peers::{
    CREATE, ERROR_ORIGIN, HOOK_PATH, IDENTITY_MASKS, LOGIN_PREFIX, MAX_MASKS, UPSERTS,
};
use keel_system::webhooks::api::{EVENT_ID_HEADER, PEER_HEADER, SIGNATURE_HEADER, sign};
use keel_testkit::{Answer, Harness, Reply, TestHttp, Who, tokio};

use crate::peers::{
    Linked, Two, call_with, composed, data, installation, link_principal, mint, outbound, refused,
};

const PATIENCE: Duration = Duration::from_secs(40);
const SECRET: &str = "whs_fedcba9876543210fedcba9876543210fedcba9876543210fedcba98765432";
const OTHER_INSTANCE: &str = "01J9C00000000000000000000C";
const ACCOUNT_UPSERT: &str = "keel.account.upsert";
const PRINCIPAL_UPSERT: &str = "keel.principal.upsert";
const ON_BEHALF_OF: &str = "x-keel-on-behalf-of";
const LOCAL_ORIGIN: &str = "erp";

type Seen = Mutex<Vec<(Id, String)>>;

static SEEN: LazyLock<Seen> = LazyLock::new(Seen::default);

static PROBE: ModuleDef = ModuleDef {
    id: "identityprobe",
    version: "0.1.0",
    sdk: VERSION,
    requires: &[],
    migrations: None,
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.contribute(
        AccountLifecycle::new()
            .on_suspend(|call| record("suspend", call))
            .on_resume(|call| record("resume", call))
            .on_delete(|call| record("delete", call)),
    );
    Ok(())
}

async fn record(change: &str, call: Call<Id>) -> Result<()> {
    SEEN.lock().unwrap_or_else(PoisonError::into_inner).push((
        *call.params(),
        format!("{change} {}", call.principal().login),
    ));
    Ok(())
}

fn seen(account: Id) -> Vec<String> {
    SEEN.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter(|(of, _)| *of == account)
        .map(|(_, line)| line.clone())
        .collect()
}

async fn until<F, Fut>(what: &str, check: F)
where
    F: Fn() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while !check().await {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn rows(harness: &Harness, sql: &str) -> Vec<String> {
    harness
        .db()
        .query_all(Statement::from_string(DbBackend::Postgres, sql))
        .await
        .unwrap_or_else(|error| panic!("{sql}: {error}"))
        .iter()
        .map(|row| {
            row.try_get_by_index::<Option<String>>(0)
                .unwrap_or_else(|error| panic!("{sql}: {error}"))
                .unwrap_or_default()
        })
        .collect()
}

async fn execute(harness: &Harness, sql: &str) {
    harness
        .db()
        .execute_unprepared(sql)
        .await
        .unwrap_or_else(|error| panic!("{sql}: {error}"));
}

async fn login_of(harness: &Harness, principal: Id) -> Vec<String> {
    rows(
        harness,
        &format!("SELECT login FROM keel_principal WHERE id = '{principal}'"),
    )
    .await
}

async fn state(harness: &Harness, account: Id) -> Vec<String> {
    rows(
        harness,
        &format!("SELECT state FROM keel_account WHERE id = '{account}'"),
    )
    .await
}

fn id_of(value: &Value, member: &str) -> Id {
    value[member]
        .as_str()
        .unwrap_or_else(|| panic!("{member} in {value}"))
        .parse()
        .expect("a ulid")
}

fn answered(answer: &Answer) -> Value {
    assert_eq!(answer.status, 200, "{}", answer.body);
    answer.json()["data"].clone()
}

async fn upsert(harness: &Harness, func: &str, params: Value) -> Value {
    let reply = harness.call(Who::Admin, func, params).await;
    assert_eq!(reply.status(), 200, "{reply:?}");
    data(&reply).clone()
}

async fn copy_of(harness: &Harness, origin: &str, external_id: Id) -> Id {
    let upserted = upsert(
        harness,
        ACCOUNT_UPSERT,
        json!({ "origin": origin, "externalId": external_id.to_string() }),
    )
    .await;
    id_of(&upserted, "id")
}

fn identity_grants() -> Value {
    json!({ "scopes": [UPSERTS[0], UPSERTS[1], "auth.token.create"], "level": "admin", "identity": true })
}

async fn identity_pair(two: &Two) -> Linked {
    two.pair(json!({ "scopes": [], "level": "admin" }), identity_grants())
        .await
}

async fn on_behalf_pair(two: &Two, scopes: &[&str]) -> Linked {
    let mut grants = identity_grants();
    grants["scopes"] = json!(scopes);
    grants["onBehalf"] = json!(true);
    two.pair(json!({ "scopes": [], "level": "admin" }), grants)
        .await
}

async fn as_customer(
    http: &TestHttp,
    token: &str,
    customer: &str,
    func: &str,
    params: Value,
) -> Answer {
    let bearer = format!("Bearer {token}");
    http.request(
        "POST",
        &format!("/api/v1/call/{func}"),
        &[
            ("authorization", bearer.as_str()),
            ("content-type", "application/json"),
            (ON_BEHALF_OF, customer),
        ],
        Some(&params.to_string()),
    )
    .await
}

fn origin_refused(reply: &Reply, origin: &str) {
    assert_eq!(reply.status(), 403, "{reply:?}");
    let error = reply.error().unwrap_or_else(|| panic!("{reply:?}"));
    assert_eq!(error.key, ERROR_ORIGIN, "{reply:?}");
    assert_eq!(error.params["origin"], json!(origin), "{reply:?}");
}

fn synced_customer(origin: &str, account_origin: &str) -> Value {
    json!({
        "origin": origin, "externalId": "client-42", "login": "alice", "level": "user",
        "account": { "origin": account_origin, "externalId": "acc-7" },
    })
}

struct Fabricated {
    harness: Harness,
    http: TestHttp,
    instance: Id,
    link: String,
    login: String,
}

impl Fabricated {
    async fn start(harness: Harness, identity: bool) -> Self {
        let http = harness.http().await;
        let instance = Id::generate();
        let link = Id::generate().to_string();
        let principal = Id::generate();
        let login = format!("{LOGIN_PREFIX}{}", link.to_lowercase());
        let sealed = harness
            .registry()
            .services()
            .get::<dyn SecretCipher>()
            .expect("the kernel seals with the data key")
            .seal(SECRET.as_bytes())
            .unwrap();
        execute(
            &harness,
            &format!(
                "INSERT INTO keel_principal (id, account_id, login, level, role, created_at, updated_at) \
                 VALUES ('{principal}', '{}', '{login}', 'service', 'peer', now(), now())",
                Id::ROOT_ACCOUNT
            ),
        )
        .await;
        execute(
            &harness,
            &format!(
                "INSERT INTO peer_product (id, instance_id, products_json, keel_version, protocol, url, \
                 public_key, inbound_token_hash, inbound_scopes_json, outbound_token_enc, \
                 webhook_secret_enc, status, created_at, principal_id, identity) VALUES ('{link}', '{instance}', '[]', '0.3.0', 1, \
                 NULL, 'ed25519:{link}', 'hash-{link}', '{{\"functions\": [], \"level\": \"service\", \"onBehalf\": false}}', \
                 'sealed-{link}', '{sealed}', 'active', now(), '{principal}', {identity})"
            ),
        )
        .await;
        Self {
            harness,
            http,
            instance,
            link,
            login,
        }
    }

    fn origin(&self) -> String {
        format!("bill:{}", self.instance)
    }

    fn event(&self, seq: u64, kind: &str, subject: (&str, Id), data: Value) -> Event {
        Event {
            id: Id::generate(),
            seq,
            kind: kind.to_owned(),
            at: self.harness.clock().now(),
            instance_id: self.instance,
            module: "keel".into(),
            actor: None,
            subject: Some(EventSubject {
                kind: subject.0.to_owned(),
                id: subject.1,
                account_id: Some(subject.1),
            }),
            data,
        }
    }

    async fn deliver(&self, event: &Event) {
        let body = serde_json::to_vec(event).expect("an event serialises");
        let signature = sign(
            SECRET.as_bytes(),
            self.harness.clock().now().timestamp(),
            &body,
        )
        .unwrap();
        let instance = self.instance.to_string();
        let id = event.id.to_string();
        let answer = self
            .http
            .try_request_bytes(
                "POST",
                HOOK_PATH,
                &[
                    ("content-type", "application/json"),
                    (PEER_HEADER, instance.as_str()),
                    (EVENT_ID_HEADER, id.as_str()),
                    (SIGNATURE_HEADER, signature.as_str()),
                ],
                &body,
            )
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(answer.status, 200, "{}", answer.body);
        let sql = format!(
            "SELECT (processed_at IS NOT NULL)::text FROM peer_inbox WHERE event_id = '{id}'"
        );
        until("the inbox processes the event", || async {
            rows(&self.harness, &sql).await == ["true"]
        })
        .await;
    }

    async fn account_event(&self, seq: u64, kind: &str, subject: Id) {
        let state = kind.rsplit('.').next().unwrap_or_default();
        let event = self.event(
            seq,
            kind,
            ("keel.account", subject),
            json!({ "id": subject, "state": state }),
        );
        self.deliver(&event).await;
    }

    async fn applied(&self) -> Vec<String> {
        rows(
            &self.harness,
            &format!(
                "SELECT subject_kind || ' ' || subject_id || ' ' || seq FROM peer_identity WHERE peer_id = '{}' ORDER BY subject_kind, subject_id",
                self.link
            ),
        )
        .await
    }

    async fn finish(self) {
        self.http.stop().await.unwrap();
        self.harness.finish().await;
    }
}

async fn probed() -> Harness {
    composed(Harness::new().module(&PROBE)).boot().await
}

#[tokio::test]
async fn upsert_by_origin_and_external_id_twice_yields_one_account() {
    let two = Two::start().await;
    let linked = identity_pair(&two).await;
    let token = outbound(&two.b, &linked.b_link).await;
    let origin = format!("bill:{}", two.b.instance());
    let account = json!({ "origin": origin, "externalId": "acc-7", "kind": "user" });

    let first = answered(&call_with(&two.a_http, &token, ACCOUNT_UPSERT, account.clone()).await);
    assert_eq!(first["created"], json!(true));
    let id = id_of(&first, "id");
    let second = answered(&call_with(&two.a_http, &token, ACCOUNT_UPSERT, account.clone()).await);
    assert_eq!(second, json!({ "id": id.to_string(), "created": false }));
    assert_eq!(
        rows(
            &two.a,
            &format!(
                "SELECT id FROM keel_account WHERE origin = '{origin}' AND external_id = 'acc-7'"
            )
        )
        .await,
        [id.to_string()]
    );

    let customer = json!({
        "origin": origin, "externalId": "client-42", "login": "alice", "level": "user", "account": account,
    });
    let first = answered(&call_with(&two.a_http, &token, PRINCIPAL_UPSERT, customer.clone()).await);
    assert_eq!(first["created"], json!(true));
    assert_eq!(id_of(&first, "accountId"), id);
    let principal = id_of(&first, "id");
    let second = answered(&call_with(&two.a_http, &token, PRINCIPAL_UPSERT, customer).await);
    assert_eq!(
        second,
        json!({ "id": principal.to_string(), "accountId": id.to_string(), "created": false })
    );
    assert_eq!(
        rows(
            &two.a,
            &format!("SELECT id || ' ' || account_id || ' ' || login FROM keel_principal WHERE origin = '{origin}'")
        )
        .await,
        [format!("{principal} {id} alice")]
    );
    let actor = link_principal(&two.a, &linked.a_link).await;
    assert_eq!(
        rows(
            &two.a,
            &format!(
                "SELECT type || ' ' || (actor_json->>'kind') || ' ' || (actor_json->>'id') FROM keel_outbox WHERE subject_json->>'id' IN ('{id}', '{principal}') ORDER BY seq"
            )
        )
        .await,
        [
            format!("keel.account.created peer {actor}"),
            format!("keel.principal.created peer {actor}"),
        ]
    );
    two.finish().await;
}

#[tokio::test]
async fn an_account_suspended_on_the_identity_peer_is_suspended_here_with_its_lifecycle() {
    let two = Two::of(probed().await, installation().await).await;
    let linked = identity_pair(&two).await;
    assert_eq!(
        rows(
            &two.a,
            "SELECT identity::text || ' ' || (pull_seq IS NOT NULL)::text FROM peer_product"
        )
        .await,
        ["true true"]
    );
    assert_eq!(
        rows(
            &two.b,
            "SELECT identity::text || ' ' || array_to_string(events, ',') FROM peer_product"
        )
        .await,
        [format!("false {}", IDENTITY_MASKS.join(","))]
    );
    let got = two
        .a
        .call(Who::Admin, "peer.get", json!({ "id": linked.a_link }))
        .await;
    assert_eq!(data(&got)["identity"], json!(true));

    let created = two
        .b
        .call(Who::Admin, "keel.account.create", json!({ "kind": "user" }))
        .await;
    let billed = id_of(data(&created), "id");
    let token = outbound(&two.b, &linked.b_link).await;
    let origin = format!("bill:{}", two.b.instance());
    let copied = answered(
        &call_with(
            &two.a_http,
            &token,
            ACCOUNT_UPSERT,
            json!({ "origin": origin, "externalId": billed.to_string() }),
        )
        .await,
    );
    let copy = id_of(&copied, "id");
    let stranger = copy_of(&two.a, &format!("crm:{OTHER_INSTANCE}"), billed).await;

    let suspended = two
        .b
        .call(Who::Admin, "keel.account.suspend", json!({ "id": billed }))
        .await;
    assert_eq!(suspended.status(), 200, "{suspended:?}");
    until("the copy is suspended here", || async {
        state(&two.a, copy).await == ["suspended"]
    })
    .await;
    let login = format!("{LOGIN_PREFIX}{}", linked.a_link.to_lowercase());
    until("the lifecycle of the copy runs", || async {
        seen(copy) == [format!("suspend {login}")]
    })
    .await;
    assert_eq!(state(&two.a, stranger).await, ["active"]);
    assert!(seen(stranger).is_empty());
    let b_admin = two.b.principal(Who::Admin).id;
    let actor = link_principal(&two.a, &linked.a_link).await;
    assert_eq!(
        rows(
            &two.a,
            "SELECT coalesce(instance_id, 'here') || ' ' || (subject_json->>'id') || ' ' || (actor_json->>'id') FROM keel_outbox WHERE type = 'keel.account.suspended' ORDER BY seq"
        )
        .await,
        [
            format!("{} {billed} {b_admin}", two.b.instance()),
            format!("here {copy} {actor}"),
        ]
    );
    let seq = rows(
        &two.b,
        &format!("SELECT seq::text FROM keel_outbox WHERE type = 'keel.account.suspended' AND subject_json->>'id' = '{billed}'"),
    )
    .await;
    assert_eq!(
        rows(
            &two.a,
            "SELECT subject_kind || ' ' || subject_id || ' ' || seq FROM peer_identity"
        )
        .await,
        [format!("keel.account {billed} {}", seq[0])]
    );

    let resumed = two
        .b
        .call(Who::Admin, "keel.account.resume", json!({ "id": billed }))
        .await;
    assert_eq!(resumed.status(), 200, "{resumed:?}");
    until("the copy is resumed here", || async {
        state(&two.a, copy).await == ["active"]
    })
    .await;
    until("the lifecycle of the copy runs again", || async {
        seen(copy) == [format!("suspend {login}"), format!("resume {login}")]
    })
    .await;
    two.finish().await;
}

#[tokio::test]
async fn a_peer_upserts_only_under_an_origin_naming_it() {
    let two = Two::start().await;
    let linked = identity_pair(&two).await;
    let token = outbound(&two.b, &linked.b_link).await;
    let own = format!("bill:{}", two.b.instance());
    let foreign = format!("bill:{OTHER_INSTANCE}");
    let account =
        |origin: &str, external_id: &str| json!({ "origin": origin, "externalId": external_id });
    let customer = |origin: &str, account: Value| json!({ "origin": origin, "externalId": "client-42", "login": "alice", "level": "user", "account": account });

    for (func, params, origin) in [
        (ACCOUNT_UPSERT, account(&foreign, "acc-7"), &foreign),
        (
            PRINCIPAL_UPSERT,
            customer(&foreign, account(&own, "acc-7")),
            &foreign,
        ),
        (
            PRINCIPAL_UPSERT,
            customer(&own, account(&foreign, "acc-7")),
            &foreign,
        ),
    ] {
        let answer = call_with(&two.a_http, &token, func, params.clone()).await;
        refused(&answer, 403, ERROR_ORIGIN);
        assert_eq!(
            answer.json()["error"]["params"]["origin"],
            json!(origin),
            "{params}"
        );
    }
    assert_eq!(
        rows(
            &two.a,
            "SELECT count(*)::text FROM keel_account WHERE external_id IS NOT NULL"
        )
        .await,
        ["0"]
    );
    let minted = mint(&two.a_http, &token).await;
    refused(
        &call_with(
            &two.a_http,
            &minted,
            ACCOUNT_UPSERT,
            account(&foreign, "acc-7"),
        )
        .await,
        403,
        "keel.error.level",
    );
    let actor = link_principal(&two.a, &linked.a_link).await;
    execute(
        &two.a,
        &format!("UPDATE keel_principal SET level = 'admin' WHERE id = '{actor}'"),
    )
    .await;
    refused(
        &call_with(
            &two.a_http,
            &minted,
            ACCOUNT_UPSERT,
            account(&foreign, "acc-7"),
        )
        .await,
        403,
        ERROR_ORIGIN,
    );
    let crm = format!("crm:{}", two.b.instance());
    answered(&call_with(&two.a_http, &minted, ACCOUNT_UPSERT, account(&crm, "acc-8")).await);
    answered(
        &call_with(
            &two.a_http,
            &token,
            PRINCIPAL_UPSERT,
            customer(&own, account(&own, "acc-7")),
        )
        .await,
    );
    upsert(&two.a, ACCOUNT_UPSERT, account(&foreign, "acc-9")).await;
    assert_eq!(
        rows(&two.a, "SELECT origin || ' ' || external_id FROM keel_account WHERE external_id IS NOT NULL ORDER BY external_id").await,
        [format!("{own} acc-7"), format!("{crm} acc-8"), format!("{foreign} acc-9")]
    );
    two.finish().await;
}

#[tokio::test]
async fn a_caller_below_admin_places_no_record_under_an_origin_naming_an_instance() {
    let two = Two::start().await;
    let peer = format!("bill:{}", two.b.instance());
    let unpaired = format!("bill:{OTHER_INSTANCE}");
    let squat = |origin: &str| json!({ "origin": origin, "externalId": "acc-7" });
    for origin in [&peer, &unpaired] {
        for (func, params) in [
            (ACCOUNT_UPSERT, squat(origin)),
            (PRINCIPAL_UPSERT, synced_customer(origin, LOCAL_ORIGIN)),
            (PRINCIPAL_UPSERT, synced_customer(LOCAL_ORIGIN, origin)),
        ] {
            origin_refused(&two.a.call(Who::Reseller, func, params).await, origin);
        }
    }
    let mut local = synced_customer(LOCAL_ORIGIN, LOCAL_ORIGIN);
    local["login"] = json!("bob");
    let local = two.a.call(Who::Reseller, PRINCIPAL_UPSERT, local).await;
    assert_eq!(local.status(), 200, "{local:?}");
    let reseller = two.a.principal(Who::Reseller).account_id;
    assert_eq!(
        rows(
            &two.a,
            "SELECT parent_id FROM keel_account WHERE external_id IS NOT NULL"
        )
        .await,
        [reseller.to_string()]
    );

    let linked = identity_pair(&two).await;
    origin_refused(
        &two.a
            .call(Who::Reseller, ACCOUNT_UPSERT, squat(&peer))
            .await,
        &peer,
    );
    let token = outbound(&two.b, &linked.b_link).await;
    let synced = answered(
        &call_with(
            &two.a_http,
            &token,
            PRINCIPAL_UPSERT,
            synced_customer(&peer, &peer),
        )
        .await,
    );
    assert_eq!(synced["created"], json!(true));
    let account = id_of(&synced, "accountId");
    assert_eq!(
        rows(
            &two.a,
            &format!("SELECT path FROM keel_account WHERE id = '{account}'")
        )
        .await,
        [format!("/{account}/")],
        "the peer's customer lies under the root, out of the reseller's subtree"
    );
    let reached = two
        .a
        .call(Who::Reseller, "keel.account.get", json!({ "id": account }))
        .await;
    assert_eq!(reached.status(), 404, "{reached:?}");
    assert_eq!(
        rows(
            &two.a,
            "SELECT origin || ' ' || external_id FROM keel_account WHERE external_id IS NOT NULL ORDER BY origin"
        )
        .await,
        [format!("{peer} acc-7"), format!("{LOCAL_ORIGIN} acc-7")]
    );
    two.finish().await;
}

#[tokio::test]
async fn a_customers_own_credential_minted_by_its_peer_is_bound_like_a_local_caller() {
    let two = Two::start().await;
    let linked = on_behalf_pair(&two, &[UPSERTS[0], UPSERTS[1], "auth.token.create"]).await;
    let token = outbound(&two.b, &linked.b_link).await;
    let peer = format!("bill:{}", two.b.instance());
    let synced = answered(
        &call_with(
            &two.a_http,
            &token,
            PRINCIPAL_UPSERT,
            json!({
                "origin": peer, "externalId": "client-42", "login": "rita", "level": "reseller",
                "account": { "origin": peer, "externalId": "acc-r", "kind": "reseller" },
            }),
        )
        .await,
    );
    let customer_account = id_of(&synced, "accountId");
    let customer = format!("origin={peer}; external_id=client-42");
    let created = as_customer(
        &two.a_http,
        &token,
        &customer,
        "auth.token.create",
        json!({ "name": "own" }),
    )
    .await;
    let own = answered(&created)["token"]
        .as_str()
        .unwrap_or_else(|| panic!("{}", created.body))
        .to_owned();

    let squat = |origin: &str| json!({ "origin": origin, "externalId": "acc-s" });
    let answer = call_with(&two.a_http, &own, ACCOUNT_UPSERT, squat(&peer)).await;
    refused(&answer, 403, ERROR_ORIGIN);
    assert_eq!(answer.json()["error"]["params"]["origin"], json!(peer));
    answered(&call_with(&two.a_http, &own, ACCOUNT_UPSERT, squat(LOCAL_ORIGIN)).await);
    let placed =
        answered(&as_customer(&two.a_http, &token, &customer, ACCOUNT_UPSERT, squat(&peer)).await);
    let placed = id_of(&placed, "id");
    assert_eq!(
        rows(
            &two.a,
            "SELECT origin || ' ' || parent_id FROM keel_account WHERE external_id = 'acc-s' ORDER BY origin"
        )
        .await,
        [
            format!("{peer} {customer_account}"),
            format!("{LOCAL_ORIGIN} {customer_account}"),
        ],
        "acting for the customer, the peer places {placed} under its own origin"
    );
    two.finish().await;
}

#[tokio::test]
async fn every_identity_the_upsert_accepts_stays_reachable_on_behalf() {
    let two = Two::start().await;
    let linked = on_behalf_pair(&two, &[UPSERTS[0], UPSERTS[1], "keel.principal.get"]).await;
    let token = outbound(&two.b, &linked.b_link).await;
    let peer = format!("bill:{}", two.b.instance());
    let alphabet: String = (b'!'..=b'~')
        .filter(|byte| !matches!(byte, b'"' | b',' | b';' | b'\\'))
        .map(char::from)
        .collect();
    let synced = answered(
        &call_with(
            &two.a_http,
            &token,
            PRINCIPAL_UPSERT,
            json!({
                "origin": peer, "externalId": alphabet, "login": "alice", "level": "user",
                "account": { "origin": peer, "externalId": alphabet },
            }),
        )
        .await,
    );
    let principal = id_of(&synced, "id");
    let customer = format!("origin={peer}; external_id={alphabet}");
    let got = answered(
        &as_customer(
            &two.a_http,
            &token,
            &customer,
            "keel.principal.get",
            json!({ "id": principal }),
        )
        .await,
    );
    assert_eq!(got["id"], json!(principal.to_string()));
    assert_eq!(
        rows(
            &two.a,
            &format!("SELECT external_id FROM keel_principal WHERE id = '{principal}'")
        )
        .await,
        std::slice::from_ref(&alphabet)
    );

    for separator in ['"', ',', ';', '\\'] {
        let external_id = format!("client{separator}42");
        let answer = call_with(
            &two.a_http,
            &token,
            ACCOUNT_UPSERT,
            json!({ "origin": peer, "externalId": external_id }),
        )
        .await;
        refused(&answer, 422, "keel.error.validation");
        let field = &answer.json()["error"]["fields"][0];
        assert_eq!(field["path"], json!("externalId"), "{}", answer.body);
        assert_eq!(
            field["key"],
            json!("keel.schema.pattern"),
            "{}",
            answer.body
        );
    }
    two.finish().await;
}

#[tokio::test]
async fn only_a_link_with_the_identity_role_applies_the_identity_events_of_its_peer() {
    let peer = Fabricated::start(probed().await, false).await;
    let billed = Id::generate();
    let copy = copy_of(&peer.harness, &peer.origin(), billed).await;
    peer.account_event(1, "keel.account.suspended", billed)
        .await;
    assert_eq!(state(&peer.harness, copy).await, ["active"]);
    assert!(seen(copy).is_empty());
    assert!(peer.applied().await.is_empty());
    assert_eq!(
        rows(
            &peer.harness,
            &format!(
                "SELECT count(*)::text FROM keel_outbox WHERE instance_id = '{}'",
                peer.instance
            )
        )
        .await,
        ["1"],
        "the event still reaches the bus"
    );

    execute(
        &peer.harness,
        &format!(
            "UPDATE peer_product SET identity = true WHERE id = '{}'",
            peer.link
        ),
    )
    .await;
    peer.account_event(2, "keel.account.suspended", billed)
        .await;
    assert_eq!(state(&peer.harness, copy).await, ["suspended"]);
    until("the lifecycle runs", || async {
        seen(copy) == [format!("suspend {}", peer.login)]
    })
    .await;
    assert_eq!(peer.applied().await, [format!("keel.account {billed} 2")]);
    peer.finish().await;
}

#[tokio::test]
async fn an_identity_event_older_than_the_last_one_applied_to_its_record_is_skipped() {
    let peer = Fabricated::start(probed().await, true).await;
    let billed = Id::generate();
    let copy = copy_of(&peer.harness, &peer.origin(), billed).await;
    peer.account_event(10, "keel.account.suspended", billed)
        .await;
    assert_eq!(state(&peer.harness, copy).await, ["suspended"]);
    peer.account_event(9, "keel.account.resumed", billed).await;
    assert_eq!(state(&peer.harness, copy).await, ["suspended"]);
    assert_eq!(peer.applied().await, [format!("keel.account {billed} 10")]);
    peer.account_event(11, "keel.account.resumed", billed).await;
    assert_eq!(state(&peer.harness, copy).await, ["active"]);
    assert_eq!(peer.applied().await, [format!("keel.account {billed} 11")]);
    until("the lifecycle runs twice", || async {
        seen(copy)
            == [
                format!("suspend {}", peer.login),
                format!("resume {}", peer.login),
            ]
    })
    .await;

    let deleted = peer
        .harness
        .call(Who::Admin, "peer.delete", json!({ "id": peer.link }))
        .await;
    assert_eq!(deleted.status(), 200, "{deleted:?}");
    assert!(
        peer.applied().await.is_empty(),
        "the applied events go with the link"
    );
    peer.finish().await;
}

#[tokio::test]
async fn an_identity_event_changes_the_one_record_its_peer_synced_and_nothing_else() {
    let peer = Fabricated::start(probed().await, true).await;
    let shared = Id::generate();
    let first = copy_of(&peer.harness, &peer.origin(), shared).await;
    let second = copy_of(&peer.harness, &format!("crm:{}", peer.instance), shared).await;
    peer.account_event(1, "keel.account.suspended", shared)
        .await;
    assert_eq!(state(&peer.harness, first).await, ["active"]);
    assert_eq!(state(&peer.harness, second).await, ["active"]);
    assert!(
        peer.applied().await.is_empty(),
        "two records under the peer's origins are ambiguous"
    );

    let elsewhere = Id::generate();
    let other = copy_of(&peer.harness, &format!("bill:{OTHER_INSTANCE}"), elsewhere).await;
    peer.account_event(2, "keel.account.suspended", elsewhere)
        .await;
    assert_eq!(state(&peer.harness, other).await, ["active"]);
    assert!(
        peer.applied().await.is_empty(),
        "a record of another peer is out of reach"
    );

    let person = Id::generate();
    let customer = upsert(
        &peer.harness,
        PRINCIPAL_UPSERT,
        json!({
            "origin": peer.origin(), "externalId": person.to_string(), "login": "alice", "level": "user",
            "account": { "origin": peer.origin(), "externalId": "acc-p" },
        }),
    )
    .await;
    let principal = id_of(&customer, "id");
    let renamed = |seq: u64, to: &str| {
        peer.event(
            seq,
            "keel.principal.renamed",
            ("keel.principal", person),
            json!({ "id": person, "login": to }),
        )
    };
    peer.deliver(&renamed(3, "alice2")).await;
    assert_eq!(login_of(&peer.harness, principal).await, ["alice2"]);
    assert_eq!(
        rows(
            &peer.harness,
            &format!("SELECT data_json->>'login' FROM keel_outbox WHERE type = 'keel.principal.renamed' AND instance_id IS NULL AND subject_json->>'id' = '{principal}'")
        )
        .await,
        ["alice2"]
    );
    upsert(
        &peer.harness,
        "keel.principal.create",
        json!({ "login": "bob", "level": "user" }),
    )
    .await;
    peer.deliver(&renamed(4, "bob")).await;
    assert_eq!(
        login_of(&peer.harness, principal).await,
        ["alice2"],
        "a taken login is refused here"
    );
    assert_eq!(peer.applied().await, [format!("keel.principal {person} 4")]);

    let stray = Id::generate();
    let foreign_account = upsert(
        &peer.harness,
        PRINCIPAL_UPSERT,
        json!({
            "origin": peer.origin(), "externalId": stray.to_string(), "login": "carol", "level": "user",
            "account": { "origin": format!("bill:{OTHER_INSTANCE}"), "externalId": "acc-q" },
        }),
    )
    .await;
    let stray_principal = id_of(&foreign_account, "id");
    peer.deliver(&peer.event(
        5,
        "keel.principal.renamed",
        ("keel.principal", stray),
        json!({ "id": stray, "login": "carol2" }),
    ))
    .await;
    assert_eq!(
        login_of(&peer.harness, stray_principal).await,
        ["carol"],
        "a principal in an account of another peer is out of reach"
    );
    peer.finish().await;
}

#[cfg(feature = "tasks")]
#[tokio::test]
async fn an_account_deleted_on_the_identity_peer_is_deleted_here_by_the_task() {
    let harness = composed(
        Harness::new()
            .module(&keel_system::tasks::MODULE)
            .module(&PROBE),
    )
    .boot()
    .await;
    let peer = Fabricated::start(harness, true).await;
    let billed = Id::generate();
    let copy = copy_of(&peer.harness, &peer.origin(), billed).await;
    peer.account_event(1, "keel.account.deleted", billed).await;
    until("the copy is deleted here", || async {
        state(&peer.harness, copy).await.is_empty()
    })
    .await;
    assert!(
        seen(copy).contains(&format!("delete {}", peer.login)),
        "{:?}",
        seen(copy)
    );
    let principal = rows(
        &peer.harness,
        &format!(
            "SELECT principal_id FROM peer_product WHERE id = '{}'",
            peer.link
        ),
    )
    .await;
    until("the deletion task is done", || async {
        rows(
            &peer.harness,
            &format!("SELECT function || ' ' || principal_id || ' ' || state FROM task_job WHERE params_json->>'id' = '{copy}'"),
        )
        .await
            == [format!("keel.account.delete {} done", principal[0])]
    })
    .await;
    assert_eq!(peer.applied().await, [format!("keel.account {billed} 1")]);
    peer.finish().await;
}

#[tokio::test]
async fn the_identity_role_counts_its_masks_against_the_limit_of_a_subscription() {
    let harness = installation().await;
    let asked: Vec<String> = (0..MAX_MASKS - 1)
        .map(|index| format!("probe.n{index}.*"))
        .collect();
    let answer = harness
        .call(
            Who::Admin,
            CREATE,
            json!({
                "url": "http://127.0.0.1:9",
                "code": "PAIR-AAAA-AAAA",
                "subscribe": asked,
                "identity": true,
            }),
        )
        .await;
    let error = answer.error().unwrap_or_else(|| panic!("{answer:?}"));
    assert_eq!(error.fields[0].path, "subscribe", "{answer:?}");
    assert_eq!(error.fields[0].key.as_deref(), Some("keel.schema.maxItems"));
    assert_eq!(error.fields[0].params["max"], json!(MAX_MASKS));
    assert_eq!(
        rows(&harness, "SELECT count(*)::text FROM peer_product").await,
        ["0"]
    );
    harness.finish().await;
}
