use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::Query;
use keel_sdk::sea_orm::{
    ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, QueryFilter, Statement,
};

use super::entity::{message, problem};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dropped {
    pub messages: u64,
    pub problems: u64,
}

pub async fn drop_account(db: &Db, account: Id) -> Result<Dropped> {
    let account = account.to_string();
    let of_account = Query::select()
        .column(problem::Column::Id)
        .from(problem::Entity)
        .and_where(problem::Column::AccountId.eq(account.clone()))
        .to_owned();
    let messages = message::Entity::delete_many()
        .filter(
            message::Column::AccountId
                .eq(account.clone())
                .or(message::Column::ProblemId.in_subquery(of_account)),
        )
        .exec(db)
        .await?;
    let problems = problem::Entity::delete_many()
        .filter(problem::Column::AccountId.eq(account))
        .exec(db)
        .await?;
    Ok(Dropped {
        messages: messages.rows_affected,
        problems: problems.rows_affected,
    })
}

pub(super) async fn forget(call: Call<Id>) -> Result<()> {
    let account = *call.params();
    let tx = call.tx()?;
    let dropped = drop_account(tx, account).await?;
    let direct = tx
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM notify_direct WHERE principal_id IN \
             (SELECT id FROM keel_principal WHERE account_id = $1)",
            [account.to_string().into()],
        ))
        .await?
        .rows_affected();
    if let Some(task) = call.task() {
        task.log(&format!(
            "removed {} message(s), {} problem(s) and {direct} direct mail(s) of account {account}",
            dropped.messages, dropped.problems
        ))
        .await?;
    }
    Ok(())
}
