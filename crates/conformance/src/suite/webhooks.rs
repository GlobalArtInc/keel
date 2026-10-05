use std::time::{Duration, Instant};

use keel_types::{Event, Id};
use serde_json::{Value, json};

use super::{Suite, User, probes};
use crate::client::{Answer, Credential};
use crate::listener::{EVENT_ID_HEADER, Listener, Received};
use crate::report::{Aborted, Failure, ensure, fail};
use crate::signature::{self, HEADER as SIGNATURE_HEADER, WINDOW_SECONDS, now_seconds, within};

pub const ADMIN_SECRET: &str = "whsec_conformance_admin_secret_01";
pub const USER_SECRET: &str = "whsec_conformance_user_secret_002";
pub const RETRY_SECRET: &str = "whsec_conformance_retry_secret_03";
pub const DELIVERY_TIMEOUT: Duration = Duration::from_secs(30);
pub const QUIET: Duration = Duration::from_secs(2);
pub const TICK: Duration = Duration::from_millis(1100);
pub const FIRST_RUNG: i64 = 60;
pub const STATE_DELIVERED: &str = "delivered";
pub const STATE_FAILED: &str = "failed";

pub async fn run(suite: &mut Suite) -> Result<(), Aborted> {
    let outcome = suite.login().await;
    suite
        .report
        .check("auth.login as the admin for the webhook checks", outcome)?;
    let listener = suite.report.check(
        "the suite opens a listener for webhook deliveries",
        Listener::start().await,
    )?;
    let answer = subscribe(
        suite,
        "conformance admin",
        &listener.hook_url("admin"),
        ADMIN_SECRET,
        probes::MASK,
    )
    .await;
    let admin_subscription = suite.report.check(
        "webhook.subscription.set as the admin to webhook.subscription.* answers the chosen secret once",
        subscribed(answer, ADMIN_SECRET, probes::MASK),
    )?;
    let name = suite.name("hook-admin");
    let outcome = probes::create(suite, &name).await;
    let admin_probe = suite.report.check(
        "webhook.subscription.set as the admin saves the probe the webhook announces",
        outcome,
    )?;
    let hit = suite.report.check(
        "the listener receives the delivery of webhook.subscription.saved about the probe within the timeout",
        listener
            .wait_for("hook/admin", "the admin's probe", |hit| {
                hit.about(admin_probe)
            })
            .await,
    )?;
    let now = now_seconds();
    let event = suite.report.check(
        "the delivery carries Content-Type application/json and X-Keel-Event-Id equal to the body's id",
        shaped(&hit, probes::SAVED, "webhook"),
    )?;
    let header = hit.header(SIGNATURE_HEADER).unwrap_or_default().to_owned();
    suite.report.check(
        "X-Keel-Signature t=<seconds>,v1=<hex> verifies with HMAC-SHA256 over t.body under the secret",
        signature::verify(ADMIN_SECRET.as_bytes(), &header, &hit.body, now),
    )?;
    let mut tampered = hit.body.clone();
    if let Some(byte) = tampered.last_mut() {
        *byte ^= 0x01;
    }
    suite.report.check(
        "a tampered body fails the verification",
        refused(signature::verify(
            ADMIN_SECRET.as_bytes(),
            &header,
            &tampered,
            now,
        )),
    )?;
    let t = suite.report.check(
        "the signature time is within the window of the suite's clock",
        signature::parse(&header).and_then(|(t, _)| {
            ensure(within(t, now), || format!("t {t} is far from now {now}"))?;
            Ok(t)
        }),
    )?;
    suite.report.check(
        "a stale t outside the 300 second window fails the verification",
        refused(signature::verify(
            ADMIN_SECRET.as_bytes(),
            &header,
            &hit.body,
            t.saturating_add(WINDOW_SECONDS + 1),
        )),
    )?;
    let answer = deliveries(suite, admin_subscription, event.id, STATE_DELIVERED).await;
    suite.report.check(
        "webhook.delivery.list scoped by the subscription shows the delivery as delivered with status 200 at the first attempt",
        delivered(answer, event.id, 1),
    )?;

    let outcome = suite.find_user().await;
    let user = suite.report.check(
        "keel.principal.list finds the user for the webhook checks",
        outcome,
    )?;
    let outcome = suite.su(&user).await;
    suite
        .report
        .check("auth.su into the user for the webhook checks", outcome)?;
    let answer = subscribe(
        suite,
        "conformance user",
        &listener.hook_url("user"),
        USER_SECRET,
        probes::MASK,
    )
    .await;
    let user_subscription = suite.report.check(
        "webhook.subscription.set as the user to webhook.subscription.* answers the chosen secret once",
        subscribed(answer, USER_SECRET, probes::MASK),
    )?;
    let name = suite.name("hook-user");
    let outcome = probes::create(suite, &name).await;
    let user_probe = suite.report.check(
        "webhook.subscription.set as the user saves the user's probe",
        outcome,
    )?;
    let outcome = suite.su_exit().await;
    suite
        .report
        .check("auth.su.exit back to the admin", outcome)?;
    let name = suite.name("hook-admin-2");
    let outcome = probes::create(suite, &name).await;
    let second_admin_probe = suite.report.check(
        "webhook.subscription.set as the admin saves a second probe of the admin",
        outcome,
    )?;
    let hit = suite.report.check(
        "the user's subscription receives the webhook.subscription.saved of the user's probe",
        listener
            .wait_for("hook/user", "the user's probe", |hit| hit.about(user_probe))
            .await,
    )?;
    let header = hit.header(SIGNATURE_HEADER).unwrap_or_default().to_owned();
    suite.report.check(
        "the user's delivery verifies under the user's secret",
        signature::verify(USER_SECRET.as_bytes(), &header, &hit.body, now_seconds()),
    )?;
    let both = match listener
        .wait_for("hook/admin", "the user's probe", |hit| {
            hit.about(user_probe)
        })
        .await
    {
        Ok(_) => listener
            .wait_for("hook/admin", "the admin's second probe", |hit| {
                hit.about(second_admin_probe)
            })
            .await
            .map(|_| ()),
        Err(failure) => Err(failure),
    };
    suite.report.check(
        "the admin's subscription receives the probes of both accounts",
        both,
    )?;
    tokio::time::sleep(QUIET).await;
    suite.report.check(
        "the user's subscription receives events about the user's account alone",
        own_account_only(
            &listener.received("hook/user"),
            &user,
            &[admin_probe, second_admin_probe],
        ),
    )?;

    retries(suite, &listener).await?;

    let outcome = probes::delete(
        suite,
        &[
            admin_probe,
            user_probe,
            second_admin_probe,
            admin_subscription,
            user_subscription,
        ],
    )
    .await;
    suite.report.check(
        "webhook.subscription.delete removes the probes and both subscriptions",
        outcome,
    )?;
    let outcome = suite.logout().await;
    suite
        .report
        .check("auth.logout after the webhook checks", outcome)?;
    Ok(())
}

async fn retries(suite: &mut Suite, listener: &Listener) -> Result<(), Aborted> {
    let answer = subscribe(
        suite,
        "conformance retry",
        &listener.flaky_url("retry"),
        RETRY_SECRET,
        probes::DELETED,
    )
    .await;
    let subscription = suite.report.check(
        "webhook.subscription.set to a listener that fails each first attempt answers the chosen secret once",
        subscribed(answer, RETRY_SECRET, probes::DELETED),
    )?;
    let name = suite.name("hook-retry");
    let outcome = probes::create(suite, &name).await;
    let probe = suite.report.check(
        "webhook.subscription.set saves the probe whose deletion the retried webhook announces",
        outcome,
    )?;
    let outcome = probes::delete(suite, &[probe]).await;
    suite.report.check(
        "webhook.subscription.delete of the probe emits webhook.subscription.deleted",
        outcome,
    )?;
    let first = suite.report.check(
        "the first attempt of the delivery reaches the listener, which answers 500",
        listener
            .wait_for("flaky/retry", "the deleted probe", |hit| {
                hit.about(probe) && hit.status == 500
            })
            .await,
    )?;
    let event = suite.report.check(
        "the failed attempt is signed under the subscription's secret",
        first.event().and_then(|event| {
            let header = first.header(SIGNATURE_HEADER).unwrap_or_default();
            signature::verify(RETRY_SECRET.as_bytes(), header, &first.body, now_seconds())?;
            Ok(event)
        }),
    )?;
    let answer = deliveries(suite, subscription, event.id, STATE_FAILED).await;
    let delivery = suite.report.check(
        "webhook.delivery.list shows the delivery failed at its first attempt with status 500 and the next attempt a minute away",
        failed(answer, event.id),
    )?;
    tokio::time::sleep(TICK).await;
    let answer = suite
        .client
        .call(
            "webhook.delivery.retry",
            json!({ "id": delivery }),
            Credential::Session,
            &[],
        )
        .await;
    suite.report.check(
        "webhook.delivery.retry puts the failed delivery back to pending at once",
        answer.and_then(|answer| {
            let (row, _) = answer.data::<Value>()?;
            ensure(row["state"] == json!("pending"), || {
                answer.failure("the state is not pending").0
            })
        }),
    )?;
    let second = suite.report.check(
        "the second attempt reaches the listener, which admits it",
        listener
            .wait_for("flaky/retry", "the retried delivery", |hit| {
                hit.about(probe) && hit.status == 200
            })
            .await,
    )?;
    suite.report.check(
        "the retry, sent a second after the first attempt, carries the same X-Keel-Event-Id and a signature made at its own later t that verifies",
        resent(&first, &second, &event),
    )?;
    let answer = deliveries(suite, subscription, event.id, STATE_DELIVERED).await;
    suite.report.check(
        "webhook.delivery.list shows the delivery delivered with status 200 at the second attempt",
        delivered(answer, event.id, 2),
    )?;
    let outcome = probes::delete(suite, &[subscription]).await;
    suite.report.check(
        "webhook.subscription.delete removes the retried subscription",
        outcome,
    )?;
    Ok(())
}

async fn subscribe(
    suite: &mut Suite,
    name: &str,
    url: &str,
    secret: &str,
    mask: &str,
) -> Result<Answer, Failure> {
    suite
        .client
        .call(
            probes::SET,
            json!({ "name": name, "url": url, "secret": secret, "events": [mask] }),
            Credential::Session,
            &[],
        )
        .await
        .map(Answer::confidential)
}

async fn deliveries(
    suite: &mut Suite,
    subscription: Id,
    event: Id,
    state: &str,
) -> Result<Answer, Failure> {
    let started = Instant::now();
    loop {
        let answer = suite
            .client
            .call(
                "webhook.delivery.list",
                json!({ "scope": { "subscription_id": subscription } }),
                Credential::Session,
                &[],
            )
            .await?;
        let (page, _) = answer.data::<Value>()?;
        let settled = page["data"].as_array().is_some_and(|rows| {
            rows.iter()
                .any(|row| row["eventId"] == json!(event) && row["state"] == json!(state))
        });
        if settled || started.elapsed() >= DELIVERY_TIMEOUT {
            return Ok(answer);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn refused(outcome: Result<(), Failure>) -> Result<(), Failure> {
    match outcome {
        Ok(()) => Err(fail("the signature verified")),
        Err(_) => Ok(()),
    }
}

fn subscribed(answer: Result<Answer, Failure>, secret: &str, mask: &str) -> Result<Id, Failure> {
    let answer = answer?;
    let (data, _) = answer.data::<Value>()?;
    ensure(data["secret"] == json!(secret), || {
        answer.failure("data.secret is not the chosen secret").0
    })?;
    ensure(data["enabled"] == json!(true), || {
        answer.failure("data.enabled is not true").0
    })?;
    ensure(data["events"] == json!([mask]), || {
        answer.failure("data.events differ from the masks").0
    })?;
    super::id_of(&data, "id").map_err(|failure| answer.failure(failure.0))
}

pub fn shaped(hit: &Received, kind: &str, module: &str) -> Result<Event, Failure> {
    let event = hit.event()?;
    ensure(
        hit.header("content-type")
            .is_some_and(|value| value.starts_with("application/json")),
        || format!("Content-Type is {:?}", hit.header("content-type")),
    )?;
    ensure(
        hit.header(EVENT_ID_HEADER) == Some(event.id.to_string().as_str()),
        || {
            format!(
                "{EVENT_ID_HEADER} is {:?} while the body's id is {}",
                hit.header(EVENT_ID_HEADER),
                event.id
            )
        },
    )?;
    ensure(event.kind == kind, || {
        format!("the event type is {}", event.kind)
    })?;
    ensure(event.module == module, || {
        format!("the event module is {}", event.module)
    })?;
    ensure(event.subject.is_some(), || {
        "the event has no subject".to_owned()
    })?;
    Ok(event)
}

fn row_of<'a>(answer: &'a Answer, page: &'a Value, event: Id) -> Result<&'a Value, Failure> {
    let rows = page["data"]
        .as_array()
        .ok_or_else(|| answer.failure("no data array"))?;
    rows.iter()
        .find(|row| row["eventId"] == json!(event))
        .ok_or_else(|| answer.failure(format!("no delivery row for the event {event}")))
}

fn delivered(answer: Result<Answer, Failure>, event: Id, attempt: u32) -> Result<(), Failure> {
    let answer = answer?;
    let (page, _) = answer.data::<Value>()?;
    let row = row_of(&answer, &page, event)?;
    ensure(row["state"] == json!(STATE_DELIVERED), || {
        answer.failure("the state is not delivered").0
    })?;
    ensure(row["lastStatus"] == json!(200), || {
        answer.failure("lastStatus is not 200").0
    })?;
    ensure(row["attempt"] == json!(attempt), || {
        answer.failure(format!("attempt is not {attempt}")).0
    })
}

fn failed(answer: Result<Answer, Failure>, event: Id) -> Result<Id, Failure> {
    let answer = answer?;
    let (page, _) = answer.data::<Value>()?;
    let row = row_of(&answer, &page, event)?;
    ensure(row["state"] == json!(STATE_FAILED), || {
        answer.failure("the state is not failed").0
    })?;
    ensure(row["lastStatus"] == json!(500), || {
        answer.failure("lastStatus is not 500").0
    })?;
    ensure(row["attempt"] == json!(1), || {
        answer.failure("attempt is not 1").0
    })?;
    let next_at = row["nextAt"]
        .as_str()
        .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
        .ok_or_else(|| answer.failure("nextAt is not a time"))?;
    let away = next_at.timestamp() - now_seconds();
    ensure(away > FIRST_RUNG / 2 && away <= FIRST_RUNG + 5, || {
        answer
            .failure(format!(
                "the next attempt is {away} seconds away, not about {FIRST_RUNG}"
            ))
            .0
    })?;
    super::id_of(row, "id").map_err(|failure| answer.failure(failure.0))
}

fn resent(first: &Received, second: &Received, event: &Event) -> Result<(), Failure> {
    let again = second.event()?;
    ensure(again.id == event.id, || {
        format!(
            "the retry carries the event {} instead of {}",
            again.id, event.id
        )
    })?;
    ensure(
        second.header(EVENT_ID_HEADER) == Some(event.id.to_string().as_str()),
        || {
            format!(
                "{EVENT_ID_HEADER} of the retry is {:?}",
                second.header(EVENT_ID_HEADER)
            )
        },
    )?;
    let header = second.header(SIGNATURE_HEADER).unwrap_or_default();
    signature::verify(RETRY_SECRET.as_bytes(), header, &second.body, now_seconds())?;
    let earlier = first.header(SIGNATURE_HEADER).unwrap_or_default();
    ensure(header != earlier, || {
        "the retry repeats the signature of the first attempt".to_owned()
    })?;
    let (before, _) = signature::parse(earlier)?;
    let (after, _) = signature::parse(header)?;
    ensure(after > before, || {
        format!(
            "the retry is signed at {after}, not after the first attempt at {before}, though it was sent at least {TICK:?} later"
        )
    })
}

pub fn own_account_only(hits: &[Received], user: &User, foreign: &[Id]) -> Result<(), Failure> {
    for hit in hits {
        let event = hit.event()?;
        let subject = event
            .subject
            .as_ref()
            .ok_or_else(|| fail(format!("the event {} has no subject", event.id)))?;
        ensure(!foreign.contains(&subject.id), || {
            format!("the user's hook received the admin's probe {}", subject.id)
        })?;
        ensure(subject.account_id == Some(user.account), || {
            format!(
                "the user's hook received the event {} about the account {:?}",
                event.id, subject.account_id
            )
        })?;
    }
    ensure(!hits.is_empty(), || {
        "the user's hook received nothing".to_owned()
    })
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use super::*;

    fn hit(subject: Id, account: Id) -> Received {
        let event = json!({
            "id": Id::generate(),
            "seq": 1,
            "type": probes::SAVED,
            "at": "2026-10-05T10:00:00.000Z",
            "instanceId": Id::generate(),
            "module": "webhook",
            "subject": { "kind": "webhook.subscription", "id": subject, "accountId": account },
            "data": {}
        });
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        Received {
            hook: "hook/user".into(),
            headers,
            body: serde_json::to_vec(&event).unwrap(),
            status: 200,
        }
    }

    fn attempt(body: &[u8], event: Id, t: i64, secret: &str, status: u16) -> Received {
        let mut headers = HeaderMap::new();
        headers.insert(
            EVENT_ID_HEADER,
            HeaderValue::from_str(&event.to_string()).unwrap(),
        );
        headers.insert(
            SIGNATURE_HEADER,
            HeaderValue::from_str(&signature::sign(secret.as_bytes(), t, body).unwrap()).unwrap(),
        );
        Received {
            hook: "flaky/retry".into(),
            headers,
            body: body.to_vec(),
            status,
        }
    }

    #[test]
    fn a_retry_passes_only_with_the_same_event_and_a_signature_made_after_the_first_attempt() {
        let first = hit(Id::generate(), Id::generate());
        let event = first.event().unwrap();
        let now = now_seconds();
        let failed = attempt(&first.body, event.id, now - 1, RETRY_SECRET, 500);
        let retried = attempt(&first.body, event.id, now, RETRY_SECRET, 200);
        resent(&failed, &retried, &event).unwrap();
        let repeated = attempt(&first.body, event.id, now - 1, RETRY_SECRET, 200);
        assert!(resent(&failed, &repeated, &event).is_err());
        let same_second = attempt(&first.body, event.id, now, RETRY_SECRET, 500);
        assert!(resent(&same_second, &retried, &event).is_err());
        let earlier = attempt(&first.body, event.id, now - 2, RETRY_SECRET, 200);
        assert!(resent(&failed, &earlier, &event).is_err());
        let foreign = attempt(&first.body, event.id, now, USER_SECRET, 200);
        assert!(resent(&failed, &foreign, &event).is_err());
        let relabelled = attempt(&first.body, Id::generate(), now, RETRY_SECRET, 200);
        assert!(resent(&failed, &relabelled, &event).is_err());
        let other = hit(Id::generate(), Id::generate());
        let elsewhere = attempt(&other.body, event.id, now, RETRY_SECRET, 200);
        assert!(resent(&failed, &elsewhere, &event).is_err());
    }

    #[test]
    fn the_user_hook_passes_only_with_events_about_the_users_account_and_none_of_the_admins_probes()
    {
        let user = User {
            id: Id::generate(),
            account: Id::generate(),
        };
        let own = Id::generate();
        let admin = Id::generate();
        assert!(own_account_only(&[hit(own, user.account)], &user, &[admin]).is_ok());
        assert!(own_account_only(&[], &user, &[admin]).is_err());
        assert!(
            own_account_only(
                &[hit(own, user.account), hit(admin, user.account)],
                &user,
                &[admin]
            )
            .is_err()
        );
        assert!(own_account_only(&[hit(own, Id::ROOT_ACCOUNT)], &user, &[admin]).is_err());
    }
}
