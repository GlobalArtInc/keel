use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ConnectionTrait, DbBackend, Statement};

const STATEMENTS: [&str; 2] = [
    "DELETE FROM settings_value WHERE scope_id = $1",
    "DELETE FROM settings_pref WHERE principal_id IN \
     (SELECT id FROM keel_principal WHERE account_id = $1)",
];

pub(super) async fn forget(call: Call<Id>) -> Result<()> {
    let account = call.params().to_string();
    let tx = call.tx()?;
    let mut removed = 0;
    for sql in STATEMENTS {
        removed += tx
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                [account.as_str().into()],
            ))
            .await?
            .rows_affected();
    }
    if let Some(task) = call.task() {
        task.log(&format!(
            "removed {removed} settings row(s) of account {account}"
        ))
        .await?;
    }
    Ok(())
}
