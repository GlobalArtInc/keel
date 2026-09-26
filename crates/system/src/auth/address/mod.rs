pub mod functions;

use keel_sdk::async_trait::async_trait;
use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::sea_orm::sea_query::{Expr, OnConflict};
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set};
use keel_sdk::system::{Address, AddressSource};
use keel_sdk::{Db, Id, Result};

use super::entity::principal_address;

const KIND_EMAIL: &str = "email";

pub async fn row(db: &Db, principal: Id) -> Result<Option<principal_address::Model>> {
    Ok(
        principal_address::Entity::find_by_id((principal.to_string(), KIND_EMAIL.to_owned()))
            .one(db)
            .await?,
    )
}

pub async fn store(
    db: &Db,
    principal: Id,
    value: &str,
    verified_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<()> {
    principal_address::Entity::insert(principal_address::ActiveModel {
        principal_id: Set(principal.to_string()),
        kind: Set(KIND_EMAIL.to_owned()),
        value: Set(value.to_owned()),
        verified_at: Set(verified_at),
        updated_at: Set(now),
    })
    .on_conflict(
        OnConflict::columns([
            principal_address::Column::PrincipalId,
            principal_address::Column::Kind,
        ])
        .update_columns([
            principal_address::Column::Value,
            principal_address::Column::VerifiedAt,
            principal_address::Column::UpdatedAt,
        ])
        .to_owned(),
    )
    .exec(db)
    .await?;
    Ok(())
}

pub async fn confirm(db: &Db, principal: Id, value: &str, now: DateTime<Utc>) -> Result<bool> {
    let updated = principal_address::Entity::update_many()
        .col_expr(principal_address::Column::VerifiedAt, Expr::value(now))
        .filter(principal_address::Column::PrincipalId.eq(principal.to_string()))
        .filter(principal_address::Column::Kind.eq(KIND_EMAIL))
        .filter(principal_address::Column::Value.eq(value))
        .exec(db)
        .await?;
    Ok(updated.rows_affected > 0)
}

pub struct Addresses;

#[async_trait]
impl AddressSource for Addresses {
    async fn email(&self, principal: Id, db: &Db) -> Result<Option<Address>> {
        Ok(row(db, principal).await?.map(|row| Address {
            value: row.value,
            verified: row.verified_at.is_some(),
        }))
    }
}
