use keel_types::{Boot, Descriptor, ErrorCode};
use serde_json::json;

use super::Suite;
use crate::client::{Answer, Credential};
use crate::report::{Aborted, Failure, ensure};

const WITHHELD_FUNCTION: &str = "note.withheld";
const WITHHELD_FEATURE: &str = "demo.withheld";

pub async fn run(suite: &mut Suite) -> Result<(), Aborted> {
    let outcome = suite.login().await;
    suite
        .report
        .check("auth.login as the admin for the feature checks", outcome)?;
    let answer = suite
        .client
        .call(WITHHELD_FUNCTION, json!({}), Credential::Session, &[])
        .await;
    suite.report.check(
        "note.withheld behind demo.withheld, a feature the fixture leaves out, answers 402 license keel.error.license naming the feature",
        unlicensed(answer),
    )?;
    let answer = suite
        .client
        .get(
            &format!("/api/v1/functions/{WITHHELD_FUNCTION}"),
            Credential::Session,
            &[],
        )
        .await;
    suite.report.check(
        "the descriptor of note.withheld answers 402 license as well",
        unlicensed(answer),
    )?;
    let answer = suite
        .client
        .get("/api/v1/functions", Credential::Session, &[])
        .await;
    suite.report.check(
        "the descriptor listing leaves note.withheld out",
        left_out(answer),
    )?;
    let answer = suite
        .client
        .get("/api/v1/boot", Credential::Session, &[])
        .await;
    suite.report.check(
        "the features of /api/v1/boot leave demo.withheld out",
        not_enabled(answer),
    )?;
    let outcome = suite.logout().await;
    suite
        .report
        .check("auth.logout after the feature checks", outcome)?;
    Ok(())
}

fn unlicensed(answer: Result<Answer, Failure>) -> Result<(), Failure> {
    let answer = answer?;
    let error = answer.error(402, ErrorCode::License, "keel.error.license")?;
    ensure(error.params["feature"] == WITHHELD_FEATURE, || {
        answer.failure("params.feature does not name the feature").0
    })
}

fn left_out(answer: Result<Answer, Failure>) -> Result<(), Failure> {
    let answer = answer?;
    answer.status_is(200)?;
    let descriptors: Vec<Descriptor> = answer.body_as()?;
    ensure(
        !descriptors
            .iter()
            .any(|descriptor| descriptor.name == WITHHELD_FUNCTION),
        || answer.failure("the listing carries note.withheld").0,
    )
}

fn not_enabled(answer: Result<Answer, Failure>) -> Result<(), Failure> {
    let answer = answer?;
    answer.status_is(200)?;
    let boot: Boot = answer.body_as()?;
    ensure(
        !boot
            .features
            .iter()
            .any(|feature| feature == WITHHELD_FEATURE),
        || answer.failure("the features name demo.withheld").0,
    )
}
