use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::Expr;
use keel_sdk::sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use keel_sdk::serde_json;

use super::api::{
    Cleared, EVENT_PROBLEM_CLEARED, EVENT_PROBLEM_RAISED, KEY_PROBLEM_CLEARED, KEY_PROBLEM_RAISED,
    Message, Pair, Problem, Raise, Recipients, SOURCE_PROBLEM, severity_of,
};
use super::entity::problem;
use super::functions::internal::MESSAGE_CREATE;

pub const ERROR_ID: &str = "notify.error.id";
pub const ERROR_ACCOUNT: &str = "notify.error.account";

pub fn problem_of(row: problem::Model) -> Result<Problem> {
    let id: Id = row
        .id
        .parse()
        .map_err(|_| KeelError::internal(ERROR_ID).param("id", row.id.as_str()))?;
    let account = row
        .account_id
        .as_deref()
        .map(|account| {
            account.parse::<Id>().map_err(|_| {
                KeelError::internal(ERROR_ACCOUNT)
                    .param("id", row.id.as_str())
                    .param("account", account)
            })
        })
        .transpose()?;
    Ok(Problem {
        id,
        key: row.key.clone(),
        subject: row.subject,
        severity: severity_of(&row.severity, &row.id)?,
        params: row.params_json,
        account_id: account,
        source: row.source,
        count: u64::try_from(row.count).unwrap_or_default(),
        open: row.open,
        raised_at: row.raised_at,
        updated_at: row.updated_at,
        cleared_at: row.cleared_at,
    })
}

pub(super) async fn open_pair(db: &Db, key: &str, subject: &str) -> Result<Option<problem::Model>> {
    Ok(problem::Entity::find()
        .filter(problem::Column::Key.eq(key))
        .filter(problem::Column::Subject.eq(subject))
        .filter(problem::Column::ClearedAt.is_null())
        .one(db)
        .await?)
}

pub(super) async fn open_of_source(db: &Db, source: &str) -> Result<Vec<problem::Model>> {
    Ok(problem::Entity::find()
        .filter(problem::Column::Source.eq(source))
        .filter(problem::Column::ClearedAt.is_null())
        .order_by_asc(problem::Column::Id)
        .all(db)
        .await?)
}

async fn reload(db: &Db, id: &str) -> Result<Problem> {
    let row = problem::Entity::find_by_id(id.to_owned())
        .one(db)
        .await?
        .ok_or_else(|| KeelError::not_found("keel.error.not_found"))?;
    problem_of(row)
}

fn recipients_of(problem: &Problem) -> Vec<Recipients> {
    let mut recipients = vec![Recipients::Admins];
    if let Some(account) = problem.account_id {
        recipients.push(Recipients::Account(account));
    }
    recipients
}

async fn announce(
    call: &Call<impl Send + Sync>,
    problem: &Problem,
    key: &str,
    level: keel_sdk::types::NotificationLevel,
) -> Result<()> {
    let message = Message {
        recipients: recipients_of(problem),
        key: key.to_owned(),
        params: serde_json::to_value(problem)?,
        level,
        source: SOURCE_PROBLEM.to_owned(),
        problem: Some(problem.id),
    };
    call.invoke(MESSAGE_CREATE, serde_json::to_value(message)?)
        .await?;
    Ok(())
}

pub(super) async fn raise(call: &Call<Raise>) -> Result<Problem> {
    let tx = call.tx()?;
    let now = call.require::<dyn Clock>()?.now();
    let raise = call.params();
    if let Some(open) = open_pair(tx, &raise.key, &raise.subject).await? {
        problem::Entity::update_many()
            .filter(problem::Column::Id.eq(open.id.clone()))
            .col_expr(
                problem::Column::ParamsJson,
                Expr::value(raise.params.clone()),
            )
            .col_expr(
                problem::Column::Count,
                Expr::col(problem::Column::Count).add(1),
            )
            .col_expr(problem::Column::UpdatedAt, Expr::value(now))
            .exec(tx)
            .await?;
        return reload(tx, &open.id).await;
    }
    let inserted = problem::ActiveModel {
        id: Set(Id::generate().to_string()),
        key: Set(raise.key.clone()),
        subject: Set(raise.subject.clone()),
        severity: Set(raise.severity.as_str().to_owned()),
        params_json: Set(raise.params.clone()),
        account_id: Set(raise.account.map(|account| account.to_string())),
        source: Set(raise.source.clone()),
        count: Set(1),
        raised_at: Set(now),
        updated_at: Set(now),
        cleared_at: Set(None),
        open: keel_sdk::sea_orm::ActiveValue::NotSet,
    }
    .insert(tx)
    .await?;
    let problem = problem_of(inserted)?;
    call.emit(EVENT_PROBLEM_RAISED, &problem)?;
    announce(call, &problem, KEY_PROBLEM_RAISED, problem.severity.level()).await?;
    Ok(problem)
}

pub(super) async fn clear(call: &Call<Pair>) -> Result<Cleared> {
    let tx = call.tx()?;
    let now = call.require::<dyn Clock>()?.now();
    let pair = call.params();
    let Some(open) = open_pair(tx, &pair.key, &pair.subject).await? else {
        return Ok(Cleared {
            cleared: false,
            problem: None,
        });
    };
    problem::Entity::update_many()
        .filter(problem::Column::Id.eq(open.id.clone()))
        .col_expr(problem::Column::ClearedAt, Expr::value(Some(now)))
        .col_expr(problem::Column::UpdatedAt, Expr::value(now))
        .exec(tx)
        .await?;
    let problem = reload(tx, &open.id).await?;
    call.emit(EVENT_PROBLEM_CLEARED, &problem)?;
    announce(
        call,
        &problem,
        KEY_PROBLEM_CLEARED,
        keel_sdk::types::NotificationLevel::Info,
    )
    .await?;
    Ok(Cleared {
        cleared: true,
        problem: Some(problem),
    })
}

#[cfg(test)]
mod tests {
    use keel_sdk::chrono::Utc;
    use keel_sdk::serde_json::json;

    use super::*;

    fn row(account: Option<&str>) -> problem::Model {
        problem::Model {
            id: "01J9Q3W6B1R8N5X2K4T7M0V9Z3".into(),
            key: "note.overdue".into(),
            subject: "s".into(),
            severity: "warning".into(),
            params_json: json!({ "count": 2 }),
            account_id: account.map(str::to_owned),
            source: "note.overdue".into(),
            count: 2,
            raised_at: Utc::now(),
            updated_at: Utc::now(),
            cleared_at: None,
            open: true,
        }
    }

    #[test]
    fn a_row_becomes_a_problem_and_its_recipients_are_the_admins_plus_the_account() {
        let problem = problem_of(row(Some("01J8A0000000000000000000A1"))).expect("a valid row");
        assert_eq!(problem.count, 2);
        assert!(problem.open);
        assert_eq!(problem.severity, super::super::api::Severity::Warning);
        assert_eq!(
            recipients_of(&problem),
            [
                Recipients::Admins,
                Recipients::Account("01J8A0000000000000000000A1".parse().expect("an id")),
            ]
        );
        let unscoped = problem_of(row(None)).expect("a valid row");
        assert_eq!(recipients_of(&unscoped), [Recipients::Admins]);
        let mut broken = row(None);
        broken.severity = "loud".into();
        assert_eq!(
            problem_of(broken)
                .expect_err("loud is not a severity")
                .key(),
            super::super::api::ERROR_SEVERITY
        );
        let mut broken = row(Some("nope"));
        broken.id = "01J9Q3W6B1R8N5X2K4T7M0V9Z4".into();
        assert_eq!(
            problem_of(broken).expect_err("nope is not an id").key(),
            ERROR_ACCOUNT
        );
        let mut broken = row(None);
        broken.id = "nope".into();
        assert_eq!(
            problem_of(broken).expect_err("nope is not an id").key(),
            ERROR_ID
        );
    }
}
