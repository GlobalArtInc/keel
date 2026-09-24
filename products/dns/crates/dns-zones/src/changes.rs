use keel::prelude::*;
use keel::sdk::chrono::{DateTime, Datelike, Utc};
use keel::sdk::sea_orm::{ConnectionTrait, DbBackend, Statement};

use crate::api::{ZoneRef, Zones};
use crate::bind::BindProvider;
use crate::provider::DnsProvider;

const SERIAL_REVISIONS: i64 = 100;
const LOCK: &str = "SELECT account_id FROM dns_zone WHERE id = $1 FOR UPDATE";
const BUMP: &str = "UPDATE dns_zone SET \
     serial = CASE WHEN changed_in IS DISTINCT FROM pg_current_xact_id() \
     THEN GREATEST(serial + 1, $2) ELSE serial END, \
     changed_in = pg_current_xact_id(), state = 'pending', updated_at = $3 \
     WHERE id = $1 RETURNING id, account_id, serial";

pub(crate) struct ZoneChanges {
    bind: Arc<BindProvider>,
}

impl ZoneChanges {
    pub(crate) fn new(bind: Arc<BindProvider>) -> Self {
        Self { bind }
    }
}

#[async_trait]
impl Zones for ZoneChanges {
    async fn change(
        &self,
        db: &Db,
        scope: &AccountScope,
        zone: &Id,
        now: DateTime<Utc>,
    ) -> Result<ZoneRef> {
        let id = zone.to_string();
        let locked = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                LOCK,
                [id.as_str().into()],
            ))
            .await?
            .ok_or_else(not_found)?;
        let account: String = locked.try_get("", "account_id")?;
        let account: Id = account.parse().map_err(|_| not_found())?;
        if !scope.includes(db, account).await? {
            return Err(not_found());
        }
        bump(db, &id, now).await?.ok_or_else(not_found)
    }

    async fn publish(&self, ctx: &AfterContext, zone: &Id) -> Result<()> {
        self.bind.apply(ctx, &zone.to_string()).await
    }
}

pub(crate) async fn bump<C: ConnectionTrait>(
    db: &C,
    zone: &str,
    now: DateTime<Utc>,
) -> Result<Option<ZoneRef>> {
    let Some(row) = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            BUMP,
            [zone.into(), first_serial(now).into(), now.into()],
        ))
        .await?
    else {
        return Ok(None);
    };
    let id: String = row.try_get("", "id")?;
    let account: String = row.try_get("", "account_id")?;
    Ok(Some(ZoneRef {
        id: id.parse().map_err(|_| not_found())?,
        account: account.parse().map_err(|_| not_found())?,
        serial: row.try_get("", "serial")?,
    }))
}

pub(crate) fn first_serial(now: DateTime<Utc>) -> i64 {
    let day = i64::from(now.year()) * 10_000 + i64::from(now.month()) * 100 + i64::from(now.day());
    day * SERIAL_REVISIONS
}

fn not_found() -> KeelError {
    KeelError::not_found("keel.error.not_found")
}

#[cfg(test)]
mod tests {
    use keel::sdk::chrono::TimeZone;

    use super::*;

    #[test]
    fn the_first_serial_of_a_day_is_the_date_followed_by_two_zero_digits() {
        let day = Utc
            .with_ymd_and_hms(2026, 9, 24, 23, 59, 59)
            .single()
            .expect("a time");
        assert_eq!(first_serial(day), 2_026_092_400);
    }
}
