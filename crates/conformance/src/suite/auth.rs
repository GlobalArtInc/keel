use keel_types::{ActorVia, Boot, Page};
use serde_json::{Value, json};

use super::{Suite, User, ids};
use crate::client::{API_PREFIX, Answer, Credential};
use crate::report::{Aborted, Failure, ensure};

pub async fn run(suite: &mut Suite) -> Result<(), Aborted> {
    let outcome = suite.login().await;
    suite.report.check(
        "auth.login as the admin answers state authenticated and sets the session cookie",
        outcome,
    )?;
    let answer = suite
        .client
        .get(&format!("{API_PREFIX}/boot"), Credential::Session, &[])
        .await;
    let admin_login = suite.settings.admin_login.clone();
    let wide_meta = suite.report.check(
        "GET /api/v1/boot with the session names the admin without an actor",
        booted(answer, &admin_login, None),
    )?;
    let answer = suite
        .client
        .call("keel.account.list", json!({}), Credential::Session, &[])
        .await;
    let wide = suite.report.check(
        "keel.account.list with the session answers every account and X-Keel-Meta-Version equal to boot.meta",
        wide_list(answer, &wide_meta, &admin_login),
    )?;
    let outcome = suite.find_user().await;
    let user = suite.report.check(
        "keel.principal.list finds the user in another account",
        outcome,
    )?;
    let outcome = suite.su(&user).await;
    suite.report.check(
        "auth.su into the user answers the user with the admin as actor via su and sets no cookie",
        outcome,
    )?;
    let answer = suite
        .client
        .call("keel.account.list", json!({}), Credential::Session, &[])
        .await;
    let user_login = suite.settings.user_login.clone();
    let narrow_meta = suite.report.check(
        "keel.account.list under su is narrowed to the user's account and meta.principal.actor.via is su",
        narrowed(answer, &user, wide, &user_login, &admin_login, &wide_meta),
    )?;
    let answer = suite
        .client
        .get(&format!("{API_PREFIX}/boot"), Credential::Session, &[])
        .await;
    let su_meta = suite.report.check(
        "GET /api/v1/boot under su names the user with the actor and another meta",
        booted(answer, &user_login, Some(&admin_login)),
    )?;
    suite.report.check(
        "the meta under su is the one the narrowed call carried",
        ensure(su_meta == narrow_meta, || {
            format!("boot.meta {su_meta} differs from X-Keel-Meta-Version {narrow_meta}")
        }),
    )?;
    let outcome = suite.su_exit().await;
    suite
        .report
        .check("auth.su.exit answers the admin without an actor", outcome)?;
    let answer = suite
        .client
        .call("keel.account.list", json!({}), Credential::Session, &[])
        .await;
    suite.report.check(
        "keel.account.list after su.exit is wide again",
        wide_list(answer, &wide_meta, &admin_login).and_then(|total| {
            ensure(total == wide, || {
                format!("the total is {total} instead of {wide}")
            })
        }),
    )?;
    let cookie = suite.client.session().map(str::to_owned);
    let outcome = suite.logout().await;
    suite
        .report
        .check("auth.logout clears the cookie", outcome)?;
    let cookie = cookie.unwrap_or_default();
    let answer = suite
        .client
        .call(
            "keel.account.list",
            json!({}),
            Credential::Cookie(cookie),
            &[],
        )
        .await;
    suite.report.check(
        "keel.account.list with the revoked cookie answers 401 unauthenticated",
        answer.and_then(|answer| {
            answer
                .error(
                    401,
                    keel_types::ErrorCode::Unauthenticated,
                    "keel.error.unauthenticated",
                )
                .map(|_| ())
        }),
    )?;
    Ok(())
}

fn booted(
    answer: Result<Answer, Failure>,
    login: &str,
    actor: Option<&str>,
) -> Result<String, Failure> {
    let answer = answer?;
    answer.status_is(200)?;
    let boot: Boot = answer.body_as()?;
    let principal = boot
        .principal
        .as_ref()
        .ok_or_else(|| answer.failure("principal is null"))?;
    ensure(principal.login == login, || {
        answer
            .failure(format!("the principal is {}", principal.login))
            .0
    })?;
    match (actor, &principal.actor) {
        (None, None) => {}
        (Some(expected), Some(actual)) => {
            ensure(
                actual.login == expected && actual.via == ActorVia::Su,
                || {
                    answer
                        .failure(format!(
                            "the actor is {} via {:?}",
                            actual.login, actual.via
                        ))
                        .0
                },
            )?;
        }
        (None, Some(actual)) => {
            return Err(answer.failure(format!("an unexpected actor {}", actual.login)));
        }
        (Some(_), None) => return Err(answer.failure("no actor")),
    }
    ensure(answer.etag()? == boot.meta, || {
        answer.failure("the ETag differs from meta").0
    })?;
    Ok(boot.meta)
}

fn wide_list(answer: Result<Answer, Failure>, meta: &str, admin: &str) -> Result<u64, Failure> {
    let answer = answer?;
    let (page, envelope) = answer.data::<Page<Value>>()?;
    ensure(page.total >= 2 && !page.data.is_empty(), || {
        answer
            .failure(format!(
                "{} rows of {} total; the admin should see at least two accounts",
                page.data.len(),
                page.total
            ))
            .0
    })?;
    let principal = envelope
        .principal
        .as_ref()
        .ok_or_else(|| answer.failure("no meta.principal"))?;
    ensure(
        principal.login == admin && principal.actor.is_none(),
        || {
            answer
                .failure("meta.principal is not the admin without an actor")
                .0
        },
    )?;
    ensure(answer.meta_version()? == meta, || {
        answer
            .failure("X-Keel-Meta-Version differs from boot.meta")
            .0
    })?;
    Ok(page.total)
}

fn narrowed(
    answer: Result<Answer, Failure>,
    user: &User,
    wide: u64,
    user_login: &str,
    admin_login: &str,
    wide_meta: &str,
) -> Result<String, Failure> {
    let answer = answer?;
    let (page, envelope) = answer.data::<Page<Value>>()?;
    let listed = ids(&page).map_err(|failure| answer.failure(failure.0))?;
    ensure(
        listed == [user.account] && page.total == 1 && page.total < wide,
        || {
            answer
                .failure(format!("the rows are {listed:?} with total {}", page.total))
                .0
        },
    )?;
    let principal = envelope
        .principal
        .as_ref()
        .ok_or_else(|| answer.failure("no meta.principal"))?;
    let actor = principal
        .actor
        .as_ref()
        .ok_or_else(|| answer.failure("meta.principal.actor is null"))?;
    ensure(
        principal.login == user_login && actor.login == admin_login && actor.via == ActorVia::Su,
        || {
            answer
                .failure("meta.principal is not the user with the admin via su")
                .0
        },
    )?;
    let meta = answer.meta_version()?;
    ensure(meta != wide_meta, || {
        answer
            .failure("X-Keel-Meta-Version did not change under su")
            .0
    })?;
    Ok(meta.to_owned())
}
