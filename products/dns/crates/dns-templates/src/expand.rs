use keel::prelude::*;
use keel::sdk::sea_orm::{
    ColumnTrait, Condition, EntityTrait, QueryFilter, QuerySelect, QueryTrait, Select,
};
use keel_dns_zones::api::{Expansion, RecordSeed, ZoneTemplates};

use crate::{PLATFORM, template};

pub(crate) const UNKNOWN: &str = "dns.template.error.unknown";

pub(crate) struct Expander;

#[async_trait]
impl ZoneTemplates for Expander {
    async fn expand(
        &self,
        db: &Db,
        scope: &AccountScope,
        template: &Id,
        expansion: &Expansion,
    ) -> Result<Vec<RecordSeed>> {
        let row = visible(template::Entity::find_by_id(template.to_string()), scope)
            .one(db)
            .await?
            .ok_or_else(|| {
                KeelError::validation(UNKNOWN)
                    .field("template")
                    .param("template", template.to_string())
            })?;
        Ok(row
            .records
            .0
            .iter()
            .filter_map(|record| record.expand(expansion))
            .collect())
    }

    async fn exists(&self, db: &Db, scope: &AccountScope, template: &Id) -> Result<bool> {
        Ok(
            visible(template::Entity::find_by_id(template.to_string()), scope)
                .one(db)
                .await?
                .is_some(),
        )
    }
}

pub(crate) fn visible(
    select: Select<template::Entity>,
    scope: &AccountScope,
) -> Select<template::Entity> {
    if matches!(scope, AccountScope::All) {
        return select;
    }
    let scoped = scope
        .apply(template::Entity::find())
        .select_only()
        .column(template::Column::Id)
        .into_query();
    select.filter(
        Condition::any()
            .add(template::Column::AccountId.eq(PLATFORM.to_string()))
            .add(template::Column::Id.in_subquery(scoped)),
    )
}
