use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use keel_kernel::dispatcher::{
    CallOptions, Dispatcher, OUTBOX_BATCH, RELAY_LOCAL_KEY, RELAY_TAKEN_KEY, Reply,
};
use keel_kernel::{Composition, Kernel, Storage};
use keel_sdk::Registrar;
use keel_sdk::prelude::*;
use keel_sdk::system::{EventRelay, OutboxSubscriber};
use keel_testkit::TestDatabase;
use keel_types::{ErrorCode, Event, EventActor, EventActorKind, EventSubject, Scope, Via};
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

fn foreign(origin: Id, label: &str) -> Event {
    Event {
        id: Id::generate(),
        seq: 8812,
        kind: "dns.zone.created".into(),
        at: "2026-09-12T10:00:00.000Z".parse().expect("an instant"),
        instance_id: origin,
        module: "dns".into(),
        actor: Some(EventActor {
            kind: EventActorKind::Peer,
            id: Id::generate(),
        }),
        subject: Some(EventSubject {
            kind: "dns.zone".into(),
            id: Id::generate(),
            account_id: Some(Id::generate()),
        }),
        data: json!({ "label": label }),
    }
}

#[tokio::test]
async fn a_relayed_event_enters_the_outbox_once_under_its_own_id_and_origin_and_wakes_the_feed() {
    let bench = bench().await;
    let relay = bench
        .kernel
        .registry()
        .services()
        .get::<dyn EventRelay>()
        .expect("the kernel provides the relay");
    let mut latest = bench.subscriber.latest();
    bench.ping("local", false).await;
    let local = changed(&mut latest).await;
    let origin = Id::generate();
    let event = foreign(origin, "remote");
    let seq = relay
        .relay(&event)
        .await
        .expect("the relay writes")
        .expect("a new event gets a seq");
    assert!(seq > local, "{seq} follows {local}");
    assert_eq!(changed(&mut latest).await, seq);
    let read = bench.after(local).await;
    assert_eq!(read.len(), 1);
    let relayed = &read[0];
    assert_eq!(relayed.seq, seq);
    assert_eq!(relayed.id, event.id);
    assert_eq!(relayed.instance_id, origin);
    assert_eq!(relayed.kind, event.kind);
    assert_eq!(relayed.module, event.module);
    assert_eq!(relayed.at, event.at);
    assert_eq!(relayed.actor, event.actor);
    assert_eq!(relayed.subject, event.subject);
    assert_eq!(relayed.data, event.data);
    let everything = bench.after(0).await;
    assert_eq!(everything[0].instance_id, bench.instance);
    assert_eq!(labels(&everything), ["local", "remote"]);
    assert_eq!(
        relay.relay(&event).await.expect("a repeat is no error"),
        None
    );
    assert_eq!(bench.after(0).await.len(), 2);
    let mut own = foreign(origin, "own");
    own.instance_id = bench.instance;
    let refused = relay
        .relay(&own)
        .await
        .expect_err("an event of this installation is not relayed");
    assert_eq!(refused.code(), ErrorCode::Validation);
    assert_eq!(refused.key(), RELAY_LOCAL_KEY);
    assert_eq!(refused.params()["id"], json!(own.id.to_string()));
    assert_eq!(bench.after(0).await.len(), 2);
    bench.finish().await;
}

#[tokio::test]
async fn an_event_of_a_call_through_a_peer_link_names_the_link_as_its_peer_actor() {
    let bench = bench().await;
    let link = Id::generate();
    let mut direct = owner();
    direct.level = Level::Admin;
    direct.via = Via::Peer(link);
    let mut customer = owner();
    customer.level = Level::Admin;
    customer.via = Via::Peer(link);
    customer.actor = Some(Box::new(direct.clone()));
    for principal in [direct.clone(), customer.clone()] {
        let reply = bench
            .dispatcher
            .call(
                principal,
                "feeder.item.ping",
                json!({ "label": "peer" }),
                CallOptions::default(),
            )
            .await;
        assert_eq!(reply.status(), 200, "{:?}", reply.error());
    }
    let actors: Vec<Option<EventActor>> = bench
        .after(0)
        .await
        .into_iter()
        .map(|event| event.actor)
        .collect();
    assert_eq!(
        actors,
        [
            Some(EventActor {
                kind: EventActorKind::Peer,
                id: direct.id,
            }),
            Some(EventActor {
                kind: EventActorKind::Peer,
                id: direct.id,
            }),
        ]
    );
    bench.finish().await;
}

#[tokio::test]
async fn an_id_the_bus_holds_for_another_installation_is_refused_rather_than_taken_for_a_repeat() {
    let bench = bench().await;
    let relay = bench
        .kernel
        .registry()
        .services()
        .get::<dyn EventRelay>()
        .expect("the kernel provides the relay");
    let first = Id::generate();
    let event = foreign(first, "first");
    relay
        .relay(&event)
        .await
        .expect("the relay writes")
        .expect("a new event gets a seq");
    let mut forged = foreign(Id::generate(), "forged");
    forged.id = event.id;
    let refused = relay
        .relay(&forged)
        .await
        .expect_err("an id held for another installation is no repeat");
    assert_eq!(refused.code(), ErrorCode::Validation);
    assert_eq!(refused.key(), RELAY_TAKEN_KEY);
    assert_eq!(refused.params()["id"], json!(event.id.to_string()));
    assert_eq!(refused.params()["instanceId"], json!(first.to_string()));
    assert_eq!(
        relay.relay(&event).await.expect("a repeat is no error"),
        None
    );
    bench.ping("local", false).await;
    let local = bench
        .after(0)
        .await
        .into_iter()
        .find(|seen| seen.instance_id == bench.instance)
        .expect("the local event is on the bus");
    let mut copied = foreign(Id::generate(), "copied");
    copied.id = local.id;
    let refused = relay
        .relay(&copied)
        .await
        .expect_err("an id of a local event is no repeat");
    assert_eq!(refused.key(), RELAY_TAKEN_KEY);
    assert_eq!(
        refused.params()["instanceId"],
        json!(bench.instance.to_string())
    );
    assert_eq!(labels(&bench.after(0).await), ["first", "local"]);
    bench.finish().await;
}
