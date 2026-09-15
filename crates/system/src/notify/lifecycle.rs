use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::Query;
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

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

pub async fn on_delete(call: Call<Value>) -> Result<()> {
    let account: Id = call
        .params()
        .get("id")
        .and_then(Value::as_str)
        .and_then(|id| id.parse().ok())
        .ok_or_else(|| KeelError::validation("keel.error.validation").field("id"))?;
    drop_account(call.tx()?, account).await?;
    Ok(())
}
