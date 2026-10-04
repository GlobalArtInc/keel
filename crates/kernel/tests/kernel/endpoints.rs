use keel_kernel::http::HttpOptions;
use keel_sdk::Registrar;
use keel_sdk::prelude::*;
use keel_sdk::system::{Endpoint, EndpointRequest};
use keel_testkit::{Harness, Who};

const ECHO: &str = "/api/v1/x/relay/echo";
const STATUS: &str = "/api/v1/x/relay/status";

static RELAY: ModuleDef = ModuleDef {
    id: "relay",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.contribute(Endpoint::post(
        "echo",
        |request: EndpointRequest| async move {
            Ok(json!({
                "level": request.principal.level,
                "login": request.principal.login,
                "body": String::from_utf8_lossy(&request.body),
                "probe": request.header("X-Probe"),
                "credential": request.header("authorization").or(request.header("cookie")),
                "q": request.param("q").collect::<Vec<_>>(),
                "requestId": request.request_id,
            }))
        },
    ))
    .contribute(Endpoint::get(
        "status",
        |request: EndpointRequest| async move {
            if request.param("fail").next().is_some() {
                return Err(KeelError::conflict("relay.error.refused").param("why", "asked"));
            }
            Ok(json!({ "data": [], "next": 7 }))
        },
    ));
    Ok(())
}

async fn boot() -> Harness {
    Harness::new().module(&RELAY).boot().await
}

#[tokio::test]
async fn an_endpoint_receives_the_raw_body_headers_and_query_and_answers_its_own_json() {
    let harness = boot().await;
    let http = harness.http().await;
    let body = "{\"a\":  1,\n \"b\": [2]}";
    let answer = http
        .request(
            "POST",
            &format!("{ECHO}?q=one&q=two%2Cthree"),
            &[
                ("x-probe", "probed"),
                ("cookie", "theme=dark"),
                ("x-keel-request-id", "req-endpoint-1"),
                ("content-type", "application/json"),
            ],
            Some(body),
        )
        .await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    assert_eq!(answer.header("cache-control"), Some("no-store"));
    assert_eq!(
        answer.json(),
        json!({
            "level": "guest",
            "login": "guest",
            "body": body,
            "probe": "probed",
            "credential": null,
            "q": ["one", "two,three"],
            "requestId": "req-endpoint-1",
        })
    );
    let read = http.get(Who::Guest, STATUS).await;
    assert_eq!(read.status, 200, "{}", read.body);
    assert_eq!(read.json(), json!({ "data": [], "next": 7 }));
    http.stop().await.unwrap();
    harness.finish().await;
}

#[tokio::test]
async fn an_endpoint_answers_its_own_method_alone_and_its_errors_in_the_envelope() {
    let harness = boot().await;
    let http = harness.http().await;
    let wrong = http.get(Who::Guest, ECHO).await;
    assert_eq!(wrong.status, 422, "{}", wrong.body);
    assert_eq!(wrong.header("allow"), Some("POST"));
    assert_eq!(
        wrong.json()["error"]["key"],
        json!("keel.error.method_not_allowed")
    );
    assert_eq!(
        wrong.json()["error"]["params"]["function"],
        json!("relay.echo")
    );
    let wrong = http.post(Who::Guest, STATUS, &json!({})).await;
    assert_eq!(wrong.status, 422, "{}", wrong.body);
    assert_eq!(wrong.header("allow"), Some("GET"));
    let refused = http.get(Who::Guest, &format!("{STATUS}?fail=1")).await;
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert_eq!(refused.json()["error"]["key"], json!("relay.error.refused"));
    assert_eq!(refused.json()["error"]["params"]["why"], json!("asked"));
    assert!(refused.header("x-keel-request-id").is_some());
    let elsewhere = http
        .post(Who::Guest, "/api/v1/x/other/echo", &json!({}))
        .await;
    assert_eq!(elsewhere.status, 404, "{}", elsewhere.body);
    assert_eq!(
        elsewhere.json()["error"]["key"],
        json!("keel.error.route_not_found")
    );
    let unknown = http
        .post(Who::Guest, "/api/v1/x/relay/nothing", &json!({}))
        .await;
    assert_eq!(unknown.status, 404, "{}", unknown.body);
    http.stop().await.unwrap();
    harness.finish().await;
}

#[tokio::test]
async fn an_endpoint_sees_the_authenticated_caller_and_its_body_is_capped() {
    let harness = boot().await;
    let http = harness
        .http_with(HttpOptions {
            body_limit: 32,
            ..HttpOptions::default()
        })
        .await;
    let answer = http.post(Who::Admin, ECHO, &json!({ "x": 1 })).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    assert_eq!(answer.json()["level"], json!("admin"));
    assert_eq!(answer.json()["credential"], json!(null));
    assert_eq!(
        answer.json()["login"],
        json!(harness.principal(Who::Admin).login)
    );
    let large = http
        .post(Who::Guest, ECHO, &json!({ "padding": "x".repeat(64) }))
        .await;
    assert_eq!(large.status, 422, "{}", large.body);
    assert_eq!(
        large.json()["error"]["key"],
        json!("keel.error.body_too_large")
    );
    assert_eq!(large.json()["error"]["params"]["limit"], json!(32));
    let forged = http
        .request(
            "POST",
            ECHO,
            &[("authorization", "Bearer nobody-knows-this")],
            Some("{}"),
        )
        .await;
    assert_eq!(forged.status, 401, "{}", forged.body);
    http.stop().await.unwrap();
    harness.finish().await;
}
