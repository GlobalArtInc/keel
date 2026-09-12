use chrono::Utc;
use keel_sdk::KeelError;
use keel_sdk::runtime::Emitted;
use keel_types::{EventActor, EventActorKind, Id, Level, Principal};
use sea_orm::{ConnectionTrait, DbBackend, Statement, Value as DbValue};

pub async fn write<C: ConnectionTrait>(
    db: &C,
    principal: &Principal,
    events: Vec<Emitted>,
) -> Result<usize, KeelError> {
    let actor = (principal.level != Level::Guest).then_some(EventActor {
        kind: EventActorKind::Principal,
        id: principal.id,
    });
    let actor_json = actor.map(serde_json::to_value).transpose()?;
    let count = events.len();
    for event in events {
        let now = Utc::now();
        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO keel_outbox (id, type, at, actor_json, subject_json, data_json, created_at) VALUES ($1, $2, $3, $4, NULL, $5, $6)",
            [
                DbValue::from(Id::generate().to_string()),
                DbValue::from(event.kind),
                DbValue::from(now),
                DbValue::Json(actor_json.clone().map(Box::new)),
                DbValue::Json(Some(Box::new(event.data))),
                DbValue::from(now),
            ],
        ))
        .await?;
    }
    Ok(count)
}
