use keel::prelude::*;
use keel::sdk::chrono::{DateTime, Utc};
use keel::sdk::types::ErrorCode;
use keel_testkit::{Harness, Reply, Who, tokio};

use crate::support::{Site, composed, data, panel, rows, run_job};

const LIST: &str = "hosting.service.list";
const POLL_JOB: &str = "notify.problems.poll";
const COLLECT_JOB: &str = "stats.collect";
const OPEN_DOWN: &str = "SELECT subject || ' ' || severity || ' ' || (params_json->>'state') FROM notify_problem WHERE key = 'hosting.service.down' AND open ORDER BY subject";

fn listed(reply: &Reply) -> Vec<String> {
    data(reply)["data"]
        .as_array()
        .expect("the rows")
        .iter()
        .map(|row| {
            format!(
                "{} {} {} {}",
                row["name"].as_str().expect("a name"),
                row["state"].as_str().expect("a state"),
                row["memoryMib"],
                row["since"]
            )
        })
        .collect()
}

async fn list(harness: &Harness) -> Vec<String> {
    listed(&harness.call(Who::Admin, LIST, json!({})).await)
}

fn systemctl(site: &Site) -> Vec<String> {
    site.calls()
        .into_iter()
        .filter(|line| {
            line.starts_with("systemctl ")
                && !line.contains(" show ")
                && !line.contains(" list-unit-files ")
        })
        .collect()
}

#[tokio::test]
async fn a_listed_service_restarts_and_an_unlisted_one_is_refused() {
    let site = Site::new();
    let harness = panel(&site).await;
    let running = |name: &str| format!("{name} active 7 \"2026-09-21T14:13:20Z\"");
    assert_eq!(
        list(&harness).await,
        [
            running("dovecot"),
            running("named"),
            running("nginx"),
            running("php8.3-fpm"),
            running("php8.4-fpm"),
            running("postfix"),
        ]
    );

    let restarted = harness
        .call(
            Who::Admin,
            "hosting.service.restart",
            json!({ "name": "nginx" }),
        )
        .await;
    assert_eq!(data(&restarted), &json!({ "name": "nginx" }));
    let reloaded = harness
        .call(
            Who::Admin,
            "hosting.service.reload",
            json!({ "name": "php8.4-fpm" }),
        )
        .await;
    assert_eq!(data(&reloaded), &json!({ "name": "php8.4-fpm" }));

    let refused = harness
        .call(
            Who::Admin,
            "hosting.service.restart",
            json!({ "name": "sshd" }),
        )
        .await;
    let error = refused.error().expect("an unlisted service is refused");
    assert_eq!(error.code, ErrorCode::Forbidden, "{error:?}");
    assert_eq!(error.key, "host.error.policy");
    for who in [Who::A1, Who::Reseller] {
        let refused = harness
            .call(who, "hosting.service.restart", json!({ "name": "nginx" }))
            .await;
        assert!(refused.error().is_some(), "{who:?} restarted nginx");
        assert!(harness.call(who, LIST, json!({})).await.error().is_some());
    }
    assert_eq!(
        systemctl(&site),
        ["systemctl restart nginx", "systemctl reload php8.4-fpm"]
    );
    harness.finish().await;
}

#[tokio::test]
async fn a_stopped_service_raises_a_problem_the_next_poll_clears() {
    let site = Site::new();
    let harness = panel(&site).await;
    run_job(&harness, POLL_JOB).await;
    assert!(rows(&harness, OPEN_DOWN).await.is_empty());

    let stopped = harness
        .call(
            Who::Admin,
            "hosting.service.stop",
            json!({ "name": "postfix" }),
        )
        .await;
    assert_eq!(data(&stopped), &json!({ "name": "postfix" }));
    assert!(
        list(&harness)
            .await
            .contains(&"postfix inactive null null".to_owned())
    );
    run_job(&harness, POLL_JOB).await;
    assert_eq!(
        rows(&harness, OPEN_DOWN).await,
        ["postfix warning inactive"]
    );

    let started = harness
        .call(
            Who::Admin,
            "hosting.service.start",
            json!({ "name": "postfix" }),
        )
        .await;
    assert_eq!(data(&started), &json!({ "name": "postfix" }));
    run_job(&harness, POLL_JOB).await;
    assert!(rows(&harness, OPEN_DOWN).await.is_empty());
    assert_eq!(
        systemctl(&site),
        ["systemctl stop postfix", "systemctl start postfix"]
    );
    harness.finish().await;
}

#[tokio::test]
async fn the_memory_and_uptime_of_every_running_service_are_sampled_per_service() {
    let site = Site::new();
    site.stop("dovecot");
    let now: DateTime<Utc> = "2026-09-24T00:05:00Z".parse().expect("an instant");
    let harness = composed(&site).clock(now).boot().await;
    run_job(&harness, COLLECT_JOB).await;
    let samples = rows(
        &harness,
        "SELECT DISTINCT key || ' ' || subject || ' ' || (value >= 0)::text FROM stats_sample WHERE key LIKE 'hosting.service.%' ORDER BY 1",
    )
    .await;
    let mut expected = Vec::new();
    for key in ["hosting.service.memory", "hosting.service.uptime"] {
        for name in ["named", "nginx", "php8.3-fpm", "php8.4-fpm", "postfix"] {
            expected.push(format!("{key} {name} true"));
        }
    }
    assert_eq!(samples, expected);
    assert_eq!(
        rows(
            &harness,
            "SELECT DISTINCT key || ' ' || value::text FROM stats_sample WHERE key LIKE 'hosting.service.%' ORDER BY 1"
        )
        .await,
        ["hosting.service.memory 7340032", "hosting.service.uptime 208300"]
    );
    harness.finish().await;
}
