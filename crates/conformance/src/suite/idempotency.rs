use keel_types::{Descriptor, ErrorCode, Id, Idempotency, Level, Mode, Page};
use serde_json::{Value, json};

use super::{Suite, probes};
use crate::client::{Answer, Credential, IDEMPOTENCY_KEY};
use crate::report::{Aborted, Failure, ensure, fail};

pub const MAX_KEY_LEN: usize = keel_types::IDEMPOTENCY_KEY_MAX_LEN;
pub const MISMATCH: &str = "keel.error.idempotency_mismatch";
pub const MALFORMED: &str = "keel.error.idempotency_key";
pub const READ: &str = "keel.account.list";
pub const ONCE: &str = "secret";
pub const MASK: &str = "***";

pub async fn run(suite: &mut Suite) -> Result<(), Aborted> {
    let outcome = suite.login().await;
    suite.report.check(
        "auth.login as the admin for the idempotency checks",
        outcome,
    )?;
    let key = suite.name("idempotency");
    let name = suite.name("idempotency");
    let body = probes::probe(&name);
    let answer = keyed(suite, probes::SET, body.clone(), &key).await;
    let (first, data) = suite.report.check(
        "webhook.subscription.set with an Idempotency-Key answers the probe without X-Keel-Idempotent-Replay",
        fresh(answer),
    )?;
    let answer = keyed(suite, probes::SET, body.clone(), &key).await;
    suite.report.check(
        "the same key and body again replays the stored answer with X-Keel-Idempotent-Replay: true and its once secret masked",
        replayed(answer, &data),
    )?;
    let other = probes::probe(&suite.name("idempotency-other"));
    let answer = keyed(suite, probes::SET, other, &key).await;
    suite.report.check(
        "the same key with another body answers 409 idempotency_mismatch",
        answer.and_then(|answer| {
            answer
                .error(409, ErrorCode::IdempotencyMismatch, MISMATCH)
                .map(|_| ())
        }),
    )?;
    let answer = suite
        .client
        .call(
            probes::LIST,
            json!({ "search": name }),
            Credential::Session,
            &[],
        )
        .await;
    suite.report.check(
        "webhook.subscription.list finds one probe for the one key",
        answer.and_then(|answer| {
            let (page, _) = answer.data::<Page<Value>>()?;
            ensure(page.total == 1, || {
                answer
                    .failure(format!("{} probes under the key", page.total))
                    .0
            })
        }),
    )?;
    let long = "k".repeat(MAX_KEY_LEN + 1);
    let answer = keyed(suite, probes::SET, body.clone(), &long).await;
    suite.report.check(
        "a key longer than 255 characters answers 422 keel.error.idempotency_key",
        answer.and_then(|answer| {
            let error = answer.error(422, ErrorCode::Validation, MALFORMED)?;
            ensure(error.params["max"] == json!(MAX_KEY_LEN), || {
                answer.failure("params.max is not 255").0
            })
        }),
    )?;
    let read_key = suite.name("idempotency-read");
    let mut replays = Vec::new();
    for _ in 0..2 {
        let answer = keyed(suite, READ, json!({}), &read_key).await;
        replays.push(answer.and_then(|answer| {
            answer.data::<Value>()?;
            Ok(answer.replayed())
        }));
    }
    suite.report.check(
        "a read carrying a key runs every time and is never replayed",
        replays
            .into_iter()
            .collect::<Result<Vec<bool>, Failure>>()
            .and_then(|replays| {
                ensure(replays == [false, false], || {
                    format!("the replays of the two reads are {replays:?}")
                })
            }),
    )?;
    let outcome = suite.find_user().await;
    let user = suite.report.check(
        "keel.principal.list finds the user for the idempotency checks",
        outcome,
    )?;
    let outcome = suite.su(&user).await;
    suite
        .report
        .check("auth.su into the user for the idempotency checks", outcome)?;
    let answer = keyed(suite, probes::SET, body, &key).await;
    let theirs = suite.report.check(
        "the admin's key and body sent by the user run anew for the user, not replayed",
        fresh(answer).and_then(|(id, _)| {
            ensure(id != first, || {
                format!("the user was answered the admin's probe {first}")
            })?;
            Ok(id)
        }),
    )?;
    let outcome = suite.su_exit().await;
    suite.report.check(
        "auth.su.exit back to the admin after the idempotency checks",
        outcome,
    )?;
    let outcome = probes::delete(suite, &[first, theirs]).await;
    suite.report.check(
        "webhook.subscription.delete removes the probes of the idempotency checks",
        outcome,
    )?;
    let answer = suite
        .client
        .get("/api/v1/functions", Credential::Session, &[])
        .await;
    suite.report.check(
        "every function the listing marks idempotency required is a write above guest",
        answer.and_then(|answer| {
            answer.status_is(200)?;
            let descriptors: Vec<Descriptor> = answer.body_as()?;
            let misplaced = misplaced(&descriptors);
            ensure(misplaced.is_empty(), || {
                format!("required on {misplaced:?}")
            })
        }),
    )?;
    let outcome = suite.logout().await;
    suite
        .report
        .check("auth.logout after the idempotency checks", outcome)?;
    Ok(())
}

pub async fn keyed(
    suite: &mut Suite,
    func: &str,
    params: Value,
    key: &str,
) -> Result<Answer, Failure> {
    suite
        .client
        .call(func, params, Credential::Session, &[(IDEMPOTENCY_KEY, key)])
        .await
        .map(Answer::confidential)
}

fn fresh(answer: Result<Answer, Failure>) -> Result<(Id, Value), Failure> {
    let answer = answer?;
    let (data, _) = answer.data::<Value>()?;
    ensure(!answer.replayed(), || {
        answer.failure("a first call was marked replayed").0
    })?;
    let id = super::id_of(&data, "id").map_err(|failure| answer.failure(failure.0))?;
    Ok((id, data))
}

fn replayed(answer: Result<Answer, Failure>, first: &Value) -> Result<(), Failure> {
    let answer = answer?;
    let (data, _) = answer.data::<Value>()?;
    ensure(answer.replayed(), || {
        answer
            .failure("the repeat lacks X-Keel-Idempotent-Replay: true")
            .0
    })?;
    once_masked(first, &data, &[ONCE]).map_err(|failure| answer.failure(failure.0))
}

pub fn once_masked(first: &Value, replay: &Value, once: &[&str]) -> Result<(), Failure> {
    let (Some(first), Some(replay)) = (first.as_object(), replay.as_object()) else {
        return Err(fail("the answers are not objects"));
    };
    ensure(first.len() == replay.len(), || {
        "the replay carries other members than the first answer".to_owned()
    })?;
    for (name, value) in first {
        let again = replay.get(name);
        if once.contains(&name.as_str()) {
            ensure(value.is_string() && again == Some(&json!(MASK)), || {
                format!("the once member {name} is not masked in the replay")
            })?;
        } else {
            ensure(again == Some(value), || {
                format!("the member {name} differs in the replay")
            })?;
        }
    }
    Ok(())
}

pub fn misplaced(descriptors: &[Descriptor]) -> Vec<String> {
    descriptors
        .iter()
        .filter(|descriptor| descriptor.idempotency == Idempotency::Required)
        .filter(|descriptor| {
            descriptor.mode != Mode::Write || descriptor.access.level == Level::Guest
        })
        .map(|descriptor| descriptor.name.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replay_equals_the_first_answer_but_for_its_once_members_masked() {
        let first = json!({ "id": "01J9", "name": "p", "secret": "s3cret" });
        once_masked(
            &first,
            &json!({ "id": "01J9", "name": "p", "secret": MASK }),
            &[ONCE],
        )
        .unwrap();
        for replay in [
            json!({ "id": "01J9", "name": "p", "secret": "s3cret" }),
            json!({ "id": "01J9", "name": "q", "secret": MASK }),
            json!({ "id": "01J9", "name": "p" }),
            json!({ "id": "01J9", "name": "p", "secret": MASK, "extra": 1 }),
            json!([]),
        ] {
            assert!(once_masked(&first, &replay, &[ONCE]).is_err(), "{replay}");
        }
    }

    #[test]
    fn the_key_may_be_required_only_of_a_write_above_guest() {
        let descriptor = |name: &str, mode: &str, level: &str, idempotency: &str| -> Descriptor {
            serde_json::from_value(json!({
                "keel": 1,
                "name": name,
                "module": "x",
                "kind": "action",
                "mode": mode,
                "access": { "level": level },
                "idempotency": idempotency,
                "params": {},
                "result": {}
            }))
            .unwrap()
        };
        let listing = [
            descriptor("x.open", "write", "admin", "required"),
            descriptor("x.list", "read", "user", "optional"),
            descriptor("x.peek", "read", "admin", "required"),
            descriptor("x.join", "write", "guest", "required"),
        ];
        assert_eq!(misplaced(&listing), ["x.peek", "x.join"]);
    }
}
