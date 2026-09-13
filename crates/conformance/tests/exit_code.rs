use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::process::Command;

async fn canned(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
    let addr = listener.local_addr().expect("the bound address");
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("the canned server");
    });
    (format!("http://{addr}"), server)
}

async fn run_against(url: &str) -> (bool, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_keel-conformance"))
        .args([
            "--url",
            url,
            "--admin-login",
            "root",
            "--admin-password",
            "correct-horse-battery",
            "--user-login",
            "alice",
        ])
        .output()
        .await
        .expect("the suite runs");
    (
        output.status.success(),
        String::from_utf8(output.stdout).expect("utf-8 output"),
    )
}

#[tokio::test]
async fn a_broken_expectation_makes_the_suite_exit_non_zero_with_one_line_per_check() {
    let router = Router::new()
        .route(
            "/healthz",
            get(|| async { Json(json!({ "status": "ok" })) }),
        )
        .route(
            "/readyz",
            get(|| async { Json(json!({ "status": "asleep" })) }),
        );
    let (url, server) = canned(router).await;
    let (green, stdout) = run_against(&url).await;
    server.abort();
    assert!(!green, "{stdout}");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines[0], "ok: GET /healthz answers 200 with status ok",
        "{stdout}"
    );
    assert!(
        lines[1].starts_with(
            "FAILED: GET /readyz answers 200 with status ready: GET /readyz answered 200"
        ),
        "{stdout}"
    );
    assert!(lines.contains(&"aborted: the health checks stop at the first failure"));
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("FAILED: GET /api/v1/boot as a guest")),
        "{stdout}"
    );
    let summary = lines.last().expect("a summary line");
    assert!(
        summary.contains("failed") && !summary.contains(" 0 failed"),
        "{summary}"
    );
    assert!(
        lines.iter().all(|line| line.starts_with("ok: ")
            || line.starts_with("FAILED: ")
            || line.starts_with("aborted: ")
            || line == summary),
        "{stdout}"
    );
}

#[tokio::test]
async fn an_unreachable_server_is_a_failed_check_not_a_crash() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
    let url = format!(
        "http://{}",
        listener.local_addr().expect("the bound address")
    );
    drop(listener);
    let (green, stdout) = run_against(&url).await;
    assert!(!green, "{stdout}");
    assert!(
        stdout.starts_with("FAILED: GET /healthz answers 200 with status ok: the request failed"),
        "{stdout}"
    );
}
