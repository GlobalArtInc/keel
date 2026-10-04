use std::marker::PhantomData;

use keel_sdk::async_trait::async_trait;
use keel_sdk::chrono::{DateTime, Duration, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait};
use keel_sdk::svc::{LoginTarget, OpenRequest, Opened, Period, Service, ServiceAccount};
use keel_sdk::types::InstanceInfo;
use keel_sdk::{Registrar, ServiceKind, ServiceKindDef, serde_json};
use keel_system::svc::{
    ERROR_CLOSED, ERROR_EXISTS, ERROR_NOT_FOUND, ERROR_PERIOD, EVENT_CLOSED, EVENT_OPENED,
    EVENT_RESUMED, EVENT_SUSPENDED, EVENT_UPDATED, FEATURE, KINDS, MODULE, RETENTION_DAYS,
    lock_key,
};
use keel_testkit::tokio::sync::oneshot;
use keel_testkit::{CallOptions, Harness, Reply, Who, tokio};

const BOXES: &str = "depot.boxes";
const CRATES: &str = "depot.crates";
const ORIGIN: &str = "bill";
const REQUIRED: &str = "keel.error.idempotency_required";
const NOW: &str = "2026-10-04T12:00:00Z";

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/tests/system/modules/depot/migrations")
        .table("keel_migration_depot");

pub(crate) static DEPOT: ModuleDef = ModuleDef {
    id: "depot",
    version: "0.1.0",
    sdk: VERSION,
    requires: &[],
    migrations: Some(&MIGRATIONS),
    init,
};

const EN: &str = r#"{ "depot": { "service": { "boxes": "Boxes", "crates": "Crates" } } }"#;

fn init(r: &mut Registrar) -> Result<()> {
    r.contribute(ServiceKindDef::new(Depot::<Boxes>::default()))
        .contribute(ServiceKindDef::new(Depot::<Crates>::default()))
        .i18n("en", EN);
    Ok(())
}

trait Named: Send + Sync + 'static {
    const KIND: &'static str;
    const TITLE: &'static str;
}

struct Boxes;
struct Crates;

impl Named for Boxes {
    const KIND: &'static str = BOXES;
    const TITLE: &'static str = "depot.service.boxes";
}

impl Named for Crates {
    const KIND: &'static str = CRATES;
    const TITLE: &'static str = "depot.service.crates";
}

struct Depot<N>(PhantomData<N>);

impl<N> Default for Depot<N> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Quota {
    #[schemars(range(min = 1, max = 100))]
    quota: u32,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Usage {
    quota: u32,
    active: bool,
}

type Gate = (String, oneshot::Sender<()>, oneshot::Receiver<()>);

static GATES: std::sync::Mutex<Vec<Gate>> = std::sync::Mutex::new(Vec::new());

fn gate(external_id: &str) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (entered, reached) = oneshot::channel();
    let (release, released) = oneshot::channel();
    GATES
        .lock()
        .unwrap()
        .push((external_id.to_owned(), entered, released));
    (reached, release)
}

async fn pass_gate(external_id: &str) {
    let held = {
        let mut gates = GATES.lock().unwrap();
        gates
            .iter()
            .position(|(gated, ..)| gated == external_id)
            .map(|at| gates.swap_remove(at))
    };
    if let Some((_, entered, released)) = held {
        let _ = entered.send(());
        let _ = released.await;
    }
}

async fn execute<P>(
    call: &Call<P>,
    sql: &str,
    values: Vec<keel_sdk::sea_orm::Value>,
) -> Result<()> {
    call.tx()?
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .await?;
    Ok(())
}

async fn usage<P>(call: &Call<P>, service: &Service) -> Result<Usage> {
    let row = call
        .db()
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT quota, active FROM depot_box WHERE service_id = $1",
            [service.id.to_string().into()],
        ))
        .await?
        .ok_or_else(|| KeelError::not_found("depot.error.box"))?;
    let quota: i32 = row.try_get("", "quota")?;
    Ok(Usage {
        quota: u32::try_from(quota).unwrap_or_default(),
        active: row.try_get("", "active")?,
    })
}

#[async_trait]
impl<N: Named> ServiceKind for Depot<N> {
    const KIND: &'static str = N::KIND;
    const TITLE: &'static str = N::TITLE;
    const SCHEMA_VERSION: u32 = 2;
    type Params = Quota;
    type State = Usage;
    type Account = ServiceAccount;

    async fn open(
        &self,
        call: &Call<()>,
        request: OpenRequest<ServiceAccount, Quota>,
    ) -> Result<Opened<Usage>> {
        execute(
            call,
            "INSERT INTO depot_box (service_id, kind, account_id, quota, active) VALUES ($1, $2, $3, $4, true)",
            vec![
                request.id.to_string().into(),
                N::KIND.into(),
                request.account_id.to_string().into(),
                i32::try_from(request.params.quota).unwrap_or_default().into(),
            ],
        )
        .await?;
        Ok(Opened {
            state: Usage {
                quota: request.params.quota,
                active: true,
            },
        })
    }

    async fn suspend(&self, call: &Call<()>, service: &Service, reason: Value) -> Result<()> {
        execute(
            call,
            "UPDATE depot_box SET active = false, note = $2 WHERE service_id = $1",
            vec![service.id.to_string().into(), reason.to_string().into()],
        )
        .await
    }

    async fn resume(&self, call: &Call<()>, service: &Service) -> Result<()> {
        execute(
            call,
            "UPDATE depot_box SET active = true WHERE service_id = $1",
            vec![service.id.to_string().into()],
        )
        .await
    }

    async fn close(&self, call: &Call<()>, service: &Service) -> Result<()> {
        pass_gate(&service.external_id).await;
        execute(
            call,
            "UPDATE depot_box SET active = false, note = 'closed' WHERE service_id = $1",
            vec![service.id.to_string().into()],
        )
        .await
    }

    async fn set_params(&self, call: &Call<()>, service: &Service, params: Quota) -> Result<Usage> {
        execute(
            call,
            "UPDATE depot_box SET quota = $2 WHERE service_id = $1",
            vec![
                service.id.to_string().into(),
                i32::try_from(params.quota).unwrap_or_default().into(),
            ],
        )
        .await?;
        usage(call, service).await
    }

    async fn stat(&self, call: &Call<()>, service: &Service, period: Period) -> Result<Value> {
        let usage = usage(call, service).await?;
        Ok(json!({ "boxes": usage.quota, "hours": (period.to - period.from).num_hours() }))
    }

    async fn login(&self, _: &Call<()>, service: &Service) -> Result<LoginTarget> {
        Ok(LoginTarget {
            url: format!("/f/depot.box.list?service={}", service.id),
        })
    }
}

fn at(text: &str) -> DateTime<Utc> {
    text.parse().unwrap()
}

pub(crate) fn composed(builder: keel_testkit::Builder) -> keel_testkit::Builder {
    builder.module(&MODULE).module(&DEPOT)
}

async fn boot() -> Harness {
    composed(Harness::new()).clock(at(NOW)).boot().await
}

fn func(kind: &str, verb: &str) -> String {
    format!("svc.{kind}.{verb}")
}

fn account(external_id: &str) -> Value {
    json!({ "origin": ORIGIN, "externalId": external_id })
}

fn opening(external_id: &str, account_id: &str, quota: u32) -> Value {
    json!({ "externalId": external_id, "account": account(account_id), "params": { "quota": quota } })
}

async fn call(harness: &Harness, func: &str, params: Value, key: Option<&str>) -> Reply {
    harness
        .call_with(
            harness.principal(Who::Admin),
            func,
            params,
            CallOptions {
                idempotency_key: key.map(str::to_owned),
                ..CallOptions::default()
            },
        )
        .await
}

async fn open(harness: &Harness, kind: &str, params: Value, key: &str) -> Reply {
    call(harness, &func(kind, "open"), params, Some(key)).await
}

async fn act(harness: &Harness, kind: &str, verb: &str, params: Value) -> Reply {
    let key = format!("{kind}-{verb}-{}", Id::generate());
    call(harness, &func(kind, verb), params, Some(&key)).await
}

fn data(reply: &Reply) -> &Value {
    match reply.data() {
        Some(data) => data,
        None => panic!("a result was expected, got {:?}", reply.error()),
    }
}

fn refused(reply: &Reply, status: u16, key: &str) {
    assert_eq!(reply.status(), status, "{:?}", reply.envelope);
    assert_eq!(reply.error().map(|error| error.key.as_str()), Some(key));
}

async fn rows(harness: &Harness, sql: &str) -> Vec<String> {
    harness.database().rows(sql).await
}

async fn services(harness: &Harness) -> Vec<String> {
    rows(
        harness,
        "SELECT kind || ' ' || external_id || ' ' || status || ' ' || account_id FROM svc_service ORDER BY kind, external_id",
    )
    .await
}

async fn events(harness: &Harness) -> Vec<String> {
    rows(
        harness,
        "SELECT type || ' ' || (data_json->>'externalId') || ' ' || (data_json->>'status') FROM keel_outbox WHERE type LIKE 'svc.%' ORDER BY seq",
    )
    .await
}

async fn account_state(harness: &Harness, id: &str) -> String {
    rows(
        harness,
        &format!("SELECT state FROM keel_account WHERE id = '{id}'"),
    )
    .await
    .join(",")
}

fn text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| panic!("a string was expected, got {value}"))
}

#[tokio::test]
async fn open_twice_with_one_key_yields_one_service() {
    let harness = boot().await;

    let first = open(
        &harness,
        BOXES,
        opening("bill-svc-9912", "acc-7", 5),
        "bill-9912-open",
    )
    .await;
    let answer = data(&first).clone();
    assert!(!first.replay);
    assert_eq!(answer["status"], json!("active"));
    assert_eq!(answer["state"], json!({ "quota": 5, "active": true }));
    let again = open(
        &harness,
        BOXES,
        opening("bill-svc-9912", "acc-7", 5),
        "bill-9912-open",
    )
    .await;
    assert!(again.replay, "the repeat is replayed from the store");
    assert_eq!(data(&again), &answer);

    let service = text(&answer["serviceId"]);
    let account = text(&answer["accountId"]);
    assert_eq!(
        services(&harness).await,
        [format!("{BOXES} bill-svc-9912 active {account}")]
    );
    assert_eq!(
        rows(&harness, "SELECT service_id || ' ' || quota FROM depot_box").await,
        [format!("{service} 5")]
    );
    assert_eq!(
        rows(
            &harness,
            "SELECT id FROM keel_account WHERE origin = 'bill' AND external_id = 'acc-7'"
        )
        .await,
        std::slice::from_ref(&account)
    );
    assert_eq!(
        rows(
            &harness,
            &format!(
                "SELECT params_json::text || ' ' || state_json::text || ' ' || schema_version || ' ' || coalesce(peer_id, 'local') FROM svc_service WHERE id = '{service}'"
            )
        )
        .await,
        [r#"{"quota": 5} {"quota": 5, "active": true} 2 local"#]
    );
    assert_eq!(
        events(&harness).await,
        [format!("{EVENT_OPENED} bill-svc-9912 active")]
    );

    let mismatch = open(
        &harness,
        BOXES,
        opening("bill-svc-9912", "acc-7", 6),
        "bill-9912-open",
    )
    .await;
    refused(&mismatch, 409, "keel.error.idempotency_mismatch");
    let fresh_key = open(
        &harness,
        BOXES,
        opening("bill-svc-9912", "acc-7", 5),
        "bill-9912-again",
    )
    .await;
    refused(&fresh_key, 409, ERROR_EXISTS);
    assert_eq!(
        fresh_key.error().unwrap().params,
        *json!({ "kind": BOXES, "externalId": "bill-svc-9912" })
            .as_object()
            .unwrap()
    );
    assert_eq!(services(&harness).await.len(), 1);
    assert_eq!(
        rows(&harness, "SELECT count(*)::text FROM depot_box").await,
        ["1"]
    );

    harness.finish().await;
}

#[tokio::test]
async fn two_kinds_for_one_customer_land_in_one_account() {
    let harness = boot().await;

    let boxes = open(&harness, BOXES, opening("svc-1", "acc-7", 5), "k-boxes").await;
    let crates = open(&harness, CRATES, opening("svc-1", "acc-7", 9), "k-crates").await;
    let account = text(&data(&boxes)["accountId"]);
    assert_eq!(data(&crates)["accountId"], json!(account));
    assert_ne!(data(&crates)["serviceId"], data(&boxes)["serviceId"]);
    assert_eq!(
        services(&harness).await,
        [
            format!("{BOXES} svc-1 active {account}"),
            format!("{CRATES} svc-1 active {account}"),
        ]
    );
    assert_eq!(
        rows(
            &harness,
            "SELECT count(*)::text FROM keel_account WHERE origin = 'bill' AND external_id = 'acc-7'"
        )
        .await,
        ["1"]
    );
    let other = open(&harness, BOXES, opening("svc-2", "acc-8", 1), "k-other").await;
    assert_ne!(data(&other)["accountId"], json!(account));
    assert_eq!(
        rows(
            &harness,
            &format!("SELECT parent_id FROM keel_account WHERE id = '{account}'")
        )
        .await,
        [Id::ROOT_ACCOUNT.to_string()]
    );

    harness.finish().await;
}

#[tokio::test]
async fn close_suspends_and_deletes_nothing() {
    let harness = boot().await;
    let opened = open(&harness, BOXES, opening("svc-1", "acc-7", 5), "k-open").await;
    let service = text(&data(&opened)["serviceId"]);
    let account = text(&data(&opened)["accountId"]);
    let accounts = rows(&harness, "SELECT count(*)::text FROM keel_account").await;

    let closed = call(
        &harness,
        &func(BOXES, "close"),
        json!({ "externalId": "svc-1" }),
        Some("k-close"),
    )
    .await;
    let answer = data(&closed);
    assert_eq!(answer["status"], json!("closed"));
    assert_eq!(answer["serviceId"], json!(service));
    let retained: DateTime<Utc> = serde_json::from_value(answer["retainUntil"].clone()).unwrap();
    assert_eq!(
        retained,
        at(NOW) + Duration::days(i64::from(RETENTION_DAYS))
    );

    assert_eq!(account_state(&harness, &account).await, "suspended");
    assert_eq!(
        services(&harness).await,
        [format!("{BOXES} svc-1 closed {account}")]
    );
    assert_eq!(
        rows(
            &harness,
            &format!(
                "SELECT to_char(retain_until AT TIME ZONE 'UTC', 'YYYY-MM-DD') || ' ' || suspended_account FROM svc_service WHERE id = '{service}'"
            )
        )
        .await,
        ["2026-11-03 true"]
    );
    assert_eq!(
        rows(
            &harness,
            "SELECT service_id || ' ' || active || ' ' || note FROM depot_box"
        )
        .await,
        [format!("{service} false closed")]
    );
    assert_eq!(
        rows(&harness, "SELECT count(*)::text FROM keel_account").await,
        accounts
    );
    assert_eq!(
        rows(
            &harness,
            "SELECT type FROM keel_outbox WHERE type IN ('svc.service.closed', 'keel.account.suspended', 'keel.account.deleted') ORDER BY type"
        )
        .await,
        ["keel.account.suspended", EVENT_CLOSED]
    );
    let again = call(
        &harness,
        &func(BOXES, "close"),
        json!({ "externalId": "svc-1" }),
        Some("k-close"),
    )
    .await;
    assert!(again.replay);
    let twice = call(
        &harness,
        &func(BOXES, "close"),
        json!({ "externalId": "svc-1" }),
        Some("k-close-2"),
    )
    .await;
    refused(&twice, 409, ERROR_CLOSED);

    harness.finish().await;
}

async fn account_events(harness: &Harness) -> Vec<String> {
    rows(
        harness,
        "SELECT type FROM keel_outbox WHERE type IN ('keel.account.suspended', 'keel.account.resumed') ORDER BY seq",
    )
    .await
}

async fn marks(harness: &Harness) -> Vec<String> {
    rows(
        harness,
        "SELECT external_id || ' ' || suspended_account FROM svc_service ORDER BY external_id",
    )
    .await
}

#[tokio::test]
async fn closing_one_of_two_services_keeps_the_account_active_until_the_last_and_an_open_resumes_it()
 {
    let harness = boot().await;
    let boxes = open(&harness, BOXES, opening("svc-1", "acc-7", 5), "k-boxes").await;
    open(&harness, CRATES, opening("svc-2", "acc-7", 5), "k-crates").await;
    let account = text(&data(&boxes)["accountId"]);

    data(
        &act(
            &harness,
            CRATES,
            "suspend",
            json!({ "externalId": "svc-2" }),
        )
        .await,
    );
    data(&act(&harness, BOXES, "close", json!({ "externalId": "svc-1" })).await);
    assert_eq!(account_state(&harness, &account).await, "active");
    data(&act(&harness, CRATES, "close", json!({ "externalId": "svc-2" })).await);
    assert_eq!(account_state(&harness, &account).await, "suspended");
    assert_eq!(marks(&harness).await, ["svc-1 false", "svc-2 true"]);

    let reopened = open(&harness, BOXES, opening("svc-3", "acc-7", 5), "k-again").await;
    assert_eq!(data(&reopened)["accountId"], json!(account));
    assert_eq!(data(&reopened)["status"], json!("active"));
    assert_eq!(account_state(&harness, &account).await, "active");
    assert_eq!(
        account_events(&harness).await,
        ["keel.account.suspended", "keel.account.resumed"]
    );
    assert_eq!(
        marks(&harness).await,
        ["svc-1 false", "svc-2 false", "svc-3 false"]
    );

    harness.finish().await;
}

#[tokio::test]
async fn an_open_keeps_a_suspension_that_no_close_made() {
    let harness = boot().await;
    let suspend =
        |id: String| harness.call(Who::Admin, "keel.account.suspend", json!({ "id": id }));
    let first = open(&harness, BOXES, opening("svc-1", "acc-7", 5), "k-first").await;
    let account = text(&data(&first)["accountId"]);
    data(&suspend(account.clone()).await);

    let second = open(&harness, CRATES, opening("svc-2", "acc-7", 5), "k-second").await;
    assert_eq!(data(&second)["status"], json!("active"));
    assert_eq!(account_state(&harness, &account).await, "suspended");

    let bare = harness
        .call(
            Who::Admin,
            "keel.account.upsert",
            json!({ "origin": ORIGIN, "externalId": "acc-8" }),
        )
        .await;
    let empty = text(&data(&bare)["id"]);
    data(&suspend(empty.clone()).await);
    let first_of_its_own = open(&harness, BOXES, opening("svc-3", "acc-8", 5), "k-third").await;
    assert_eq!(data(&first_of_its_own)["accountId"], json!(empty));
    assert_eq!(account_state(&harness, &empty).await, "suspended");
    assert_eq!(
        account_events(&harness).await,
        ["keel.account.suspended", "keel.account.suspended"]
    );

    harness.finish().await;
}

#[tokio::test]
async fn a_suspension_an_administrator_made_outlives_the_close_of_the_last_service_and_the_next_open()
 {
    let harness = boot().await;
    let first = open(&harness, BOXES, opening("svc-1", "acc-7", 5), "k-first").await;
    let account = text(&data(&first)["accountId"]);
    data(
        &harness
            .call(Who::Admin, "keel.account.suspend", json!({ "id": account }))
            .await,
    );

    let closed = act(&harness, BOXES, "close", json!({ "externalId": "svc-1" })).await;
    assert_eq!(data(&closed)["status"], json!("closed"));
    assert_eq!(account_state(&harness, &account).await, "suspended");
    assert_eq!(marks(&harness).await, ["svc-1 false"]);
    let second = open(&harness, CRATES, opening("svc-2", "acc-7", 5), "k-second").await;
    assert_eq!(data(&second)["status"], json!("active"));
    assert_eq!(data(&second)["accountId"], json!(account));
    assert_eq!(account_state(&harness, &account).await, "suspended");
    assert_eq!(account_events(&harness).await, ["keel.account.suspended"]);

    harness.finish().await;
}

#[tokio::test]
async fn a_suspend_or_resume_of_the_account_after_the_last_close_takes_the_suspension_from_svc() {
    let harness = boot().await;
    let switch = |func: &'static str, id: &str| {
        harness.call(Who::Admin, func, json!({ "id": id.to_owned() }))
    };

    let first = open(&harness, BOXES, opening("svc-1", "acc-7", 5), "k-1").await;
    let again = text(&data(&first)["accountId"]);
    data(&act(&harness, BOXES, "close", json!({ "externalId": "svc-1" })).await);
    assert_eq!(marks(&harness).await, ["svc-1 true"]);
    let repeated = switch("keel.account.suspend", &again).await;
    assert_eq!(data(&repeated)["changed"], json!(false));
    assert_eq!(marks(&harness).await, ["svc-1 false"]);
    open(&harness, CRATES, opening("svc-2", "acc-7", 5), "k-2").await;
    assert_eq!(account_state(&harness, &again).await, "suspended");

    let third = open(&harness, BOXES, opening("svc-3", "acc-8", 5), "k-3").await;
    let cycled = text(&data(&third)["accountId"]);
    data(&act(&harness, BOXES, "close", json!({ "externalId": "svc-3" })).await);
    assert_eq!(account_state(&harness, &cycled).await, "suspended");
    data(&switch("keel.account.resume", &cycled).await);
    assert_eq!(
        marks(&harness).await,
        ["svc-1 false", "svc-2 false", "svc-3 false"]
    );
    data(&switch("keel.account.suspend", &cycled).await);
    let fourth = open(&harness, CRATES, opening("svc-4", "acc-8", 5), "k-4").await;
    assert_eq!(data(&fourth)["status"], json!("active"));
    assert_eq!(account_state(&harness, &cycled).await, "suspended");
    assert_eq!(
        account_events(&harness).await,
        [
            "keel.account.suspended",
            "keel.account.suspended",
            "keel.account.resumed",
            "keel.account.suspended",
        ]
    );

    harness.finish().await;
}

#[tokio::test]
async fn closes_racing_for_the_last_two_services_of_an_account_suspend_it_once() {
    let harness = boot().await;
    let boxes = open(&harness, BOXES, opening("svc-1", "acc-7", 5), "k-boxes").await;
    open(&harness, CRATES, opening("svc-2", "acc-7", 5), "k-crates").await;
    let account = text(&data(&boxes)["accountId"]);

    let (first, second) = tokio::join!(
        act(&harness, BOXES, "close", json!({ "externalId": "svc-1" })),
        act(&harness, CRATES, "close", json!({ "externalId": "svc-2" })),
    );
    data(&first);
    data(&second);
    assert_eq!(account_state(&harness, &account).await, "suspended");
    assert_eq!(
        rows(
            &harness,
            "SELECT count(*)::text FROM keel_outbox WHERE type = 'keel.account.suspended'"
        )
        .await,
        ["1"]
    );

    harness.finish().await;
}

async fn hold(harness: &Harness, customer: &str) -> keel_sdk::sea_orm::DatabaseTransaction {
    let holder = harness.db().begin().await.unwrap();
    holder
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
            [lock_key(ORIGIN, customer).into()],
        ))
        .await
        .unwrap();
    holder
}

async fn after_the_holder<F: std::future::Future<Output = Reply>>(
    holder: keel_sdk::sea_orm::DatabaseTransaction,
    call: F,
) -> Reply {
    tokio::pin!(call);
    let waited = tokio::time::timeout(std::time::Duration::from_millis(300), &mut call).await;
    assert!(
        waited.is_err(),
        "the call went ahead of the holder of its account"
    );
    holder.commit().await.unwrap();
    call.await
}

#[tokio::test]
async fn open_and_close_wait_for_whoever_holds_their_account() {
    let harness = boot().await;
    let opened = open(&harness, BOXES, opening("svc-1", "acc-7", 5), "k-open").await;
    let account = text(&data(&opened)["accountId"]);

    let holder = hold(&harness, "acc-7").await;
    let second = after_the_holder(
        holder,
        open(&harness, CRATES, opening("svc-2", "acc-7", 5), "k-second"),
    )
    .await;
    assert_eq!(data(&second)["accountId"], json!(account));
    data(&act(&harness, CRATES, "close", json!({ "externalId": "svc-2" })).await);

    let holder = hold(&harness, "acc-7").await;
    let closed = after_the_holder(
        holder,
        act(&harness, BOXES, "close", json!({ "externalId": "svc-1" })),
    )
    .await;
    assert_eq!(data(&closed)["status"], json!("closed"));
    assert_eq!(account_state(&harness, &account).await, "suspended");

    harness.finish().await;
}

#[tokio::test]
async fn a_suspend_or_resume_of_an_account_with_services_waits_for_whoever_holds_it() {
    let harness = boot().await;
    let opened = open(&harness, BOXES, opening("svc-1", "acc-7", 5), "k-open").await;
    let account = text(&data(&opened)["accountId"]);

    for func in ["keel.account.suspend", "keel.account.resume"] {
        let holder = hold(&harness, "acc-7").await;
        let switched = after_the_holder(
            holder,
            harness.call(Who::Admin, func, json!({ "id": account })),
        )
        .await;
        assert_eq!(data(&switched)["changed"], json!(true), "{func}");
    }
    assert_eq!(account_state(&harness, &account).await, "active");

    harness.finish().await;
}

#[tokio::test]
async fn an_open_racing_the_close_of_the_last_service_of_its_account_waits_and_resumes_it() {
    let harness = boot().await;
    let opened = open(&harness, BOXES, opening("svc-gated", "acc-7", 5), "k-open").await;
    let account = text(&data(&opened)["accountId"]);
    let (entered, release) = gate("svc-gated");

    let closing = act(
        &harness,
        BOXES,
        "close",
        json!({ "externalId": "svc-gated" }),
    );
    let reopening = async {
        entered.await.expect("the close reaches its kind");
        let second = open(&harness, CRATES, opening("svc-2", "acc-7", 5), "k-second");
        tokio::pin!(second);
        let waited = tokio::time::timeout(std::time::Duration::from_millis(300), &mut second).await;
        assert!(
            waited.is_err(),
            "the open went ahead of the close holding its account"
        );
        release.send(()).expect("the close waits in its kind");
        second.await
    };
    let (closed, reopened) = tokio::join!(closing, reopening);

    assert_eq!(data(&closed)["status"], json!("closed"));
    assert_eq!(data(&reopened)["status"], json!("active"));
    assert_eq!(data(&reopened)["accountId"], json!(account));
    assert_eq!(account_state(&harness, &account).await, "active");
    assert_eq!(
        account_events(&harness).await,
        ["keel.account.suspended", "keel.account.resumed"]
    );
    assert_eq!(
        services(&harness).await,
        [
            format!("{BOXES} svc-gated closed {account}"),
            format!("{CRATES} svc-2 active {account}"),
        ]
    );

    harness.finish().await;
}

#[tokio::test]
async fn open_and_close_without_a_key_answer_validation() {
    let harness = boot().await;

    let open = call(
        &harness,
        &func(BOXES, "open"),
        opening("svc-1", "acc-7", 5),
        None,
    )
    .await;
    refused(&open, 422, REQUIRED);
    assert!(services(&harness).await.is_empty());
    assert_eq!(
        rows(
            &harness,
            "SELECT count(*)::text FROM keel_account WHERE origin = 'bill'"
        )
        .await,
        ["0"]
    );
    data(&act(&harness, BOXES, "open", opening("svc-1", "acc-7", 5)).await);
    let close = call(
        &harness,
        &func(BOXES, "close"),
        json!({ "externalId": "svc-1" }),
        None,
    )
    .await;
    refused(&close, 422, REQUIRED);
    let suspend = call(
        &harness,
        &func(BOXES, "suspend"),
        json!({ "externalId": "svc-1" }),
        None,
    )
    .await;
    data(&suspend);

    harness.finish().await;
}

#[tokio::test]
async fn suspend_and_resume_hand_the_service_to_its_kind_and_a_repeat_changes_nothing() {
    let harness = boot().await;
    let opened = open(&harness, BOXES, opening("svc-1", "acc-7", 5), "k-open").await;
    let account = text(&data(&opened)["accountId"]);

    let suspended = act(
        &harness,
        BOXES,
        "suspend",
        json!({ "externalId": "svc-1", "reason": { "code": "unpaid" } }),
    )
    .await;
    assert_eq!(data(&suspended)["status"], json!("suspended"));
    assert_eq!(
        data(&suspended)["state"],
        json!({ "quota": 5, "active": true })
    );
    assert_eq!(
        rows(&harness, "SELECT active || ' ' || note FROM depot_box").await,
        [r#"false {"code":"unpaid"}"#]
    );
    let repeated = act(&harness, BOXES, "suspend", json!({ "externalId": "svc-1" })).await;
    assert_eq!(data(&repeated)["status"], json!("suspended"));
    assert_eq!(
        rows(&harness, "SELECT note FROM depot_box").await,
        [r#"{"code":"unpaid"}"#]
    );
    assert_eq!(account_state(&harness, &account).await, "active");

    let resumed = act(&harness, BOXES, "resume", json!({ "externalId": "svc-1" })).await;
    assert_eq!(data(&resumed)["status"], json!("active"));
    assert_eq!(
        rows(&harness, "SELECT active::text FROM depot_box").await,
        ["true"]
    );
    data(&act(&harness, BOXES, "resume", json!({ "externalId": "svc-1" })).await);
    assert_eq!(
        events(&harness).await,
        [
            format!("{EVENT_OPENED} svc-1 active"),
            format!("{EVENT_SUSPENDED} svc-1 suspended"),
            format!("{EVENT_RESUMED} svc-1 active"),
        ]
    );

    harness.finish().await;
}

#[tokio::test]
async fn set_params_applies_the_params_and_answers_the_state_of_the_kind() {
    let harness = boot().await;
    let opened = open(&harness, BOXES, opening("svc-1", "acc-7", 5), "k-open").await;
    let service = text(&data(&opened)["serviceId"]);
    harness
        .db()
        .execute_unprepared(&format!(
            "UPDATE svc_service SET schema_version = 1 WHERE id = '{service}'"
        ))
        .await
        .unwrap();

    let changed = act(
        &harness,
        BOXES,
        "set_params",
        json!({ "externalId": "svc-1", "params": { "quota": 40 } }),
    )
    .await;
    assert_eq!(
        data(&changed)["state"],
        json!({ "quota": 40, "active": true })
    );
    assert_eq!(data(&changed)["status"], json!("active"));
    assert_eq!(
        rows(
            &harness,
            "SELECT params_json::text || ' ' || state_json::text || ' ' || schema_version FROM svc_service"
        )
        .await,
        [r#"{"quota": 40} {"quota": 40, "active": true} 2"#]
    );
    let out_of_range = act(
        &harness,
        BOXES,
        "set_params",
        json!({ "externalId": "svc-1", "params": { "quota": 101 } }),
    )
    .await;
    assert_eq!(out_of_range.status(), 422, "{:?}", out_of_range.envelope);
    assert_eq!(out_of_range.error().unwrap().fields[0].path, "params.quota");
    assert_eq!(
        events(&harness).await.last().unwrap(),
        &format!("{EVENT_UPDATED} svc-1 active")
    );

    harness.finish().await;
}

#[tokio::test]
async fn stat_answers_the_usage_of_the_kind_for_a_period_that_ends_after_it_starts() {
    let harness = boot().await;
    let opened = open(&harness, BOXES, opening("svc-1", "acc-7", 5), "k-open").await;
    let service = text(&data(&opened)["serviceId"]);

    let stat = call(
        &harness,
        &func(BOXES, "stat"),
        json!({ "externalId": "svc-1", "from": "2026-10-01T00:00:00Z", "to": "2026-10-03T00:00:00Z" }),
        None,
    )
    .await;
    assert_eq!(
        data(&stat),
        &json!({
            "serviceId": service,
            "from": "2026-10-01T00:00:00Z",
            "to": "2026-10-03T00:00:00Z",
            "usage": { "boxes": 5, "hours": 48 }
        })
    );
    let backwards = call(
        &harness,
        &func(BOXES, "stat"),
        json!({ "externalId": "svc-1", "from": "2026-10-03T00:00:00Z", "to": "2026-10-03T00:00:00Z" }),
        None,
    )
    .await;
    assert_eq!(backwards.status(), 422);
    let field = &backwards.error().unwrap().fields[0];
    assert_eq!(field.path, "to");
    assert_eq!(field.key.as_deref(), Some(ERROR_PERIOD));

    harness.finish().await;
}

#[tokio::test]
async fn a_closed_service_refuses_every_change_and_still_answers_its_usage() {
    let harness = boot().await;
    open(&harness, BOXES, opening("svc-1", "acc-7", 5), "k-open").await;
    data(&act(&harness, BOXES, "close", json!({ "externalId": "svc-1" })).await);

    for (verb, params) in [
        ("suspend", json!({ "externalId": "svc-1" })),
        ("resume", json!({ "externalId": "svc-1" })),
        ("close", json!({ "externalId": "svc-1" })),
        (
            "set_params",
            json!({ "externalId": "svc-1", "params": { "quota": 2 } }),
        ),
        ("login", json!({ "externalId": "svc-1" })),
    ] {
        let reply = act(&harness, BOXES, verb, params).await;
        refused(&reply, 409, ERROR_CLOSED);
        assert_eq!(reply.error().unwrap().params["kind"], json!(BOXES));
    }
    let stat = call(
        &harness,
        &func(BOXES, "stat"),
        json!({ "externalId": "svc-1", "from": "2026-10-01T00:00:00Z", "to": "2026-10-02T00:00:00Z" }),
        None,
    )
    .await;
    assert_eq!(data(&stat)["usage"]["boxes"], json!(5));

    harness.finish().await;
}

#[tokio::test]
async fn login_answers_the_target_of_the_kind() {
    let harness = boot().await;
    let opened = open(&harness, BOXES, opening("svc-1", "acc-7", 5), "k-open").await;
    let service = text(&data(&opened)["serviceId"]);

    let login = act(&harness, BOXES, "login", json!({ "externalId": "svc-1" })).await;
    assert_eq!(
        data(&login),
        &json!({ "url": format!("/f/depot.box.list?service={service}") })
    );

    harness.finish().await;
}

#[tokio::test]
async fn an_unknown_service_answers_not_found_and_a_service_of_one_kind_is_unknown_to_another() {
    let harness = boot().await;
    open(&harness, BOXES, opening("svc-1", "acc-7", 5), "k-open").await;

    for (kind, verb) in [
        (BOXES, "suspend"),
        (BOXES, "resume"),
        (BOXES, "close"),
        (BOXES, "login"),
        (CRATES, "suspend"),
    ] {
        let id = if kind == BOXES { "svc-404" } else { "svc-1" };
        let reply = act(&harness, kind, verb, json!({ "externalId": id })).await;
        refused(&reply, 404, ERROR_NOT_FOUND);
        assert_eq!(reply.error().unwrap().params["externalId"], json!(id));
    }
    let stat = call(
        &harness,
        &func(BOXES, "stat"),
        json!({ "externalId": "svc-404", "from": "2026-10-01T00:00:00Z", "to": "2026-10-02T00:00:00Z" }),
        None,
    )
    .await;
    refused(&stat, 404, ERROR_NOT_FOUND);

    harness.finish().await;
}

#[tokio::test]
async fn the_account_of_an_open_is_checked_and_its_errors_land_under_account() {
    let harness = boot().await;

    let quoted = act(
        &harness,
        BOXES,
        "open",
        json!({ "externalId": "svc-1", "account": { "origin": "bill\"", "externalId": "acc-7" }, "params": { "quota": 1 } }),
    )
    .await;
    assert_eq!(quoted.status(), 422);
    assert_eq!(quoted.error().unwrap().fields[0].path, "account.origin");
    let reserved = act(
        &harness,
        BOXES,
        "open",
        json!({ "externalId": "svc-1", "account": { "origin": ORIGIN, "externalId": "acc-7", "kind": "system" }, "params": { "quota": 1 } }),
    )
    .await;
    assert_eq!(reserved.status(), 422, "{:?}", reserved.envelope);
    let field = &reserved.error().unwrap().fields[0];
    assert_eq!(field.path, "account.kind");
    assert_eq!(
        field.key.as_deref(),
        Some("keel.account.error.kind_reserved")
    );
    let spaced = act(
        &harness,
        BOXES,
        "open",
        json!({ "externalId": "svc 1", "account": account("acc-7"), "params": { "quota": 1 } }),
    )
    .await;
    assert_eq!(spaced.status(), 422, "{:?}", spaced.envelope);
    assert_eq!(spaced.error().unwrap().fields[0].path, "externalId");
    let missing = act(
        &harness,
        BOXES,
        "open",
        json!({ "externalId": "svc-2", "account": { "origin": ORIGIN }, "params": { "quota": 1 } }),
    )
    .await;
    assert_eq!(missing.status(), 422);
    assert_eq!(
        missing.error().unwrap().fields[0].path,
        "account.externalId"
    );
    assert!(services(&harness).await.is_empty());
    assert_eq!(
        rows(
            &harness,
            "SELECT count(*)::text FROM keel_account WHERE origin = 'bill'"
        )
        .await,
        ["0"]
    );

    harness.finish().await;
}

#[tokio::test]
async fn only_an_administrator_reaches_the_service_functions() {
    let harness = boot().await;

    for who in [Who::Reseller, Who::A1] {
        let reply = harness
            .call_with(
                harness.principal(who),
                &func(BOXES, "open"),
                opening("svc-1", "acc-7", 5),
                CallOptions {
                    idempotency_key: Some(format!("k-{who:?}")),
                    ..CallOptions::default()
                },
            )
            .await;
        assert_eq!(reply.status(), 403, "{:?}", reply.envelope);
        let catalogue = harness.call(who, KINDS, json!({})).await;
        assert_eq!(catalogue.status(), 403, "{:?}", catalogue.envelope);
    }
    assert!(services(&harness).await.is_empty());

    harness.finish().await;
}

#[tokio::test]
async fn the_catalogue_names_every_kind_with_its_schemas_and_functions() {
    let harness = boot().await;

    let reply = harness.call(Who::Admin, KINDS, json!({})).await;
    let catalogue = data(&reply);
    let kinds: Vec<&str> = catalogue["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, [BOXES, CRATES]);
    let boxes = &catalogue["data"][0];
    assert_eq!(boxes["schemaVersion"], json!(2));
    assert_eq!(boxes["title"], json!("depot.service.boxes"));
    assert_eq!(
        boxes["account"]["required"],
        json!(["origin", "externalId"])
    );
    assert_eq!(
        boxes["params"]["properties"]["quota"]["maximum"],
        json!(100)
    );
    assert_eq!(boxes["params"].get("$schema"), None);
    assert_eq!(boxes["state"]["required"], json!(["quota", "active"]));
    assert_eq!(
        boxes["functions"],
        json!({
            "open": "svc.depot.boxes.open",
            "suspend": "svc.depot.boxes.suspend",
            "resume": "svc.depot.boxes.resume",
            "close": "svc.depot.boxes.close",
            "setParams": "svc.depot.boxes.set_params",
            "stat": "svc.depot.boxes.stat",
            "login": "svc.depot.boxes.login"
        })
    );
    for function in boxes["functions"].as_object().unwrap().values() {
        assert!(
            harness
                .registry()
                .function(function.as_str().unwrap())
                .is_some(),
            "{function}"
        );
    }
    let open = harness.registry().function("svc.depot.boxes.open").unwrap();
    let descriptor = serde_json::to_value(open.descriptor()).unwrap();
    assert_eq!(open.module(), "svc");
    assert_eq!(descriptor["idempotency"], json!("required"));
    assert_eq!(
        descriptor["params"]["properties"]["params"],
        json!({ "$ref": "#/$defs/Quota" })
    );
    assert_eq!(
        descriptor["params"]["$defs"]["Quota"]["properties"]["quota"]["minimum"],
        json!(1)
    );
    assert_eq!(
        descriptor["result"]["properties"]["state"],
        json!({ "$ref": "#/$defs/Usage" })
    );

    harness.finish().await;
}

#[tokio::test]
async fn the_handshake_names_the_svc_feature_and_the_service_kinds() {
    let harness = boot().await;
    let http = harness.http().await;

    let answer = http.get(Who::Guest, "/api/v1/instance").await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    let info: InstanceInfo = serde_json::from_str(&answer.body).unwrap();
    assert!(
        info.features.iter().any(|feature| feature == FEATURE),
        "{info:?}"
    );
    assert_eq!(info.service_kinds, [BOXES, CRATES]);

    drop(http);
    harness.finish().await;

    let bare = Harness::new().module(&MODULE).boot().await;
    let http = bare.http().await;
    let answer = http.get(Who::Guest, "/api/v1/instance").await;
    let info: InstanceInfo = serde_json::from_str(&answer.body).unwrap();
    assert!(info.features.iter().any(|feature| feature == FEATURE));
    assert!(info.service_kinds.is_empty());
    let reply = bare.call(Who::Admin, KINDS, json!({})).await;
    assert_eq!(data(&reply), &json!({ "data": [] }));
    drop(http);
    bare.finish().await;
}

#[cfg(feature = "settings")]
#[tokio::test]
async fn close_retains_the_service_for_the_days_of_the_setting() {
    let harness = composed(Harness::new().module(&keel_system::settings::MODULE))
        .clock(at(NOW))
        .boot()
        .await;
    let set = |value: Value| {
        harness.call(
            Who::Admin,
            "settings.set",
            json!({ "key": keel_system::svc::RETENTION_DAYS_KEY, "value": value }),
        )
    };
    assert_eq!(set(json!(0)).await.status(), 422);
    data(&set(json!(7)).await);
    open(&harness, BOXES, opening("svc-1", "acc-7", 5), "k-open").await;

    let closed = act(&harness, BOXES, "close", json!({ "externalId": "svc-1" })).await;
    let retained: DateTime<Utc> =
        serde_json::from_value(data(&closed)["retainUntil"].clone()).unwrap();
    assert_eq!(retained, at(NOW) + Duration::days(7));

    harness.finish().await;
}

#[cfg(feature = "tasks")]
#[tokio::test]
async fn deleting_the_account_forgets_its_services() {
    use keel_sdk::types::TaskState;
    use keel_system::tasks::api::Drain;

    let harness = composed(Harness::new().module(&keel_system::tasks::MODULE))
        .clock(at(NOW))
        .boot()
        .await;
    let opened = open(&harness, BOXES, opening("svc-1", "acc-7", 5), "k-open").await;
    open(&harness, CRATES, opening("svc-2", "acc-7", 5), "k-crates").await;
    open(&harness, BOXES, opening("svc-3", "acc-8", 5), "k-other").await;
    let account = text(&data(&opened)["accountId"]);

    let deletion = harness
        .call(Who::Admin, "keel.account.delete", json!({ "id": account }))
        .await;
    let task: Id = data(&deletion)["task"]["id"]
        .as_str()
        .and_then(|id| id.parse().ok())
        .unwrap_or_else(|| panic!("a task was expected, got {:?}", deletion.envelope));
    let drain = harness
        .registry()
        .services()
        .get::<Drain>()
        .expect("the tasks module provides the drain");
    let state = tokio::time::timeout(std::time::Duration::from_secs(30), drain.wait(task))
        .await
        .expect("the deletion ends in time")
        .expect("the state is read");
    assert_eq!(state, TaskState::Done);
    assert_eq!(
        rows(
            &harness,
            "SELECT external_id FROM svc_service ORDER BY external_id"
        )
        .await,
        ["svc-3"]
    );
    assert_eq!(
        rows(
            &harness,
            &format!("SELECT count(*)::text FROM keel_account WHERE id = '{account}'")
        )
        .await,
        ["0"]
    );

    harness.finish().await;
}

#[cfg(feature = "peers")]
mod through_a_peer {
    use keel_sdk::system::SecretCipher;
    use keel_system::peers::FEED_PATH;
    use keel_testkit::{Answer, EVENTS_PATH, EventStream};

    use super::*;
    use crate::peers::{Two, link_principal, outbound};

    const SUBSCRIBED: &str = "svc.*";
    const PATIENCE: std::time::Duration = std::time::Duration::from_secs(40);

    async fn installation() -> Harness {
        super::composed(crate::peers::composed(Harness::new()))
            .boot()
            .await
    }

    async fn as_peer(
        http: &keel_testkit::TestHttp,
        token: &str,
        func: &str,
        params: Value,
        key: Option<&str>,
    ) -> Answer {
        let bearer = format!("Bearer {token}");
        let mut headers = vec![
            ("authorization", bearer.as_str()),
            ("content-type", "application/json"),
        ];
        if let Some(key) = key {
            headers.push(("idempotency-key", key));
        }
        http.request(
            "POST",
            &format!("/api/v1/call/{func}"),
            &headers,
            Some(&params.to_string()),
        )
        .await
    }

    async fn open_as_peer(
        http: &keel_testkit::TestHttp,
        token: &str,
        params: Value,
        key: &str,
    ) -> Answer {
        as_peer(http, token, &func(BOXES, "open"), params, Some(key)).await
    }

    async fn paired(
        host: &Harness,
        host_http: &keel_testkit::TestHttp,
        billing: &Harness,
    ) -> String {
        let code = crate::peers::code(host, json!({ "scopes": ["svc.*"], "level": "admin" })).await;
        let created = billing
            .call(
                Who::Admin,
                keel_system::peers::CREATE,
                json!({ "url": host_http.url(""), "code": code, "subscribe": [SUBSCRIBED] }),
            )
            .await;
        let link = text(&data(&created)["id"]);
        outbound(billing, &link).await
    }

    #[tokio::test]
    async fn a_peer_reaches_no_service_of_another_peers_customer() {
        let host = installation().await;
        let host_http = host.http().await;
        let alpha = installation().await;
        let alpha_http = alpha.http().await;
        let beta = installation().await;
        let beta_http = beta.http().await;
        let alpha_token = paired(&host, &host_http, &alpha).await;
        let beta_token = paired(&host, &host_http, &beta).await;
        let origin = format!("bill:{}", alpha.instance());

        let opened = open_as_peer(
            &host_http,
            &alpha_token,
            json!({
                "externalId": "bill-svc-1",
                "account": { "origin": origin, "externalId": "acc-7" },
                "params": { "quota": 3 }
            }),
            "alpha-open",
        )
        .await;
        assert_eq!(opened.status, 200, "{}", opened.body);
        let account = text(&opened.json()["data"]["accountId"]);

        for (verb, params, key) in [
            ("suspend", json!({ "externalId": "bill-svc-1" }), None),
            ("resume", json!({ "externalId": "bill-svc-1" }), None),
            (
                "close",
                json!({ "externalId": "bill-svc-1" }),
                Some("beta-close"),
            ),
            (
                "set_params",
                json!({ "externalId": "bill-svc-1", "params": { "quota": 9 } }),
                None,
            ),
            (
                "stat",
                json!({ "externalId": "bill-svc-1", "from": "2026-10-01T00:00:00Z", "to": "2026-10-02T00:00:00Z" }),
                None,
            ),
            ("login", json!({ "externalId": "bill-svc-1" }), None),
        ] {
            let reply = as_peer(&host_http, &beta_token, &func(BOXES, verb), params, key).await;
            crate::peers::refused(&reply, 404, ERROR_NOT_FOUND);
        }
        assert_eq!(
            services(&host).await,
            [format!("{BOXES} bill-svc-1 active {account}")]
        );
        assert_eq!(account_state(&host, &account).await, "active");
        assert_eq!(
            rows(&host, "SELECT quota || ' ' || active FROM depot_box").await,
            ["3 true"]
        );

        let login = as_peer(
            &host_http,
            &alpha_token,
            &func(BOXES, "login"),
            json!({ "externalId": "bill-svc-1" }),
            None,
        )
        .await;
        assert_eq!(login.status, 200, "{}", login.body);
        let closed = as_peer(
            &host_http,
            &alpha_token,
            &func(BOXES, "close"),
            json!({ "externalId": "bill-svc-1" }),
            Some("alpha-close"),
        )
        .await;
        assert_eq!(closed.status, 200, "{}", closed.body);
        assert_eq!(closed.json()["data"]["status"], json!("closed"));
        assert_eq!(account_state(&host, &account).await, "suspended");

        for http in [host_http, alpha_http, beta_http] {
            http.stop().await.unwrap();
        }
        host.finish().await;
        alpha.finish().await;
        beta.finish().await;
    }

    #[tokio::test]
    async fn a_peer_opens_a_service_under_its_own_origin_and_the_service_remembers_the_link() {
        let two = Two::of(installation().await, installation().await).await;
        let linked = two
            .pair(json!({ "scopes": ["svc.*"], "level": "admin" }), json!({}))
            .await;
        let token = outbound(&two.a, &linked.a_link).await;
        let origin = format!("bill:{}", two.a.instance());
        let params = json!({
            "externalId": "bill-svc-1",
            "account": { "origin": origin, "externalId": "acc-7" },
            "params": { "quota": 3 }
        });

        let opened = open_as_peer(&two.b_http, &token, params.clone(), "bill-1-open").await;
        assert_eq!(opened.status, 200, "{}", opened.body);
        let again = open_as_peer(&two.b_http, &token, params, "bill-1-open").await;
        assert_eq!(again.status, 200, "{}", again.body);
        assert_eq!(again.header("x-keel-idempotent-replay"), Some("true"));
        assert_eq!(again.json()["data"], opened.json()["data"]);
        assert_eq!(
            rows(
                &two.b,
                "SELECT external_id || ' ' || peer_id FROM svc_service"
            )
            .await,
            [format!("bill-svc-1 {}", linked.b_link)]
        );
        assert_eq!(
            rows(
                &two.b,
                &format!(
                    "SELECT count(*)::text FROM keel_account WHERE origin = '{origin}' AND external_id = 'acc-7'"
                )
            )
            .await,
            ["1"]
        );

        let foreign = open_as_peer(
            &two.b_http,
            &token,
            json!({
                "externalId": "bill-svc-2",
                "account": { "origin": "bill", "externalId": "acc-8" },
                "params": { "quota": 3 }
            }),
            "bill-2-open",
        )
        .await;
        assert_eq!(foreign.status, 403, "{}", foreign.body);
        assert_eq!(
            foreign.json()["error"]["key"],
            json!(keel_system::peers::ERROR_ORIGIN)
        );
        assert_eq!(
            rows(&two.b, "SELECT count(*)::text FROM svc_service").await,
            ["1"]
        );

        two.finish().await;
    }

    async fn link_on(host: &Harness, billing: &Harness) -> String {
        rows(
            host,
            &format!(
                "SELECT id FROM peer_product WHERE instance_id = '{}'",
                billing.instance()
            ),
        )
        .await
        .join(",")
    }

    async fn subscribe(host: &Harness, name: &str, link: &str) {
        let sealed = host
            .registry()
            .services()
            .get::<dyn SecretCipher>()
            .expect("the kernel seals with the data key")
            .seal(b"whs_0123456789abcdef0123456789abcdef")
            .unwrap();
        host.db()
            .execute_unprepared(&format!(
                "INSERT INTO webhook_subscription (id, account_id, principal_id, name, url, secret, events, enabled, created_at, created_by, updated_at, peer_id, level) \
                 VALUES ('{}', '{}', '{}', '{name}', 'http://127.0.0.1:9/api/v1/x/peer/hook', '{sealed}', ARRAY['{SUBSCRIBED}'], true, now(), '{name}', now(), '{link}', 'admin')",
                Id::generate(),
                Id::ROOT_ACCOUNT,
                link_principal(host, link).await,
            ))
            .await
            .unwrap();
    }

    async fn stream(http: &keel_testkit::TestHttp, token: &str) -> EventStream {
        let bearer = format!("Bearer {token}");
        let mut stream = http
            .stream(EVENTS_PATH, &[("authorization", bearer.as_str())])
            .await;
        assert_eq!(stream.status(), 200);
        assert!(stream.next().await.is_comment());
        stream
    }

    async fn streamed_until(stream: &mut EventStream, last: u64) -> Vec<String> {
        let deadline = tokio::time::Instant::now() + PATIENCE;
        let mut kinds = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let frame = match stream.next_within(remaining).await {
                Ok(Some(frame)) => frame,
                other => panic!("the stream ended before {last} with {kinds:?}: {other:?}"),
            };
            if frame.event.as_deref() != Some("data.invalidate") {
                continue;
            }
            kinds.push(text(&frame.json()["type"]));
            let seq = frame.id.as_deref().and_then(|id| id.parse::<u64>().ok());
            if seq.is_some_and(|seq| seq >= last) {
                return kinds;
            }
        }
    }

    async fn fed(http: &keel_testkit::TestHttp, token: &str) -> Vec<String> {
        let bearer = format!("Bearer {token}");
        let answer = http
            .request(
                "GET",
                &format!("{FEED_PATH}?types={SUBSCRIBED}"),
                &[("authorization", bearer.as_str())],
                None,
            )
            .await;
        assert_eq!(answer.status, 200, "{}", answer.body);
        answer.json()["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| {
                format!(
                    "{} {}",
                    text(&event["type"]),
                    text(&event["data"]["externalId"])
                )
            })
            .collect()
    }

    async fn newest(harness: &Harness) -> u64 {
        rows(harness, "SELECT max(seq)::text FROM keel_outbox").await[0]
            .parse()
            .unwrap()
    }

    async fn delivered_past(harness: &Harness, last: u64) {
        let deadline = tokio::time::Instant::now() + PATIENCE;
        loop {
            let cursor = rows(harness, "SELECT seq::text FROM webhook_cursor").await;
            if cursor
                .first()
                .and_then(|seq| seq.parse::<u64>().ok())
                .is_some_and(|seq| seq >= last)
            {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the webhook consumer did not pass {last}: {cursor:?}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn an_event_about_a_service_reaches_the_peer_whose_customer_it_serves_and_no_other() {
        let host = installation().await;
        let host_http = host.http().await;
        let alpha = installation().await;
        let alpha_http = alpha.http().await;
        let beta = installation().await;
        let beta_http = beta.http().await;
        let alpha_token = paired(&host, &host_http, &alpha).await;
        let beta_token = paired(&host, &host_http, &beta).await;
        subscribe(&host, "alpha", &link_on(&host, &alpha).await).await;
        subscribe(&host, "beta", &link_on(&host, &beta).await).await;
        let mut alpha_stream = stream(&host_http, &alpha_token).await;
        let mut beta_stream = stream(&host_http, &beta_token).await;

        let opened = open_as_peer(
            &host_http,
            &alpha_token,
            json!({
                "externalId": "bill-svc-1",
                "account": { "origin": format!("bill:{}", alpha.instance()), "externalId": "acc-7" },
                "params": { "quota": 3 }
            }),
            "alpha-open",
        )
        .await;
        assert_eq!(opened.status, 200, "{}", opened.body);
        let local = open(&host, BOXES, opening("svc-local", "acc-9", 1), "k-local").await;
        assert_eq!(data(&local)["status"], json!("active"));
        let sentinel = host
            .call(
                Who::Admin,
                "keel.account.upsert",
                json!({ "origin": ORIGIN, "externalId": "sentinel" }),
            )
            .await;
        data(&sentinel);
        let last = newest(&host).await;

        assert_eq!(
            fed(&host_http, &alpha_token).await,
            [format!("{EVENT_OPENED} bill-svc-1")]
        );
        assert!(fed(&host_http, &beta_token).await.is_empty());
        let to_alpha = streamed_until(&mut alpha_stream, last).await;
        let to_beta = streamed_until(&mut beta_stream, last).await;
        let opened_events =
            |kinds: &[String]| kinds.iter().filter(|kind| *kind == EVENT_OPENED).count();
        assert_eq!(opened_events(&to_alpha), 1, "{to_alpha:?}");
        assert_eq!(opened_events(&to_beta), 0, "{to_beta:?}");
        delivered_past(&host, last).await;
        assert_eq!(
            rows(
                &host,
                "SELECT s.name || ' ' || d.type FROM webhook_delivery d JOIN webhook_subscription s ON s.id = d.subscription_id ORDER BY s.name, d.seq"
            )
            .await,
            [format!("alpha {EVENT_OPENED}")]
        );

        drop((alpha_stream, beta_stream));
        for http in [host_http, alpha_http, beta_http] {
            http.stop().await.unwrap();
        }
        host.finish().await;
        alpha.finish().await;
        beta.finish().await;
    }

    #[tokio::test]
    async fn a_peer_cannot_lift_an_administrators_suspension_by_closing_the_last_service_and_opening_another()
     {
        let two = Two::of(installation().await, installation().await).await;
        let linked = two
            .pair(json!({ "scopes": ["svc.*"], "level": "admin" }), json!({}))
            .await;
        let token = outbound(&two.a, &linked.a_link).await;
        let origin = format!("bill:{}", two.a.instance());
        let opening = |external_id: &str| {
            json!({
                "externalId": external_id,
                "account": { "origin": origin, "externalId": "acc-7" },
                "params": { "quota": 3 }
            })
        };
        let opened = open_as_peer(&two.b_http, &token, opening("bill-svc-1"), "open-1").await;
        assert_eq!(opened.status, 200, "{}", opened.body);
        let account = text(&opened.json()["data"]["accountId"]);
        data(
            &two.b
                .call(Who::Admin, "keel.account.suspend", json!({ "id": account }))
                .await,
        );

        let closed = as_peer(
            &two.b_http,
            &token,
            &func(BOXES, "close"),
            json!({ "externalId": "bill-svc-1" }),
            Some("close-1"),
        )
        .await;
        assert_eq!(closed.status, 200, "{}", closed.body);
        let reopened = open_as_peer(&two.b_http, &token, opening("bill-svc-2"), "open-2").await;
        assert_eq!(reopened.status, 200, "{}", reopened.body);
        assert_eq!(reopened.json()["data"]["status"], json!("active"));
        assert_eq!(account_state(&two.b, &account).await, "suspended");
        assert_eq!(account_events(&two.b).await, ["keel.account.suspended"]);
        let resumed = as_peer(
            &two.b_http,
            &token,
            "keel.account.resume",
            json!({ "id": account }),
            None,
        )
        .await;
        assert_eq!(resumed.status, 403, "{}", resumed.body);
        assert_eq!(account_state(&two.b, &account).await, "suspended");

        two.finish().await;
    }
}
