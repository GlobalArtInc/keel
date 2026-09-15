use keel::prelude::*;
use keel::sdk::sea_orm::{ConnectionTrait, DbBackend, Statement};
use keel_system::notify::api::{ProblemSource, Raise, Severity};

pub const OVERDUE: &str = "note.overdue";

const OVERDUE_SQL: &str = "SELECT account_id, count(*) AS overdue FROM note_item WHERE kind = 'todo' AND due IS NOT NULL AND due < CURRENT_DATE GROUP BY account_id ORDER BY account_id";

#[derive(Serialize)]
#[serde(crate = "keel::sdk::serde")]
struct Overdue {
    count: u64,
}

pub fn source() -> ProblemSource {
    ProblemSource::new(OVERDUE, |db| async move { overdue(&db).await })
}

pub async fn overdue(db: &Db) -> Result<Vec<Raise>> {
    let rows = db
        .query_all(Statement::from_string(DbBackend::Postgres, OVERDUE_SQL))
        .await?;
    let mut raises = Vec::with_capacity(rows.len());
    for row in rows {
        let account: String = row.try_get("", "account_id")?;
        let count: i64 = row.try_get("", "overdue")?;
        let account: Id = account.parse().map_err(|_| {
            KeelError::internal("note.error.account").param("account", account.as_str())
        })?;
        raises.push(
            Raise::new(OVERDUE, &account.to_string(), Severity::Warning)
                .params(Overdue {
                    count: u64::try_from(count).unwrap_or_default(),
                })?
                .account(account),
        );
    }
    Ok(raises)
}
