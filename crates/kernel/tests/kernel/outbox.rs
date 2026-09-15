use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use keel_kernel::dispatcher::{CallOptions, Dispatcher, OUTBOX_BATCH, Reply};
use keel_kernel::{Composition, Kernel, Storage};
use keel_sdk::Registrar;
use keel_sdk::prelude::*;
use keel_sdk::system::OutboxSubscriber;
use keel_testkit::TestDatabase;
use keel_types::{Event, EventActorKind, Scope, Via};
use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait, Value as DbValue};
use tokio::sync::{Notify, watch};

const PATIENCE: Duration = Duration::from_secs(5);
const BLINK: Duration = Duration::from_millis(300);

#[derive(Deserialize, Serialize, JsonSchema)]
struct Ping {
    label: String,
    #[serde(default)]
    hold: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Pong {
    label: String,
}

static RELEASE: Notify = Notify::const_new();

static FEEDER: ModuleDef = ModuleDef {
    id: "feeder",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.function(
        function::<Ping, Pong>("feeder.item.ping")
            .kind(Kind::Action)
            .level(Level::Admin)
            .unowned()
            .handler(|call: Call<Ping>| async move {
                let label = call.params().label.clone();
                call.emit("feeder.item.pinged", &json!({ "label": label }))?;
                if call.params().hold {
                    RELEASE.notified().await;
                }
                Ok(Pong { label })
            }),
    );
    Ok(())
}

fn owner() -> Principal {
    Principal {
        id: Id::generate(),
        login: "owner".into(),
        level: Level::Owner,
        account_id: Id::ROOT_ACCOUNT,
        role: "owner".into(),
        actor: None,
        scope: Scope::Full,
        via: Via::Http,
        external_id: None,
        origin: None,
        lang: "en".into(),
        grants: BTreeMap::new(),
    }
}

struct Bench {
    database: TestDatabase,
    storage: Storage,
    kernel: Kernel,
    dispatcher: Dispatcher,
    instance: Id,
    subscriber: Arc<dyn OutboxSubscriber>,
}

async fn bench() -> Bench {
    let database = TestDatabase::create().await;
    let storage = database.storage().await;
    let kernel = Kernel::boot(Composition::new().module(&FEEDER), &storage)
        .await
        .expect("the kernel boots");
    let dispatcher = Dispatcher::new(kernel.registry().clone(), storage.db().clone())
        .expect("every schema compiles");
    let subscriber = kernel
        .registry()
        .services()
        .get::<dyn OutboxSubscriber>()
        .expect("the kernel provides the outbox subscriber");
    let unfilled = subscriber
        .after(0, 10)
        .await
        .expect_err("the subscriber answers nothing before the runtime fills it");
    assert_eq!(unfilled.key(), "keel.error.outbox_unavailable");
    let instance = Id::generate();
    assert!(dispatcher.fill_outbox(instance));
    Bench {
        database,
        storage,
        kernel,
        dispatcher,
        instance,
        subscriber,
    }
}

impl Bench {
    async fn ping(&self, label: &str, hold: bool) -> Reply {
        self.dispatcher
            .call(
                owner(),
                "feeder.item.ping",
                json!({ "label": label, "hold": hold }),
                CallOptions::default(),
            )
            .await
    }

    async fn after(&self, seq: u64) -> Vec<Event> {
        self.subscriber
            .after(seq, OUTBOX_BATCH)
            .await
            .expect("the outbox answers")
    }

    async fn finish(self) {
        self.kernel.stop().await.expect("the kernel stops");
        self.storage.close().await.expect("the pool closes");
        self.database.finish().await;
    }
}

fn labels(events: &[Event]) -> Vec<&str> {
    events
        .iter()
        .map(|event| event.data["label"].as_str().unwrap_or_default())
        .collect()
}

async fn changed(latest: &mut watch::Receiver<u64>) -> u64 {
    tokio::time::timeout(PATIENCE, latest.changed())
        .await
        .expect("the watch wakes in time")
        .expect("the sender lives");
    *latest.borrow_and_update()
}

#[tokio::test]
async fn outbox_subscriber_answers_events_after_a_seq_with_the_instance_id_and_wakes_a_waiter_on_commit()
 {
    let bench = bench().await;
    let mut latest = bench.subscriber.latest();
    assert_eq!(*latest.borrow_and_update(), 0);
    assert!(bench.after(0).await.is_empty());
    let reply = bench.ping("first", false).await;
    assert_eq!(reply.status(), 200, "{:?}", reply.error());
    let seq = changed(&mut latest).await;
    let events = bench.after(0).await;
    assert_eq!(events.len(), 1);
    let first = &events[0];
    assert_eq!(first.seq, seq);
    assert_eq!(first.kind, "feeder.item.pinged");
    assert_eq!(first.module, "feeder");
    assert_eq!(first.instance_id, bench.instance);
    assert_eq!(first.data, json!({ "label": "first" }));
    let actor = first.actor.as_ref().expect("the caller is the actor");
    assert_eq!(actor.kind, EventActorKind::Principal);
    assert!(first.subject.is_none());
    assert!(bench.after(seq).await.is_empty());
    for label in ["second", "third"] {
        bench.ping(label, false).await;
    }
    let newest = changed(&mut latest).await;
    assert!(newest > seq, "{newest} follows {seq}");
    let rest = bench.after(seq).await;
    assert_eq!(labels(&rest), ["second", "third"]);
    assert!(rest.windows(2).all(|pair| pair[0].seq < pair[1].seq));
    assert_eq!(rest[1].seq, newest);
    assert_eq!(bench.after(0).await.len(), 3);
    assert_eq!(
        bench
            .subscriber
            .after(0, 2)
            .await
            .expect("a limited page")
            .len(),
        2
    );
    let wire = serde_json::to_value(first).expect("an event serialises");
    assert_eq!(wire["instanceId"], json!(bench.instance.to_string()));
    assert_eq!(wire["type"], json!("feeder.item.pinged"));
    bench.finish().await;
}

#[tokio::test]
async fn a_transaction_writing_an_event_waits_for_the_earlier_writer_to_commit_so_after_never_skips_a_row()
 {
    let bench = bench().await;
    let mut latest = bench.subscriber.latest();
    let earlier = bench
        .storage
        .db()
        .begin()
        .await
        .expect("a transaction opens");
    earlier
        .execute_unprepared(&format!(
            "SELECT pg_advisory_xact_lock({})",
            keel_kernel::storage::lock::OUTBOX
        ))
        .await
        .expect("the earlier writer takes the outbox lock");
    let row = earlier
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO keel_outbox (id, type, module, at, actor_json, subject_json, data_json, created_at) VALUES ($1, 'feeder.item.pinged', 'feeder', now(), NULL, NULL, $2, now()) RETURNING seq",
            [
                DbValue::from(Id::generate().to_string()),
                DbValue::Json(Some(Box::new(json!({ "label": "earlier" })))),
            ],
        ))
        .await
        .expect("the earlier writer inserts")
        .expect("a seq is returned");
    let earlier_seq: i64 = row.try_get("", "seq").expect("a seq");
    let dispatcher = bench.dispatcher.clone();
    let blocked = tokio::spawn(async move {
        dispatcher
            .call(
                owner(),
                "feeder.item.ping",
                json!({ "label": "blocked", "hold": false }),
                CallOptions::default(),
            )
            .await
    });
    tokio::time::sleep(BLINK).await;
    assert!(
        !blocked.is_finished(),
        "the second writer waits for the first to commit"
    );
    assert!(bench.after(0).await.is_empty());
    assert_eq!(*latest.borrow_and_update(), 0);
    earlier.commit().await.expect("the earlier writer commits");
    let blocked = tokio::time::timeout(PATIENCE, blocked)
        .await
        .expect("the second writer commits after the first")
        .expect("the task joins");
    assert_eq!(blocked.status(), 200, "{:?}", blocked.error());
    let events = bench.after(0).await;
    assert_eq!(labels(&events), ["earlier", "blocked"]);
    assert_eq!(events[0].seq, u64::try_from(earlier_seq).expect("positive"));
    assert_eq!(events[1].seq, events[0].seq + 1);
    let newest = changed(&mut latest).await;
    assert_eq!(newest, events[1].seq);
    assert_eq!(bench.after(events[0].seq).await.len(), 1);
    let held = bench.ping("held", true);
    let released = async {
        tokio::time::sleep(BLINK).await;
        RELEASE.notify_one();
    };
    let (held, ()) = tokio::join!(held, released);
    assert_eq!(held.status(), 200, "{:?}", held.error());
    assert_eq!(labels(&bench.after(newest).await), ["held"]);
    bench.finish().await;
}
