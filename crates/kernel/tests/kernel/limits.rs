use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::Duration;

use keel_sdk::Registrar;
use keel_sdk::limit::Unit;
use keel_sdk::prelude::*;
use keel_sdk::system::LimitSource;
use keel_testkit::{Harness, Reply, Who, tokio};
use keel_types::ErrorCode;
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::json;

const ITEMS: &str = "quota.items";
const SEATS: &str = "quota.seats";

static STAMPED: LazyLock<Mutex<Vec<String>>> = LazyLock::new(Mutex::default);

#[derive(Deserialize, Serialize, JsonSchema)]
struct Check {
    current: u64,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Nothing {}

static QUOTA: ModuleDef = ModuleDef {
    id: "quota",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.contribute(LimitDef::new(ITEMS, Unit::Count).default(3).unlimited(true))
        .contribute(LimitDef::new(SEATS, Unit::Count).default(2))
        .function(
            function::<Check, Nothing>("quota.check")
                .kind(Kind::Action)
                .level(Level::User)
                .unowned()
                .handler(|call: Call<Check>| async move {
                    call.check_limit(ITEMS, call.params().current).await?;
                    let login = call.principal().login.clone();
                    call.after_commit(move |ctx| async move {
                        let row = ctx
                            .db()?
                            .query_one(Statement::from_string(
                                DbBackend::Postgres,
                                "SELECT count(*)::text AS n FROM keel_account",
                            ))
                            .await?
                            .ok_or_else(|| KeelError::internal("keel.error.internal"))?;
                        let accounts: String = row.try_get("", "n")?;
                        STAMPED
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .push(format!("{login} {accounts}"));
                        Ok(())
                    });
                    Ok(Nothing {})
                }),
        );
    Ok(())
}

fn code(reply: &Reply) -> Option<ErrorCode> {
    reply.error().map(|error| error.code)
}

async fn store(harness: &Harness, account: Id, value: Value) {
    harness
        .db()
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO keel_limit (account_id, key, value) VALUES ($1, $2, $3) ON CONFLICT (account_id, key) DO UPDATE SET value = EXCLUDED.value",
            [account.to_string().into(), ITEMS.into(), value.into()],
        ))
        .await
        .expect("the limit is stored");
}

async fn check(harness: &Harness, who: Who, current: u64) -> Option<ErrorCode> {
    code(
        &harness
            .call(who, "quota.check", json!({ "current": current }))
            .await,
    )
}

#[tokio::test]
async fn a_limit_takes_its_default_the_tightest_value_up_the_path_and_null_as_unlimited() {
    let harness = Harness::new().module(&QUOTA).boot().await;
    let accounts = harness.fixtures().accounts.clone();

    assert_eq!(check(&harness, Who::A1, 2).await, None);
    let refused = harness
        .call(Who::A1, "quota.check", json!({ "current": 3 }))
        .await;
    let error = refused.error().expect("the default of three is reached");
    assert_eq!(error.code, ErrorCode::LimitExceeded);
    assert_eq!(error.key, "keel.error.limit_exceeded");
    assert_eq!(error.params["limit"], json!(ITEMS));
    assert_eq!(error.params["max"], json!(3));

    store(&harness, accounts.a1, json!(10)).await;
    assert_eq!(check(&harness, Who::A1, 9).await, None);
    assert_eq!(
        check(&harness, Who::A1, 10).await,
        Some(ErrorCode::LimitExceeded)
    );

    store(&harness, accounts.reseller, json!(5)).await;
    assert_eq!(
        check(&harness, Who::A1, 5).await,
        Some(ErrorCode::LimitExceeded)
    );
    assert_eq!(
        check(&harness, Who::A2, 5).await,
        Some(ErrorCode::LimitExceeded)
    );
    assert_eq!(check(&harness, Who::Unrelated, 2).await, None);
    assert_eq!(
        check(&harness, Who::Unrelated, 3).await,
        Some(ErrorCode::LimitExceeded)
    );

    store(&harness, accounts.unrelated, json!(null)).await;
    assert_eq!(check(&harness, Who::Unrelated, 1_000).await, None);
    harness.finish().await;
}

#[tokio::test]
async fn an_after_commit_hook_reads_through_the_pool_of_its_context() {
    let harness = Harness::new().module(&QUOTA).boot().await;
    let login = harness.principal(Who::A2).login.clone();
    assert_eq!(check(&harness, Who::A2, 0).await, None);
    let mut stamped = Vec::new();
    for _ in 0..100 {
        stamped = STAMPED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|line| line.starts_with(&format!("{login} ")))
            .cloned()
            .collect();
        if !stamped.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let accounts = harness
        .database()
        .rows("SELECT count(*)::text FROM keel_account")
        .await;
    assert_eq!(stamped, [format!("{login} {}", accounts[0])]);
    harness.finish().await;
}

#[tokio::test]
async fn a_module_stores_an_account_limit_and_null_only_where_the_limit_admits_unlimited() {
    let harness = Harness::new().module(&QUOTA).boot().await;
    let accounts = harness.fixtures().accounts.clone();
    let limits = harness
        .registry()
        .services()
        .get::<dyn LimitSource>()
        .expect("the kernel provides the limits");
    let db = Db::from_connection(harness.db().clone());
    let stored = || async {
        harness
            .database()
            .rows("SELECT account_id || ' ' || key || ' ' || value::text FROM keel_limit ORDER BY account_id, key")
            .await
    };

    limits
        .store(ITEMS, accounts.a1, Some(1), &db)
        .await
        .expect("a value is stored");
    assert_eq!(stored().await, [format!("{} {ITEMS} 1", accounts.a1)]);
    assert_eq!(check(&harness, Who::A1, 0).await, None);
    assert_eq!(
        check(&harness, Who::A1, 1).await,
        Some(ErrorCode::LimitExceeded)
    );
    assert_eq!(check(&harness, Who::Unrelated, 2).await, None);

    limits
        .store(ITEMS, accounts.a1, None, &db)
        .await
        .expect("null replaces the value");
    assert_eq!(stored().await, [format!("{} {ITEMS} null", accounts.a1)]);
    assert_eq!(check(&harness, Who::A1, 1_000).await, None);
    assert_eq!(
        limits.effective(ITEMS, accounts.a1, &db).await.unwrap(),
        None
    );

    limits
        .store(SEATS, accounts.a1, Some(7), &db)
        .await
        .expect("a value of a limit without unlimited is stored");
    assert_eq!(
        limits.effective(SEATS, accounts.a1, &db).await.unwrap(),
        Some(7)
    );
    for (key, value) in [(SEATS, None), ("quota.unknown", Some(1))] {
        let refused = limits
            .store(key, accounts.a2, value, &db)
            .await
            .expect_err("the value is refused");
        assert_eq!(refused.code(), ErrorCode::Internal);
        assert_eq!(refused.params()["limit"], json!(key));
    }
    assert_eq!(
        stored().await,
        [
            format!("{} {ITEMS} null", accounts.a1),
            format!("{} {SEATS} 7", accounts.a1),
        ]
    );
    harness.finish().await;
}
