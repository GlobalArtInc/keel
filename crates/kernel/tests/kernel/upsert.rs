use std::collections::BTreeMap;

use keel_kernel::dispatcher::{CallOptions, Dispatcher, Reply};
use keel_kernel::{Composition, Kernel, Storage};
use keel_sdk::prelude::*;
use keel_testkit::TestDatabase;
use keel_types::{ErrorCode, Scope, Via};
use sea_orm::{ConnectionTrait, DbBackend, Statement};

const ORIGIN: &str = "bill:01J9A00000000000000000000A";
const ACCOUNT_UPSERT: &str = "keel.account.upsert";
const PRINCIPAL_UPSERT: &str = "keel.principal.upsert";
const ROOT: &str = "00000000000000000000000000";

struct Bench {
    database: TestDatabase,
    storage: Storage,
    kernel: Kernel,
    dispatcher: Dispatcher,
    reseller: Id,
}

fn principal(level: Level, account: Id) -> Principal {
    Principal {
        id: Id::generate(),
        login: format!("{level}-{account}"),
        level,
        account_id: account,
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

fn owner() -> Principal {
    principal(Level::Owner, Id::ROOT_ACCOUNT)
}

fn data(reply: &Reply) -> &Value {
    reply
        .data()
        .unwrap_or_else(|| panic!("a result was expected, got {:?}", reply.error()))
}

fn id_of(reply: &Reply, member: &str) -> Id {
    data(reply)[member]
        .as_str()
        .unwrap_or_else(|| panic!("{member} in {:?}", reply.data()))
        .parse()
        .expect("a ulid")
}

fn refused(reply: &Reply) -> (ErrorCode, String) {
    let error = reply
        .error()
        .unwrap_or_else(|| panic!("a refusal was expected, got {:?}", reply.data()));
    (error.code, error.key.clone())
}

fn field(reply: &Reply) -> (String, String) {
    let error = reply
        .error()
        .unwrap_or_else(|| panic!("a refusal was expected, got {:?}", reply.data()));
    assert_eq!(error.code, ErrorCode::Validation, "{error:?}");
    let field = error.fields.first().expect("a field error");
    (field.path.clone(), field.key.clone().unwrap_or_default())
}

fn pair(path: &str, key: &str) -> (String, String) {
    (path.to_owned(), key.to_owned())
}

impl Bench {
    async fn boot() -> Self {
        let database = TestDatabase::create().await;
        let storage = database.storage().await;
        let kernel = Kernel::boot(Composition::new(), &storage)
            .await
            .expect("the bare kernel boots");
        let dispatcher = Dispatcher::new(kernel.registry().clone(), storage.db().clone())
            .expect("every schema compiles");
        let mut bench = Self {
            database,
            storage,
            kernel,
            dispatcher,
            reseller: Id::ROOT_ACCOUNT,
        };
        let reseller = bench
            .call(
                &owner(),
                "keel.account.create",
                json!({ "kind": "reseller" }),
            )
            .await;
        bench.reseller = id_of(&reseller, "id");
        bench
    }

    fn reseller(&self) -> Principal {
        principal(Level::Reseller, self.reseller)
    }

    async fn call(&self, principal: &Principal, func: &str, params: Value) -> Reply {
        self.dispatcher
            .call(principal.clone(), func, params, CallOptions::default())
            .await
    }

    async fn account(&self, principal: &Principal, external_id: &str, extra: Value) -> Reply {
        let mut params = json!({ "origin": ORIGIN, "externalId": external_id });
        if let (Some(params), Some(extra)) = (params.as_object_mut(), extra.as_object()) {
            params.extend(extra.clone());
        }
        self.call(principal, ACCOUNT_UPSERT, params).await
    }

    async fn rows(&self, sql: &str) -> Vec<String> {
        self.storage
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

    async fn events_about(&self, id: Id) -> Vec<String> {
        self.rows(&format!(
            "SELECT type || ' ' || (subject_json->>'kind') || ' ' || (subject_json->>'accountId') || ' ' || data_json::text FROM keel_outbox WHERE subject_json->>'id' = '{id}' ORDER BY seq"
        ))
        .await
    }

    async fn acl_gen(&self, id: Id) -> Vec<String> {
        self.rows(&format!(
            "SELECT acl_gen::text FROM keel_principal WHERE id = '{id}'"
        ))
        .await
    }

    async fn finish(self) {
        self.kernel.stop().await.expect("the kernel stops");
        self.storage.close().await.expect("the pool closes");
        self.database.finish().await;
    }
}

#[tokio::test]
async fn an_account_upsert_creates_once_and_answers_the_same_account_after() {
    let bench = Bench::boot().await;
    let first = bench.account(&owner(), "acc-7", json!({})).await;
    assert_eq!(data(&first)["created"], json!(true));
    let id = id_of(&first, "id");
    let again = bench.account(&owner(), "acc-7", json!({})).await;
    assert_eq!(
        data(&again),
        &json!({ "id": id.to_string(), "created": false })
    );
    assert_eq!(
        bench
            .rows(&format!(
                "SELECT id || ' ' || kind || ' ' || parent_id || ' ' || path || ' ' || state FROM keel_account WHERE origin = '{ORIGIN}' AND external_id = 'acc-7'"
            ))
            .await,
        [format!("{id} user {ROOT} /{id}/ active")]
    );

    let member = bench
        .call(
            &owner(),
            "keel.principal.create",
            json!({ "login": "member", "level": "user", "account": id }),
        )
        .await;
    let member = id_of(&member, "id");
    assert_eq!(bench.acl_gen(member).await, ["0"]);
    let kind = bench
        .account(&owner(), "acc-7", json!({ "kind": "client" }))
        .await;
    assert_eq!(
        data(&kind),
        &json!({ "id": id.to_string(), "created": false })
    );
    assert_eq!(
        bench
            .rows(&format!("SELECT kind FROM keel_account WHERE id = '{id}'"))
            .await,
        ["client"]
    );
    assert_eq!(bench.acl_gen(member).await, ["1"]);
    let unchanged = bench
        .account(
            &owner(),
            "acc-7",
            json!({ "kind": "client", "parent": bench.reseller }),
        )
        .await;
    assert_eq!(data(&unchanged)["created"], json!(false));
    assert_eq!(bench.acl_gen(member).await, ["1"]);
    assert_eq!(
        bench
            .rows(&format!("SELECT path FROM keel_account WHERE id = '{id}'"))
            .await,
        [format!("/{id}/")],
        "the parent applies when the account is created"
    );
    assert_eq!(
        bench.events_about(id).await,
        [format!(
            r#"keel.account.created keel.account {id} {{"id": "{id}", "kind": "user", "state": "active"}}"#
        )]
    );

    let reseller = bench.reseller();
    let under = bench.account(&reseller, "acc-8", json!({})).await;
    let under = id_of(&under, "id");
    let nested = bench
        .account(
            &reseller,
            "acc-9",
            json!({ "parent": under, "kind": "user" }),
        )
        .await;
    let nested = id_of(&nested, "id");
    assert_eq!(
        bench
            .rows(&format!(
                "SELECT path FROM keel_account WHERE id IN ('{under}', '{nested}') ORDER BY length(path)"
            ))
            .await,
        [
            format!("/{}/{under}/", bench.reseller),
            format!("/{}/{under}/{nested}/", bench.reseller)
        ]
    );
    bench.finish().await;
}

#[tokio::test]
async fn an_account_upsert_refuses_a_pair_beyond_the_reach_a_deletion_and_a_malformed_identity() {
    let bench = Bench::boot().await;
    let top = bench.account(&owner(), "acc-top", json!({})).await;
    let top = id_of(&top, "id");
    let reseller = bench.reseller();
    let beyond = bench.account(&reseller, "acc-top", json!({})).await;
    assert_eq!(
        field(&beyond),
        pair("externalId", "keel.account.error.external_taken")
    );
    let params = &beyond.error().unwrap().fields[0].params;
    assert_eq!(params["origin"], json!(ORIGIN));
    assert_eq!(params["externalId"], json!("acc-top"));
    let user = principal(Level::User, top);
    assert_eq!(
        refused(&bench.account(&user, "acc-user", json!({})).await),
        (ErrorCode::Forbidden, "keel.error.level".to_owned())
    );
    let parent = bench
        .account(&reseller, "acc-parent", json!({ "parent": top }))
        .await;
    assert_eq!(
        field(&parent),
        pair("parent", "keel.error.account_not_found")
    );

    bench
        .storage
        .db()
        .execute_unprepared(&format!(
            "UPDATE keel_account SET state = 'deleting' WHERE id = '{top}'"
        ))
        .await
        .expect("mark the account deleting");
    assert_eq!(
        refused(&bench.account(&owner(), "acc-top", json!({})).await),
        (
            ErrorCode::Conflict,
            "keel.account.error.deleting".to_owned()
        )
    );

    let long = "x".repeat(257);
    for (params, path, key) in [
        (
            json!({ "origin": "", "externalId": "acc-1" }),
            "origin",
            "keel.schema.minLength",
        ),
        (
            json!({ "origin": "bill 7", "externalId": "acc-1" }),
            "origin",
            "keel.schema.pattern",
        ),
        (
            json!({ "origin": ORIGIN, "externalId": long }),
            "externalId",
            "keel.schema.maxLength",
        ),
        (
            json!({ "origin": ORIGIN, "externalId": "клиент" }),
            "externalId",
            "keel.schema.pattern",
        ),
        (
            json!({ "origin": ORIGIN, "externalId": "client;42" }),
            "externalId",
            "keel.schema.pattern",
        ),
        (
            json!({ "origin": "bill;7", "externalId": "acc-1" }),
            "origin",
            "keel.schema.pattern",
        ),
        (
            json!({ "origin": ORIGIN, "externalId": "acc-1", "kind": "system" }),
            "kind",
            "keel.account.error.kind_reserved",
        ),
    ] {
        let reply = bench.call(&owner(), ACCOUNT_UPSERT, params.clone()).await;
        assert_eq!(field(&reply), pair(path, key), "{params}");
    }
    assert_eq!(
        bench
            .rows("SELECT count(*)::text FROM keel_account WHERE external_id IS NOT NULL")
            .await,
        ["1"]
    );
    bench.finish().await;
}

#[tokio::test]
async fn an_identity_is_visible_ascii_without_the_separators_of_the_on_behalf_header() {
    let bench = Bench::boot().await;
    let mut refused_bytes = Vec::new();
    for byte in b'!'..=b'~' {
        let symbol = char::from(byte);
        let reply = bench
            .account(&owner(), &format!("acc{symbol}7"), json!({}))
            .await;
        match reply.data() {
            Some(answer) => assert_eq!(answer["created"], json!(true), "{symbol}"),
            None => {
                assert_eq!(
                    field(&reply),
                    pair("externalId", "keel.schema.pattern"),
                    "{symbol}"
                );
                refused_bytes.push(symbol);
            }
        }
    }
    assert_eq!(refused_bytes, ['"', ',', ';', '\\']);
    for separator in refused_bytes {
        let reply = bench
            .call(
                &owner(),
                PRINCIPAL_UPSERT,
                json!({
                    "origin": format!("bill{separator}7"),
                    "externalId": format!("client{separator}42"),
                    "login": "alice",
                    "level": "user",
                    "account": { "origin": ORIGIN, "externalId": "acc-7" },
                }),
            )
            .await;
        let error = reply.error().expect("a refusal");
        let mut fields: Vec<(String, String)> = error
            .fields
            .iter()
            .map(|field| (field.path.clone(), field.key.clone().unwrap_or_default()))
            .collect();
        fields.sort();
        assert_eq!(
            fields,
            [
                pair("externalId", "keel.schema.pattern"),
                pair("origin", "keel.schema.pattern"),
            ],
            "{separator}"
        );
    }
    bench.finish().await;
}

#[tokio::test]
async fn concurrent_upserts_of_one_pair_yield_one_account() {
    let bench = Bench::boot().await;
    let mut calls = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let dispatcher = bench.dispatcher.clone();
        calls.spawn(async move {
            dispatcher
                .call(
                    owner(),
                    ACCOUNT_UPSERT,
                    json!({ "origin": ORIGIN, "externalId": "acc-race" }),
                    CallOptions::default(),
                )
                .await
        });
    }
    let mut ids = Vec::new();
    let mut created = 0;
    while let Some(reply) = calls.join_next().await {
        let reply = reply.expect("the call finishes");
        ids.push(id_of(&reply, "id"));
        if data(&reply)["created"] == json!(true) {
            created += 1;
        }
    }
    ids.dedup();
    assert_eq!(ids.len(), 1, "{ids:?}");
    assert_eq!(created, 1);
    assert_eq!(
        bench
            .rows("SELECT count(*)::text FROM keel_account WHERE external_id = 'acc-race'")
            .await,
        ["1"]
    );
    assert_eq!(bench.events_about(ids[0]).await.len(), 1);
    bench.finish().await;
}

fn customer(login: &str, account: Value) -> Value {
    json!({
        "origin": ORIGIN,
        "externalId": "client-42",
        "login": login,
        "level": "user",
        "account": account,
    })
}

#[tokio::test]
async fn a_principal_upsert_creates_once_then_renames_moves_and_relevels_it() {
    let bench = Bench::boot().await;
    let acc7 = json!({ "origin": ORIGIN, "externalId": "acc-7" });
    let first = bench
        .call(&owner(), PRINCIPAL_UPSERT, customer("alice", acc7.clone()))
        .await;
    assert_eq!(data(&first)["created"], json!(true));
    let id = id_of(&first, "id");
    let account = id_of(&first, "accountId");
    assert_eq!(
        bench
            .rows(&format!(
                "SELECT login || ' ' || level || ' ' || role || ' ' || lang || ' ' || origin || ' ' || external_id || ' ' || account_id FROM keel_principal WHERE id = '{id}'"
            ))
            .await,
        [format!("alice user owner en {ORIGIN} client-42 {account}")]
    );
    assert_eq!(
        bench
            .rows(&format!(
                "SELECT origin || ' ' || external_id FROM keel_account WHERE id = '{account}'"
            ))
            .await,
        [format!("{ORIGIN} acc-7")]
    );
    let again = bench
        .call(&owner(), PRINCIPAL_UPSERT, customer("alice", acc7.clone()))
        .await;
    assert_eq!(
        data(&again),
        &json!({ "id": id.to_string(), "accountId": account.to_string(), "created": false })
    );
    assert_eq!(bench.acl_gen(id).await, ["0"]);

    let mut renamed = customer("alice2", acc7.clone());
    renamed["lang"] = json!("ru");
    renamed["role"] = json!("member");
    let renamed = bench.call(&owner(), PRINCIPAL_UPSERT, renamed).await;
    assert_eq!(data(&renamed)["created"], json!(false));
    assert_eq!(
        bench
            .rows(&format!(
                "SELECT login || ' ' || role || ' ' || lang || ' ' || acl_gen FROM keel_principal WHERE id = '{id}'"
            ))
            .await,
        ["alice2 member ru 1"]
    );

    let moved = bench
        .call(
            &owner(),
            PRINCIPAL_UPSERT,
            json!({
                "origin": ORIGIN,
                "externalId": "client-42",
                "login": "alice2",
                "level": "reseller",
                "account": { "origin": ORIGIN, "externalId": "acc-8", "kind": "reseller" },
            }),
        )
        .await;
    let other = id_of(&moved, "accountId");
    assert_ne!(other, account);
    assert_eq!(
        bench
            .rows(&format!(
                "SELECT level || ' ' || account_id || ' ' || acl_gen FROM keel_principal WHERE id = '{id}'"
            ))
            .await,
        [format!("reseller {other} 2")]
    );
    assert_eq!(
        bench
            .rows(&format!(
                "SELECT count(*)::text FROM keel_principal WHERE origin = '{ORIGIN}' AND external_id = 'client-42'"
            ))
            .await,
        ["1"]
    );
    assert_eq!(
        bench.events_about(id).await,
        [
            format!(
                r#"keel.principal.created keel.principal {account} {{"id": "{id}", "login": "alice", "accountId": "{account}"}}"#
            ),
            format!(
                r#"keel.principal.renamed keel.principal {account} {{"id": "{id}", "login": "alice2"}}"#
            ),
        ]
    );
    for created in [account, other] {
        assert_eq!(bench.events_about(created).await.len(), 1);
    }
    bench.finish().await;
}

#[tokio::test]
async fn a_principal_upsert_keeps_the_level_rules_the_unique_login_and_the_reach() {
    let bench = Bench::boot().await;
    let acc7 = json!({ "origin": ORIGIN, "externalId": "acc-7" });
    let mut admin = customer("alice", acc7.clone());
    admin["level"] = json!("admin");
    assert_eq!(
        field(&bench.call(&owner(), PRINCIPAL_UPSERT, admin).await),
        pair("level", "keel.principal.error.level_account")
    );
    bench
        .call(
            &owner(),
            "keel.principal.create",
            json!({ "login": "taken", "level": "user" }),
        )
        .await;
    assert_eq!(
        field(
            &bench
                .call(&owner(), PRINCIPAL_UPSERT, customer("taken", acc7.clone()))
                .await
        ),
        pair("login", "keel.principal.error.login_exists")
    );
    let reseller = bench.reseller();
    let mut above = customer("alice", json!({ "origin": ORIGIN, "externalId": "acc-r" }));
    above["level"] = json!("admin");
    assert_eq!(
        field(&bench.call(&reseller, PRINCIPAL_UPSERT, above).await),
        pair("level", "keel.principal.error.level_above_caller")
    );
    assert_eq!(
        bench
            .rows("SELECT count(*)::text FROM keel_account WHERE external_id IS NOT NULL")
            .await,
        ["0"],
        "a refused principal leaves no account behind"
    );
    let top = bench.account(&owner(), "acc-top", json!({})).await;
    let top = id_of(&top, "id");
    for (account, path, key) in [
        (
            json!({ "origin": ORIGIN, "externalId": "acc-9", "kind": "system" }),
            "account.kind",
            "keel.account.error.kind_reserved",
        ),
        (
            json!({ "origin": ORIGIN, "externalId": "acc 9" }),
            "account.externalId",
            "keel.schema.pattern",
        ),
        (
            json!({ "origin": ORIGIN, "externalId": "acc-9", "parent": top }),
            "account.parent",
            "keel.error.account_not_found",
        ),
        (
            json!({ "origin": ORIGIN, "externalId": "acc-top" }),
            "account.externalId",
            "keel.account.error.external_taken",
        ),
    ] {
        let reply = bench
            .call(
                &reseller,
                PRINCIPAL_UPSERT,
                customer("bob", account.clone()),
            )
            .await;
        assert_eq!(field(&reply), pair(path, key), "{account}");
    }

    let beyond = bench
        .call(
            &owner(),
            PRINCIPAL_UPSERT,
            customer(
                "carol",
                json!({ "origin": ORIGIN, "externalId": "acc-top" }),
            ),
        )
        .await;
    assert_eq!(data(&beyond)["created"], json!(true));
    let taken = bench
        .call(
            &reseller,
            PRINCIPAL_UPSERT,
            customer("carol", json!({ "origin": ORIGIN, "externalId": "acc-r" })),
        )
        .await;
    assert_eq!(
        field(&taken),
        pair("externalId", "keel.principal.error.external_taken")
    );
    assert_eq!(
        bench
            .rows("SELECT account_id FROM keel_principal WHERE external_id = 'client-42'")
            .await,
        [top.to_string()]
    );
    bench.finish().await;
}

#[tokio::test]
async fn creating_and_renaming_emit_the_identity_events_and_an_unchanged_login_emits_none() {
    let bench = Bench::boot().await;
    let account = bench
        .call(&owner(), "keel.account.create", json!({ "kind": "user" }))
        .await;
    let account = id_of(&account, "id");
    assert_eq!(
        bench.events_about(account).await,
        [format!(
            r#"keel.account.created keel.account {account} {{"id": "{account}", "kind": "user", "state": "active"}}"#
        )]
    );
    let created = bench
        .call(
            &owner(),
            "keel.principal.create",
            json!({ "login": "dave", "level": "user", "account": account }),
        )
        .await;
    let id = id_of(&created, "id");
    for params in [
        json!({ "id": id, "lang": "ru" }),
        json!({ "id": id, "login": "dave" }),
        json!({ "id": id, "login": "erin" }),
        json!({ "id": id, "login": "erin", "role": "member" }),
    ] {
        let reply = bench.call(&owner(), "keel.principal.update", params).await;
        assert!(reply.data().is_some(), "{:?}", reply.error());
    }
    assert_eq!(
        bench.events_about(id).await,
        [
            format!(
                r#"keel.principal.created keel.principal {account} {{"id": "{id}", "login": "dave", "accountId": "{account}"}}"#
            ),
            format!(
                r#"keel.principal.renamed keel.principal {account} {{"id": "{id}", "login": "erin"}}"#
            ),
        ]
    );
    bench.finish().await;
}
