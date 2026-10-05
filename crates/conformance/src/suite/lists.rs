use keel_types::{ErrorCode, Page};
use serde_json::{Value, json};

use super::{Suite, ids, probes};
use crate::client::{Answer, Credential};
use crate::report::{Aborted, Failure, ensure};

pub const SUFFIXES: [&str; 3] = ["a", "b", "c"];
pub const OVERSIZE_PAGE: u32 = 1_000_000;

pub async fn run(suite: &mut Suite) -> Result<(), Aborted> {
    let outcome = suite.login().await;
    suite
        .report
        .check("auth.login as the admin for the list checks", outcome)?;
    accounts(suite).await?;
    probes(suite).await?;
    let outcome = suite.logout().await;
    suite
        .report
        .check("auth.logout after the list checks", outcome)?;
    Ok(())
}

async fn accounts(suite: &mut Suite) -> Result<(), Aborted> {
    let answer = list(suite, "keel.account.list", json!({ "pageSize": 1 })).await;
    let (first, total) = suite.report.check(
        "keel.account.list with pageSize 1 answers one row and the total beyond the page",
        one_row(answer),
    )?;
    let answer = list(
        suite,
        "keel.account.list",
        json!({ "page": 2, "pageSize": 1 }),
    )
    .await;
    suite.report.check(
        "keel.account.list page 2 of size 1 answers another row with the same total",
        one_row(answer).and_then(|(second, again)| {
            ensure(second != first && again == total, || {
                format!("row {second} total {again} after row {first} total {total}")
            })
        }),
    )?;
    let answer = list(
        suite,
        "keel.account.list",
        json!({ "pageSize": OVERSIZE_PAGE }),
    )
    .await;
    suite.report.check(
        "keel.account.list with an oversize pageSize is clamped and answered, not refused",
        page_of(answer).and_then(|page| {
            ensure(page.data.len() as u64 == page.total, || {
                format!("{} rows of {} total", page.data.len(), page.total)
            })
        }),
    )?;
    let answer = list(
        suite,
        "keel.account.list",
        json!({ "sort": { "key": "state", "dir": "asc" } }),
    )
    .await;
    suite.report.check(
        "keel.account.list sorted by a column not marked sortable answers 422 on sort.key",
        not_sortable(answer, "state"),
    )?;
    let answer = list(
        suite,
        "keel.account.list",
        json!({ "sort": { "key": "kind", "dir": "asc" } }),
    )
    .await;
    suite.report.check(
        "keel.account.list sorted by kind ascending answers the rows in that order",
        page_of(answer).and_then(|page| sorted_by(&page, "kind")),
    )?;
    let answer = list(
        suite,
        "keel.account.list",
        json!({ "filters": [{ "key": "state", "value": "active" }] }),
    )
    .await;
    suite.report.check(
        "keel.account.list filtered by state active answers active rows only",
        page_of(answer).and_then(|page| {
            ensure(
                page.total >= 1 && page.data.iter().all(|row| row["state"] == "active"),
                || format!("{:?}", page.data),
            )
        }),
    )?;
    let answer = list(
        suite,
        "keel.account.list",
        json!({ "filters": [{ "key": "nothing", "value": "x" }] }),
    )
    .await;
    suite.report.check(
        "keel.account.list filtered by an unknown column answers 422 on filters[0].key",
        not_filterable(answer, "nothing"),
    )?;
    let answer = list(suite, "keel.account.list", json!({ "search": "system" })).await;
    suite.report.check(
        "keel.account.list searched by the kind system answers the root account alone",
        page_of(answer).and_then(|page| {
            ensure(
                page.total == 1 && page.data.len() == 1 && page.data[0]["kind"] == "system",
                || format!("{:?}", page.data),
            )
        }),
    )?;
    Ok(())
}

async fn probes(suite: &mut Suite) -> Result<(), Aborted> {
    let prefix = suite.name("list-");
    let names: Vec<String> = SUFFIXES
        .iter()
        .map(|suffix| format!("{prefix}{suffix}"))
        .collect();
    let mut created = Vec::new();
    for name in &names {
        let answer = probes::save(suite, probes::probe(name)).await;
        let id = suite.report.check(
            &format!("webhook.subscription.set creates the probe {name}"),
            answer.and_then(|answer| {
                let answer = answer.confidential();
                let (row, _) = answer.data::<Value>()?;
                ensure(row["name"] == json!(name), || {
                    answer.failure("the saved name differs").0
                })?;
                super::id_of(&row, "id").map_err(|failure| answer.failure(failure.0))
            }),
        )?;
        created.push(id);
    }
    let answer = list(
        suite,
        probes::LIST,
        json!({ "search": prefix, "pageSize": 2 }),
    )
    .await;
    suite.report.check(
        "webhook.subscription.list searched by the prefix with pageSize 2 answers two rows and total 3",
        page_of(answer).and_then(|page| {
            ensure(page.data.len() == 2 && page.total == 3, || {
                format!("{} rows of {} total", page.data.len(), page.total)
            })
        }),
    )?;
    let answer = list(suite, probes::LIST, json!({ "search": names[1] })).await;
    suite.report.check(
        "webhook.subscription.list searched by one name answers that probe alone",
        page_of(answer).and_then(|page| {
            ensure(
                page.total == 1 && page.data.len() == 1 && page.data[0]["name"] == json!(names[1]),
                || format!("{:?}", page.data),
            )
        }),
    )?;
    let answer = list(
        suite,
        probes::LIST,
        json!({ "search": prefix, "sort": { "key": "name", "dir": "desc" } }),
    )
    .await;
    suite.report.check(
        "webhook.subscription.list sorted by name descending answers c, b, a",
        page_of(answer).and_then(|page| {
            let listed: Vec<&str> = page
                .data
                .iter()
                .filter_map(|row| row["name"].as_str())
                .collect();
            let expected: Vec<&str> = names.iter().rev().map(String::as_str).collect();
            ensure(listed == expected, || format!("{listed:?}"))
        }),
    )?;
    let answer = list(
        suite,
        probes::LIST,
        json!({ "sort": { "key": "events", "dir": "asc" } }),
    )
    .await;
    suite.report.check(
        "webhook.subscription.list sorted by a column not marked sortable answers 422 on sort.key",
        not_sortable(answer, "events"),
    )?;
    let answer = list(
        suite,
        probes::LIST,
        json!({ "filters": [{ "key": "name", "value": names[2] }] }),
    )
    .await;
    suite.report.check(
        "webhook.subscription.list filtered by name answers that probe alone",
        page_of(answer).and_then(|page| {
            ensure(
                page.total == 1 && page.data.len() == 1 && page.data[0]["name"] == json!(names[2]),
                || format!("{:?}", page.data),
            )
        }),
    )?;
    let outcome = probes::delete(suite, &created).await;
    suite.report.check(
        "webhook.subscription.delete removes the three probes",
        outcome,
    )?;
    Ok(())
}

async fn list(suite: &mut Suite, func: &str, query: Value) -> Result<Answer, Failure> {
    suite
        .client
        .call(func, query, Credential::Session, &[])
        .await
}

fn page_of(answer: Result<Answer, Failure>) -> Result<Page<Value>, Failure> {
    let answer = answer?;
    let (page, _) = answer.data::<Page<Value>>()?;
    Ok(page)
}

fn one_row(answer: Result<Answer, Failure>) -> Result<(keel_types::Id, u64), Failure> {
    let answer = answer?;
    let (page, _) = answer.data::<Page<Value>>()?;
    let listed = ids(&page).map_err(|failure| answer.failure(failure.0))?;
    match listed.as_slice() {
        [only] if page.total >= 2 => Ok((*only, page.total)),
        _ => Err(answer.failure(format!("{} rows of {} total", listed.len(), page.total))),
    }
}

fn sorted_by(page: &Page<Value>, key: &str) -> Result<(), Failure> {
    let values: Vec<&str> = page
        .data
        .iter()
        .filter_map(|row| row[key].as_str())
        .collect();
    ensure(
        values.len() == page.data.len() && values.windows(2).all(|pair| pair[0] <= pair[1]),
        || format!("{values:?} is not ascending by {key}"),
    )
}

fn not_sortable(answer: Result<Answer, Failure>, field: &str) -> Result<(), Failure> {
    let answer = answer?;
    let error = answer.error(422, ErrorCode::Validation, "keel.error.validation")?;
    let first = error
        .fields
        .first()
        .ok_or_else(|| answer.failure("no fields[]"))?;
    ensure(
        first.path == "sort.key"
            && first.key.as_deref() == Some("keel.error.list.not_sortable")
            && first.params["field"] == field,
        || answer.failure(format!("fields[0] is {first:?}")).0,
    )
}

fn not_filterable(answer: Result<Answer, Failure>, field: &str) -> Result<(), Failure> {
    let answer = answer?;
    let error = answer.error(422, ErrorCode::Validation, "keel.error.validation")?;
    let first = error
        .fields
        .first()
        .ok_or_else(|| answer.failure("no fields[]"))?;
    ensure(
        first.path == "filters[0].key"
            && first.key.as_deref() == Some("keel.error.list.not_filterable")
            && first.params["field"] == field,
        || answer.failure(format!("fields[0] is {first:?}")).0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::fail;

    #[test]
    fn a_page_is_sorted_when_every_neighbour_pair_is_in_order() {
        let page = Page::new(
            vec![json!({ "kind": "reseller" }), json!({ "kind": "system" })],
            2,
        );
        assert!(sorted_by(&page, "kind").is_ok());
        let page = Page::new(
            vec![json!({ "kind": "user" }), json!({ "kind": "system" })],
            2,
        );
        assert!(sorted_by(&page, "kind").is_err());
        let page = Page::new(vec![json!({ "other": 1 })], 1);
        assert_eq!(
            sorted_by(&page, "kind"),
            Err(fail("[] is not ascending by kind"))
        );
    }
}
