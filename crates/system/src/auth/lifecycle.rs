use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ConnectionTrait, DbBackend, Statement};

macro_rules! members {
    () => {
        "(SELECT id FROM keel_principal WHERE account_id = $1)"
    };
}

macro_rules! doomed {
    () => {
        concat!(
            "WITH RECURSIVE doomed(id_hash) AS (",
            "SELECT id_hash FROM auth_session WHERE account_id = $1",
            " OR principal_id IN ",
            members!(),
            " OR actor_principal_id IN ",
            members!(),
            " OR link_principal_id IN ",
            members!(),
            " UNION ",
            "SELECT s.id_hash FROM auth_session s JOIN doomed d ON s.parent_id_hash = d.id_hash",
            ") "
        )
    };
}

const UNPOINT: &str = concat!(
    doomed!(),
    "UPDATE auth_session SET su_id_hash = NULL",
    " WHERE su_id_hash IN (SELECT id_hash FROM doomed)",
    " AND id_hash NOT IN (SELECT id_hash FROM doomed)"
);

const SESSIONS: &str = concat!(
    doomed!(),
    "DELETE FROM auth_session WHERE id_hash IN (SELECT id_hash FROM doomed)"
);

const TOKENS: &str = concat!(
    "DELETE FROM auth_token WHERE account_id = $1",
    " OR principal_id IN ",
    members!(),
    " OR link_principal_id IN ",
    members!()
);

const LOGIN_KEYS: &str = concat!(
    "DELETE FROM auth_login_key WHERE principal_id IN ",
    members!(),
    " OR issued_by IN ",
    members!(),
    " OR link_principal_id IN ",
    members!()
);

const GRANTS: &str = concat!(
    "DELETE FROM auth_grant WHERE account_id = $1",
    " OR principal_id IN ",
    members!()
);

const OF_PRINCIPAL: [&str; 6] = [
    concat!(
        "DELETE FROM auth_login_challenge WHERE principal_id IN ",
        members!()
    ),
    concat!(
        "DELETE FROM auth_mail_token WHERE principal_id IN ",
        members!()
    ),
    concat!(
        "DELETE FROM auth_recovery_code WHERE principal_id IN ",
        members!()
    ),
    concat!("DELETE FROM auth_totp WHERE principal_id IN ", members!()),
    concat!(
        "DELETE FROM auth_password WHERE principal_id IN ",
        members!()
    ),
    concat!(
        "DELETE FROM auth_principal_address WHERE principal_id IN ",
        members!()
    ),
];

pub(super) async fn forget(call: Call<Id>) -> Result<()> {
    let account = call.params().to_string();
    let tx = call.tx()?;
    run(tx, UNPOINT, &account).await?;
    let mut removed = 0;
    for sql in [SESSIONS, TOKENS, LOGIN_KEYS, GRANTS]
        .into_iter()
        .chain(OF_PRINCIPAL)
    {
        removed += run(tx, sql, &account).await?;
    }
    if let Some(task) = call.task() {
        task.log(&format!(
            "removed {removed} credential row(s) of account {account}"
        ))
        .await?;
    }
    Ok(())
}

async fn run(db: &Db, sql: &str, account: &str) -> Result<u64> {
    let done = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            [account.into()],
        ))
        .await?;
    Ok(done.rows_affected())
}
