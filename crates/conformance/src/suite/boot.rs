use keel_types::{Boot, PROTOCOL};
use reqwest::header::IF_NONE_MATCH;

use super::Suite;
use crate::client::{API_PREFIX, Answer, Credential};
use crate::report::{Aborted, Failure, ensure};

pub async fn run(suite: &mut Suite) -> Result<(), Aborted> {
    let answer = suite
        .client
        .get(&format!("{API_PREFIX}/boot"), Credential::Guest, &[])
        .await;
    let (answer, boot) = suite.report.check(
        "GET /api/v1/boot as a guest answers 200 with principal null",
        guest_boot(answer),
    )?;
    suite.report.check(
        "the guest boot names api 1 and carries the platform group at most",
        platform_only(&answer, &boot),
    )?;
    suite.report.check(
        "boot.meta equals the ETag and the response is private, no-store",
        cached(&answer, &boot),
    )?;
    let etag = format!("\"{}\"", boot.meta);
    let again = suite
        .client
        .get(
            &format!("{API_PREFIX}/boot"),
            Credential::Guest,
            &[(IF_NONE_MATCH, etag.as_str())],
        )
        .await;
    suite.report.check(
        "GET /api/v1/boot with If-None-Match naming the meta answers 304",
        not_modified(again, &boot),
    )?;
    Ok(())
}

fn guest_boot(answer: Result<Answer, Failure>) -> Result<(Answer, Boot), Failure> {
    let answer = answer?;
    answer.status_is(200)?;
    let boot: Boot = answer.body_as()?;
    ensure(boot.principal.is_none(), || {
        answer.failure("principal is not null").0
    })?;
    Ok((answer, boot))
}

fn platform_only(answer: &Answer, boot: &Boot) -> Result<(), Failure> {
    ensure(boot.keel.api == PROTOCOL, || {
        answer.failure(format!("keel.api is {}", boot.keel.api)).0
    })?;
    ensure(!boot.keel.version.is_empty(), || {
        answer.failure("keel.version is empty").0
    })?;
    let products: Vec<&str> = boot
        .menu
        .iter()
        .filter_map(|group| group.product.as_deref())
        .collect();
    ensure(products.is_empty(), || {
        answer
            .failure(format!("a guest sees the product groups {products:?}"))
            .0
    })
}

fn cached(answer: &Answer, boot: &Boot) -> Result<(), Failure> {
    ensure(boot.meta.starts_with("m-"), || {
        answer.failure(format!("meta is {}", boot.meta)).0
    })?;
    ensure(answer.etag()? == boot.meta, || {
        answer.failure("the ETag differs from meta").0
    })?;
    answer.is_private()
}

fn not_modified(answer: Result<Answer, Failure>, boot: &Boot) -> Result<(), Failure> {
    let answer = answer?;
    answer.status_is(304)?;
    ensure(answer.etag()? == boot.meta, || {
        answer.failure("the ETag differs from meta").0
    })?;
    ensure(answer.body.is_empty(), || {
        answer.failure("a 304 with a body").0
    })
}
