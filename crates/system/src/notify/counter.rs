use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
use keel_sdk::system::ProblemCounter;
use keel_sdk::types::BootProblems;

use super::api::Severity;
use super::entity::problem;

#[derive(Clone, Copy, Debug, Default)]
pub struct Counter;

#[async_trait]
impl ProblemCounter for Counter {
    async fn counts(&self, principal: &Principal, db: &Db) -> Result<BootProblems> {
        let open = AccountScope::of(principal)
            .apply(problem::Entity::find().filter(problem::Column::ClearedAt.is_null()));
        let critical = open
            .clone()
            .filter(problem::Column::Severity.eq(Severity::Critical.as_str()));
        Ok(BootProblems {
            open: open.count(db).await?,
            critical: critical.count(db).await?,
        })
    }
}
