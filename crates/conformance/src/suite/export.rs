use keel_types::ErrorCode;
use serde_json::{Value, json};

use super::Suite;
use crate::client::{Answer, Credential};
use crate::report::{Aborted, Failure, ensure};

pub const ROUTE: &str = "/api/v1/x/keel/export";
pub const LIST: &str = "note.list";
pub const NOTE: &str = "conformance-export";
pub const CSV_TYPE: &str = "text/csv; charset=utf-8";
pub const ACCOUNT_COLUMN: &str = "accountId";
pub const ACCOUNT_LABEL: &str = "Account";

pub async fn run(suite: &mut Suite) -> Result<(), Aborted> {
    let outcome = suite.login().await;
    suite
        .report
        .check("auth.login as the admin for the export checks", outcome)?;
    let answer = suite
        .client
        .call(
            "note.edit",
            json!({ "title": NOTE }),
            Credential::Session,
            &[],
        )
        .await;
    let note = suite.report.check(
        "note.edit creates the note the export carries",
        answer.and_then(|answer| {
            let (row, _) = answer.data::<Value>()?;
            super::id_of(&row, "id").map_err(|failure| answer.failure(failure.0))
        }),
    )?;
    let answer = export(suite, json!({ "func": LIST, "format": "csv" })).await;
    let admin_header = suite.report.check(
        "keel.export of note.list as the admin streams a csv named after the list and the day",
        csv(answer).and_then(|(header, rows, disposition)| {
            ensure(
                disposition.starts_with(&format!("attachment; filename=\"{LIST}-"))
                    && disposition.ends_with(".csv\""),
                || format!("Content-Disposition is {disposition}"),
            )?;
            ensure(rows.iter().any(|row| row.contains(NOTE)), || {
                format!("{NOTE} is not among {} rows", rows.len())
            })?;
            Ok(header)
        }),
    )?;
    let _ = suite.report.check(
        "the admin's csv carries the account column",
        ensure(
            admin_header.split(',').any(|label| label == ACCOUNT_LABEL),
            || format!("the header is {admin_header}"),
        ),
    );
    let outcome = suite.find_user().await;
    let user = suite.report.check(
        "keel.principal.list finds the user for the export checks",
        outcome,
    )?;
    let outcome = suite.su(&user).await;
    suite
        .report
        .check("auth.su into the user for the export checks", outcome)?;
    let answer = export(suite, json!({ "func": LIST, "format": "csv" })).await;
    let _ = suite.report.check(
        "keel.export of note.list as the user lacks the accountId column",
        csv(answer).and_then(|(header, _, _)| {
            ensure(
                !header.split(',').any(|label| label == ACCOUNT_LABEL) && !header.is_empty(),
                || format!("the header is {header}"),
            )
        }),
    );
    let answer = export(suite, json!({ "func": LIST, "columns": [ACCOUNT_COLUMN] })).await;
    let _ = suite.report.check(
        "keel.export with columns accountId as the user answers 422 on columns",
        answer.and_then(|answer| {
            let error = answer.error(422, ErrorCode::Validation, "keel.error.validation")?;
            ensure(
                error
                    .fields
                    .first()
                    .is_some_and(|field| field.path == "columns"),
                || answer.failure("no field error on columns").0,
            )
        }),
    );
    let outcome = suite.su_exit().await;
    suite.report.check(
        "auth.su.exit back to the admin after the export checks",
        outcome,
    )?;
    let answer = suite
        .client
        .call(
            "note.delete",
            json!({ "ids": [note] }),
            Credential::Session,
            &[],
        )
        .await;
    let _ = suite.report.check(
        "note.delete removes the export note",
        answer.and_then(|answer| answer.data::<Value>().map(|_| ())),
    );
    let outcome = suite.logout().await;
    suite
        .report
        .check("auth.logout after the export checks", outcome)?;
    Ok(())
}

async fn export(suite: &mut Suite, params: Value) -> Result<Answer, Failure> {
    suite
        .client
        .post(ROUTE, params, Credential::Session, &[])
        .await
}

fn csv(answer: Result<Answer, Failure>) -> Result<(String, Vec<String>, String), Failure> {
    let answer = answer?;
    answer.status_is(200)?;
    ensure(answer.header("content-type") == Some(CSV_TYPE), || {
        answer.failure("Content-Type is not the csv type").0
    })?;
    ensure(answer.header("cache-control") == Some("no-store"), || {
        answer.failure("Cache-Control is not no-store").0
    })?;
    let disposition = answer
        .header("content-disposition")
        .ok_or_else(|| answer.failure("no Content-Disposition"))?
        .to_owned();
    let mut lines = answer
        .body
        .split("\r\n")
        .filter(|line| !line.is_empty())
        .map(str::to_owned);
    let header = lines
        .next()
        .ok_or_else(|| answer.failure("the csv has no header row"))?;
    Ok((header, lines.collect(), disposition))
}
