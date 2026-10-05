use keel_client::http::header::ORIGIN;
use keel_types::{ErrorCode, Id};
use serde_json::json;

use super::{Suite, probes};
use crate::client::{Answer, Credential};
use crate::report::{Aborted, Failure, ensure};

pub const UNKNOWN_FUNCTION: &str = "nope.nothing";
pub const ADMIN_FUNCTION: &str = "keel.product.list";
pub const FOREIGN_ORIGIN: &str = "https://evil.example";

pub async fn run(suite: &mut Suite) -> Result<(), Aborted> {
    let answer = suite
        .client
        .call(UNKNOWN_FUNCTION, json!({}), Credential::Guest, &[])
        .await;
    suite.report.check(
        "an unknown function answers 404 not_found keel.error.function_not_found with X-Keel-Meta-Version",
        unknown(answer),
    )?;
    let answer = suite
        .client
        .call("keel.account.list", json!({}), Credential::Guest, &[])
        .await;
    suite.report.check(
        "a guest on a user function answers 401 unauthenticated",
        answer.and_then(|answer| {
            answer
                .error(
                    401,
                    ErrorCode::Unauthenticated,
                    "keel.error.unauthenticated",
                )
                .map(|_| ())
        }),
    )?;
    let outcome = suite.login().await;
    suite
        .report
        .check("auth.login as the admin for the error checks", outcome)?;
    let mut nameless = probes::probe(&suite.name("error-nameless"));
    if let Some(params) = nameless.as_object_mut() {
        params.remove("name");
    }
    let answer = probes::save(suite, nameless).await;
    suite.report.check(
        "webhook.subscription.set without its required name answers 422 validation with fields[] naming name",
        missing_field(answer, "name"),
    )?;
    let outcome = suite.find_user().await;
    let user = suite.report.check(
        "keel.principal.list finds the user for the error checks",
        outcome,
    )?;
    let outcome = suite.su(&user).await;
    suite
        .report
        .check("auth.su into the user for the error checks", outcome)?;
    let answer = suite
        .client
        .call(ADMIN_FUNCTION, json!({}), Credential::Session, &[])
        .await;
    suite.report.check(
        "the user on an admin function answers 403 forbidden keel.error.level",
        answer.and_then(|answer| {
            let error = answer.error(403, ErrorCode::Forbidden, "keel.error.level")?;
            ensure(error.params["level"] == "admin", || {
                answer.failure("params.level is not admin").0
            })
        }),
    )?;
    let mut foreign = probes::probe(&suite.name("error-forbidden"));
    foreign["account"] = json!(Id::ROOT_ACCOUNT);
    let answer = probes::save(suite, foreign).await;
    suite.report.check(
        "the user sending account to webhook.subscription.set answers 403 forbidden_field with fields[].path account",
        forbidden_field(answer, "account"),
    )?;
    let outcome = suite.su_exit().await;
    suite
        .report
        .check("auth.su.exit after the error checks", outcome)?;
    let answer = suite
        .client
        .call(
            "keel.account.list",
            json!({}),
            Credential::Session,
            &[(ORIGIN, FOREIGN_ORIGIN)],
        )
        .await;
    suite.report.check(
        "a cookie request with a foreign Origin answers 403 forbidden keel.error.cross_site",
        answer.and_then(|answer| {
            answer
                .error(403, ErrorCode::Forbidden, "keel.error.cross_site")
                .map(|_| ())
        }),
    )?;
    let outcome = suite.logout().await;
    suite
        .report
        .check("auth.logout after the error checks", outcome)?;
    Ok(())
}

fn unknown(answer: Result<Answer, Failure>) -> Result<(), Failure> {
    let answer = answer?;
    let error = answer.error(404, ErrorCode::NotFound, "keel.error.function_not_found")?;
    ensure(error.params["function"] == UNKNOWN_FUNCTION, || {
        answer
            .failure("params.function does not name the function")
            .0
    })?;
    answer.meta_version().map(|_| ())
}

fn missing_field(answer: Result<Answer, Failure>, field: &str) -> Result<(), Failure> {
    let answer = answer?;
    let error = answer.error(422, ErrorCode::Validation, "keel.error.validation")?;
    ensure(
        error.fields.iter().any(|entry| {
            entry.path == field && entry.key.as_deref() == Some("keel.schema.required")
        }),
        || {
            answer
                .failure(format!(
                    "fields[] does not name {field}: {:?}",
                    error.fields
                ))
                .0
        },
    )
}

fn forbidden_field(answer: Result<Answer, Failure>, field: &str) -> Result<(), Failure> {
    let answer = answer?;
    let error = answer.error(403, ErrorCode::ForbiddenField, "keel.error.forbidden_field")?;
    ensure(error.fields.iter().any(|entry| entry.path == field), || {
        answer
            .failure(format!(
                "fields[] does not name {field}: {:?}",
                error.fields
            ))
            .0
    })
}
