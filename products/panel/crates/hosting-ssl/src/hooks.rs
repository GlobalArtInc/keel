use keel::prelude::*;
use keel::sdk::chrono::Duration;
use keel::sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use keel::sdk::serde_json;
use keel_system::cron::api::CronJob;
use keel_system::notify::api::{ProblemSource, Raise, Severity};

use crate::certificate;
use crate::functions::{RENEW, drop_rows};
use crate::install::{Installer, Store, origin, parse_id};

pub(crate) const EXPIRING: &str = "hosting.certificate.expiring";

const RENEW_JOB: &str = "hosting.certificate.renew";
const RENEW_SCHEDULE: &str = "40 3 * * *";
const EXPIRING_DAYS: i64 = 14;

#[derive(Serialize)]
#[serde(crate = "keel::sdk::serde", rename_all = "camelCase")]
struct Expiring {
    domain: String,
    not_after: String,
}

pub(crate) async fn forget_sites(call: Call<Value>, store: Arc<Store>) -> Result<()> {
    let ids: Vec<Id> = serde_json::from_value(call.params()["ids"].clone())?;
    let ids: Vec<String> = ids.iter().map(ToString::to_string).collect();
    let gone = drop_rows(call.tx()?, certificate::Column::SiteId.is_in(ids)).await?;
    drop_after_commit(&call, &store, gone)
}

pub(crate) fn lifecycle(store: Arc<Store>) -> AccountLifecycle {
    AccountLifecycle::new().on_delete(move |call: Call<Id>| {
        let store = Arc::clone(&store);
        async move {
            let account = call.params().to_string();
            let gone = drop_rows(
                call.tx()?,
                certificate::Column::AccountId.eq(account.as_str()),
            )
            .await?;
            if let Some(task) = call.task() {
                task.log(&format!(
                    "removed {} certificate(s) of account {account}",
                    gone.len()
                ))
                .await?;
            }
            drop_after_commit(&call, &store, gone)
        }
    })
}

pub(crate) fn renewal() -> CronJob {
    CronJob::new(RENEW_JOB, RENEW_SCHEDULE, |run| async move {
        run.invoke(RENEW, json!({})).await.map(drop)
    })
}

pub(crate) fn problems(store: Arc<Store>) -> ProblemSource {
    ProblemSource::new(EXPIRING, move |db| {
        let store = Arc::clone(&store);
        async move { expiring(&db, &store).await }
    })
}

async fn expiring(db: &Db, store: &Store) -> Result<Vec<Raise>> {
    let now = store.now()?;
    let rows = certificate::Entity::find()
        .filter(certificate::Column::Installed.is_not_null())
        .filter(certificate::Column::NotAfter.lt(now + Duration::days(EXPIRING_DAYS)))
        .order_by_asc(certificate::Column::NotAfter)
        .all(db)
        .await?;
    let mut raises = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(not_after) = row.not_after else {
            continue;
        };
        let severity = if not_after <= now {
            Severity::Critical
        } else {
            Severity::Warning
        };
        raises.push(
            Raise::new(EXPIRING, &row.id, severity)
                .params(Expiring {
                    domain: row.domain,
                    not_after: not_after.to_rfc3339(),
                })?
                .account(parse_id(&row.account_id)?),
        );
    }
    Ok(raises)
}

fn drop_after_commit<P>(
    call: &Call<P>,
    store: &Arc<Store>,
    gone: Vec<certificate::Model>,
) -> Result<()> {
    let stems: Vec<String> = gone.into_iter().filter_map(|row| row.installed).collect();
    if stems.is_empty() {
        return Ok(());
    }
    let installer = Installer::of(call, store)?;
    call.after_commit(move |ctx| async move {
        installer
            .drop_files(origin(ctx.principal(), ctx.request_id()), &stems)
            .await
    });
    Ok(())
}
