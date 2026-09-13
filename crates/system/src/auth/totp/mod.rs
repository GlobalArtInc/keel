pub mod challenge;
pub mod code;
pub mod functions;

use keel_sdk::async_trait::async_trait;
use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::sea_orm::sea_query::{Expr, OnConflict};
use keel_sdk::sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use keel_sdk::system::{SecondFactorSource, SecretCipher};
use keel_sdk::{Db, Id, Result};

use super::entity::{recovery_code, totp};
use super::session;

pub struct Enrolment {
    pub secret: String,
    pub uri: String,
}

pub struct SecondFactor;

#[async_trait]
impl SecondFactorSource for SecondFactor {
    async fn enrolled(&self, principal: Id, db: &Db) -> Result<bool> {
        Ok(confirmed(db, principal).await?.is_some())
    }
}

pub async fn row(db: &Db, principal: Id) -> Result<Option<totp::Model>> {
    let row = totp::Entity::find_by_id(principal.to_string())
        .one(db)
        .await?;
    Ok(row)
}

pub async fn confirmed(db: &Db, principal: Id) -> Result<Option<totp::Model>> {
    let row = totp::Entity::find_by_id(principal.to_string())
        .filter(totp::Column::ConfirmedAt.is_not_null())
        .one(db)
        .await?;
    Ok(row)
}

pub async fn enroll(
    db: &Db,
    cipher: &dyn SecretCipher,
    principal: Id,
    login: &str,
    now: DateTime<Utc>,
) -> Result<Option<Enrolment>> {
    let secret = code::generate_secret()?;
    let sealed = cipher.seal(&secret)?;
    let written = totp::Entity::insert(totp::ActiveModel {
        principal_id: Set(principal.to_string()),
        secret: Set(sealed),
        confirmed_at: Set(None),
        last_step: Set(None),
        created_at: Set(now),
    })
    .on_conflict(
        OnConflict::column(totp::Column::PrincipalId)
            .update_columns([
                totp::Column::Secret,
                totp::Column::ConfirmedAt,
                totp::Column::LastStep,
                totp::Column::CreatedAt,
            ])
            .action_and_where(totp::Column::ConfirmedAt.is_null())
            .to_owned(),
    )
    .exec_without_returning(db)
    .await?;
    if written == 0 {
        return Ok(None);
    }
    let encoded = code::encode_secret(&secret);
    Ok(Some(Enrolment {
        uri: code::uri(login, &encoded),
        secret: encoded,
    }))
}

pub fn open_secret(cipher: &dyn SecretCipher, row: &totp::Model) -> Result<Vec<u8>> {
    cipher.open(&row.secret)
}

pub async fn advance(db: &Db, principal: Id, step: i64) -> Result<bool> {
    let updated = totp::Entity::update_many()
        .col_expr(totp::Column::LastStep, Expr::value(step))
        .filter(totp::Column::PrincipalId.eq(principal.to_string()))
        .filter(
            totp::Column::LastStep
                .is_null()
                .or(totp::Column::LastStep.lt(step)),
        )
        .exec(db)
        .await?;
    Ok(updated.rows_affected > 0)
}

pub async fn confirm(db: &Db, principal: Id, step: i64, now: DateTime<Utc>) -> Result<bool> {
    let updated = totp::Entity::update_many()
        .col_expr(totp::Column::LastStep, Expr::value(step))
        .col_expr(totp::Column::ConfirmedAt, Expr::value(now))
        .filter(totp::Column::PrincipalId.eq(principal.to_string()))
        .filter(totp::Column::ConfirmedAt.is_null())
        .filter(
            totp::Column::LastStep
                .is_null()
                .or(totp::Column::LastStep.lt(step)),
        )
        .exec(db)
        .await?;
    Ok(updated.rows_affected > 0)
}

pub async fn remove(db: &Db, principal: Id) -> Result<bool> {
    recovery_code::Entity::delete_many()
        .filter(recovery_code::Column::PrincipalId.eq(principal.to_string()))
        .exec(db)
        .await?;
    let deleted = totp::Entity::delete_by_id(principal.to_string())
        .exec(db)
        .await?;
    Ok(deleted.rows_affected > 0)
}

pub async fn replace_recovery_codes(db: &Db, principal: Id) -> Result<Vec<String>> {
    recovery_code::Entity::delete_many()
        .filter(recovery_code::Column::PrincipalId.eq(principal.to_string()))
        .exec(db)
        .await?;
    let codes = code::generate_recovery_codes()?;
    for plain in &codes {
        recovery_code::ActiveModel {
            id: Set(Id::generate().to_string()),
            principal_id: Set(principal.to_string()),
            code_hash: Set(session::hash(plain)),
            used_at: Set(None),
        }
        .insert(db)
        .await?;
    }
    Ok(codes)
}

pub async fn redeem_recovery_code(
    db: &Db,
    principal: Id,
    submitted: &str,
    now: DateTime<Utc>,
) -> Result<bool> {
    let code_hash = session::hash(&code::normalise_recovery_code(submitted));
    let updated = recovery_code::Entity::update_many()
        .col_expr(recovery_code::Column::UsedAt, Expr::value(now))
        .filter(recovery_code::Column::PrincipalId.eq(principal.to_string()))
        .filter(recovery_code::Column::CodeHash.eq(code_hash))
        .filter(recovery_code::Column::UsedAt.is_null())
        .exec(db)
        .await?;
    Ok(updated.rows_affected > 0)
}

pub async fn redeem(
    db: &Db,
    cipher: &dyn SecretCipher,
    row: &totp::Model,
    principal: Id,
    submitted: &str,
    now: DateTime<Utc>,
) -> Result<bool> {
    let submitted = submitted.trim();
    if !code::is_code(submitted) {
        return redeem_recovery_code(db, principal, submitted, now).await;
    }
    let secret = open_secret(cipher, row)?;
    match code::matching_step(&secret, submitted, code::step_at(now))? {
        Some(step) => advance(db, principal, step).await,
        None => Ok(false),
    }
}

pub async fn confirm_with(
    db: &Db,
    cipher: &dyn SecretCipher,
    row: &totp::Model,
    principal: Id,
    submitted: &str,
    now: DateTime<Utc>,
) -> Result<bool> {
    let submitted = submitted.trim();
    if !code::is_code(submitted) {
        return Ok(false);
    }
    let secret = open_secret(cipher, row)?;
    match code::matching_step(&secret, submitted, code::step_at(now))? {
        Some(step) => confirm(db, principal, step, now).await,
        None => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use keel_testkit::{Harness, Who};

    use super::*;

    #[keel_testkit::tokio::test]
    async fn the_upsert_itself_refuses_to_replace_a_confirmed_factor_and_leaves_its_row() {
        let harness = Harness::new().module(&crate::auth::MODULE).boot().await;
        let db = Db::from_connection(harness.db().clone());
        let cipher = harness
            .registry()
            .services()
            .get::<dyn SecretCipher>()
            .expect("the kernel provides the cipher");
        let a1 = harness.principal(Who::A1).id;
        let now = harness.clock().now();
        let first = enroll(&db, cipher.as_ref(), a1, Who::A1.login(), now)
            .await
            .expect("enrols")
            .expect("an unconfirmed row is written");
        let replaced = enroll(&db, cipher.as_ref(), a1, Who::A1.login(), now)
            .await
            .expect("enrols again")
            .expect("an unconfirmed row is replaced");
        assert_ne!(first.secret, replaced.secret);
        assert!(
            confirm(&db, a1, code::step_at(now), now)
                .await
                .expect("confirms")
        );
        let before = row(&db, a1).await.expect("reads").expect("a row");
        assert!(before.confirmed_at.is_some());
        let refused = enroll(&db, cipher.as_ref(), a1, Who::A1.login(), now)
            .await
            .expect("runs");
        assert!(refused.is_none());
        let after = row(&db, a1).await.expect("reads").expect("a row");
        assert_eq!(after.secret, before.secret);
        assert_eq!(after.confirmed_at, before.confirmed_at);
        assert_eq!(after.last_step, before.last_step);
        assert_eq!(after.created_at, before.created_at);
        assert_eq!(
            cipher.open(&after.secret).expect("opens"),
            code::decode_secret(&replaced.secret).expect("base32")
        );
        harness.finish().await;
    }
}
