use serde_json::json;

use super::Suite;
use crate::client::{Answer, Credential};
use crate::report::{Aborted, Failure, ensure};

pub async fn run(suite: &mut Suite) -> Result<(), Aborted> {
    let answer = suite.client.get("/healthz", Credential::Guest, &[]).await;
    suite.report.check(
        "GET /healthz answers 200 with status ok",
        status_body(answer, "ok"),
    )?;
    let answer = suite.client.get("/readyz", Credential::Guest, &[]).await;
    suite.report.check(
        "GET /readyz answers 200 with status ready",
        status_body(answer, "ready"),
    )?;
    Ok(())
}

fn status_body(answer: Result<Answer, Failure>, status: &str) -> Result<(), Failure> {
    let answer = answer?;
    answer.status_is(200)?;
    let body = answer.json()?;
    ensure(body == json!({ "status": status }), || {
        answer
            .failure(format!("the body is not {{\"status\":\"{status}\"}}"))
            .0
    })
}
