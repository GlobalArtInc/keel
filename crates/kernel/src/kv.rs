use sea_orm::{ConnectionTrait, DbBackend, DbErr, Statement, Value};

pub async fn get<C: ConnectionTrait>(
    db: &C,
    key: &str,
) -> Result<Option<serde_json::Value>, DbErr> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT value FROM keel_kv WHERE key = $1",
            [Value::from(key)],
        ))
        .await?;
    row.map(|row| row.try_get::<serde_json::Value>("", "value"))
        .transpose()
}

pub async fn insert<C: ConnectionTrait>(
    db: &C,
    key: &str,
    value: &serde_json::Value,
) -> Result<(), DbErr> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO keel_kv (key, value) VALUES ($1, $2)",
        [Value::from(key), Value::Json(Some(Box::new(value.clone())))],
    ))
    .await?;
    Ok(())
}

pub async fn set<C: ConnectionTrait>(
    db: &C,
    key: &str,
    value: &serde_json::Value,
) -> Result<(), DbErr> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO keel_kv (key, value) VALUES ($1, $2) \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
        [Value::from(key), Value::Json(Some(Box::new(value.clone())))],
    ))
    .await?;
    Ok(())
}
