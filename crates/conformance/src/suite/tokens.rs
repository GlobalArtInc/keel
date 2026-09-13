use keel_types::{ErrorCode, Page};
use serde_json::{Value, json};

use super::Suite;
use crate::client::{Answer, Credential};
use crate::report::{Aborted, Failure, ensure};

pub const TOKEN_PREFIX: &str = "kat_";
pub const TOKEN_NAME: &str = "conformance";
pub const SCOPED_FUNCTION: &str = "keel.account.list";
pub const OTHER_FUNCTION: &str = "keel.principal.list";

pub async fn run(suite: &mut Suite) -> Result<(), Aborted> {
    let outcome = suite.login().await;
    suite
        .report
        .check("auth.login as the admin for the token checks", outcome)?;
    let answer = suite
        .client
        .call(
            "auth.token.create",
            json!({ "name": TOKEN_NAME, "scope": { "functions": [SCOPED_FUNCTION] } }),
            Credential::Session,
            &[],
        )
        .await;
    let (id, token) = suite.report.check(
        "auth.token.create scoped to keel.account.list answers a kat_ token",
        created(answer.map(Answer::confidential)),
    )?;
    let answer = suite
        .client
        .call(
            SCOPED_FUNCTION,
            json!({}),
            Credential::Bearer(token.clone()),
            &[],
        )
        .await;
    let admin_login = suite.settings.admin_login.clone();
    suite.report.check(
        "keel.account.list with the Bearer token answers 200 as the admin without a cookie",
        admitted(answer, &admin_login),
    )?;
    let answer = suite
        .client
        .call(
            OTHER_FUNCTION,
            json!({}),
            Credential::Bearer(token.clone()),
            &[],
        )
        .await;
    suite.report.check(
        "keel.principal.list with the Bearer token answers 403 forbidden keel.error.token_scope",
        refused(answer),
    )?;
    let answer = suite
        .client
        .call(
            "auth.token.revoke",
            json!({ "id": id }),
            Credential::Session,
            &[],
        )
        .await;
    suite.report.check(
        "auth.token.revoke with the session answers revoked true",
        revoked(answer),
    )?;
    let answer = suite
        .client
        .call(SCOPED_FUNCTION, json!({}), Credential::Bearer(token), &[])
        .await;
    suite.report.check(
        "keel.account.list with the revoked token answers 401 unauthenticated",
        dead(answer),
    )?;
    let outcome = suite.logout().await;
    suite
        .report
        .check("auth.logout after the token checks", outcome)?;
    Ok(())
}

fn created(answer: Result<Answer, Failure>) -> Result<(String, String), Failure> {
    let answer = answer?;
    let (data, _) = answer.data::<Value>()?;
    let token = data["token"]
        .as_str()
        .ok_or_else(|| answer.failure("no data.token"))?;
    ensure(
        token.starts_with(TOKEN_PREFIX) && token.len() > TOKEN_PREFIX.len(),
        || answer.failure("data.token is not a kat_ token").0,
    )?;
    let id = data["id"]
        .as_str()
        .ok_or_else(|| answer.failure("no data.id"))?;
    Ok((id.to_owned(), token.to_owned()))
}

fn admitted(answer: Result<Answer, Failure>, admin: &str) -> Result<(), Failure> {
    let answer = answer?;
    let (page, meta) = answer.data::<Page<Value>>()?;
    ensure(page.total >= 2, || {
        answer.failure(format!("the total is {}", page.total)).0
    })?;
    let principal = meta
        .principal
        .as_ref()
        .ok_or_else(|| answer.failure("no meta.principal"))?;
    ensure(principal.login == admin, || {
        answer
            .failure(format!("meta.principal is {}", principal.login))
            .0
    })?;
    ensure(answer.set_cookie().is_none(), || {
        answer.failure("a Bearer call set a cookie").0
    })?;
    answer.meta_version().map(|_| ())
}

fn refused(answer: Result<Answer, Failure>) -> Result<(), Failure> {
    let answer = answer?;
    let error = answer.error(403, ErrorCode::Forbidden, "keel.error.token_scope")?;
    ensure(error.params["function"] == OTHER_FUNCTION, || {
        answer
            .failure("params.function does not name the function")
            .0
    })
}

fn revoked(answer: Result<Answer, Failure>) -> Result<(), Failure> {
    let answer = answer?;
    let (data, _) = answer.data::<Value>()?;
    ensure(data["revoked"] == json!(true), || {
        answer.failure("data.revoked is not true").0
    })
}

fn dead(answer: Result<Answer, Failure>) -> Result<(), Failure> {
    let answer = answer?;
    answer
        .error(
            401,
            ErrorCode::Unauthenticated,
            "keel.error.unauthenticated",
        )
        .map(|_| ())
}
