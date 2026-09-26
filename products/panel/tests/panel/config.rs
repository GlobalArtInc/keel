use std::fs;
use std::path::Path;

use keel::prelude::*;
use keel::types::ErrorCode;
use keel_testkit::{Harness, Who, tokio};

use crate::support::{PHP, Site, create, data, eventually, panel, rows, state_becomes};

const SET: &str = "hosting.site.config.set";
const GET: &str = "hosting.site.config.get";
const DOMAIN: &str = "config.example.net";
const SERVER: &str = "client_max_body_size 64m;\r\n\r\nlocation /status {\n    return 204;\n}\n\n";
const POOL: &str = "pm.max_children = 8\n";

async fn served_site(site: &Site) -> (Harness, String) {
    let harness = panel(site).await;
    let alice = create(&harness, Who::A1, "alice").await;
    state_becomes(&harness, &alice, "active").await;
    let reply = harness
        .call(
            Who::A1,
            "hosting.site.create",
            json!({ "user": alice, "domain": DOMAIN, "php": PHP }),
        )
        .await;
    let id = data(&reply)["id"].as_str().expect("an id").to_owned();
    active(&harness, &id).await;
    (harness, id)
}

async fn active(harness: &Harness, id: &str) {
    let sql = format!("SELECT state FROM hosting_site WHERE id = '{id}'");
    eventually(&format!("the site {id} active"), async || {
        rows(harness, &sql).await == ["active"]
    })
    .await;
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[tokio::test]
async fn an_override_is_rendered_into_the_server_block_and_the_pool_and_answered_beside_itself() {
    let site = Site::new();
    let (harness, id) = served_site(&site).await;

    let got = harness.call(Who::Admin, GET, json!({ "id": id })).await;
    let config = data(&got);
    assert_eq!(config["domain"], json!(DOMAIN));
    assert_eq!(config["php"], json!(PHP));
    assert_eq!(config["server"], json!(""));
    assert_eq!(
        config["rendered"]["server"].as_str(),
        Some(read(&site.vhost(DOMAIN)).as_str())
    );
    assert_eq!(
        config["rendered"]["pool"].as_str(),
        Some(read(&site.pool(DOMAIN)).as_str())
    );

    let saved = harness
        .call(
            Who::Admin,
            SET,
            json!({ "id": id, "server": SERVER, "pool": POOL }),
        )
        .await;
    assert_eq!(data(&saved)["server"], json!(SERVER));
    let vhost = read(&site.vhost(DOMAIN));
    assert!(
        vhost.contains(
            "disable_symlinks if_not_owner from=$document_root;\n    client_max_body_size 64m;\n\n    location /status {\n        return 204;\n    }\n\n"
        ),
        "{vhost}"
    );
    let pool = read(&site.pool(DOMAIN));
    assert!(pool.ends_with("/:/tmp/\npm.max_children = 8\n"), "{pool}");
    active(&harness, &id).await;

    let got = harness.call(Who::Admin, GET, json!({ "id": id })).await;
    let config = data(&got);
    assert_eq!(config["server"], json!(SERVER));
    assert_eq!(config["pool"], json!(POOL));
    assert_eq!(config["rendered"]["server"], json!(vhost));
    assert_eq!(config["rendered"]["pool"], json!(pool));

    for who in [Who::A1, Who::Reseller] {
        let refused = harness
            .call(who, SET, json!({ "id": id, "server": "" }))
            .await;
        let error = refused
            .error()
            .expect("the configuration is the operator's");
        assert_eq!(error.code, ErrorCode::Forbidden, "{error:?}");
    }
    let control = harness
        .call(
            Who::Admin,
            SET,
            json!({ "id": id, "server": "gzip on;\u{7}" }),
        )
        .await;
    let error = control.error().expect("a control character");
    assert_eq!(error.fields[0].path, "server", "{error:?}");
    assert_eq!(
        error.fields[0].key.as_deref(),
        Some("hosting.site.config.error.text")
    );
    assert_eq!(read(&site.vhost(DOMAIN)), vhost);
    harness.finish().await;
}

#[tokio::test]
async fn an_override_that_fails_nginx_t_is_refused_with_the_output_on_the_field() {
    let site = Site::new();
    let (harness, id) = served_site(&site).await;
    let kept = harness
        .call(
            Who::Admin,
            SET,
            json!({ "id": id, "server": "client_max_body_size 64m;", "pool": POOL }),
        )
        .await;
    assert!(kept.data().is_some(), "{:?}", kept.error());
    active(&harness, &id).await;
    let vhost = read(&site.vhost(DOMAIN));
    let pool = read(&site.pool(DOMAIN));
    let before = site.calls().len();

    let refused = harness
        .call(
            Who::Admin,
            SET,
            json!({ "id": id, "server": "client_max_body_size 64m;\nkeel_test_refused on;", "pool": POOL }),
        )
        .await;
    let error = refused.error().expect("nginx -t refuses the directive");
    assert_eq!(error.code, ErrorCode::Validation, "{error:?}");
    assert_eq!(error.fields.len(), 1, "{error:?}");
    assert_eq!(error.fields[0].path, "server");
    assert_eq!(
        error.fields[0].key.as_deref(),
        Some("host.error.validate_failed")
    );
    let output = error.fields[0].params["detail"]
        .as_str()
        .expect("the output of the check");
    assert!(
        output.contains("nginx: [emerg] unknown directive \"keel_test_refused\""),
        "{output}"
    );
    assert_eq!(read(&site.vhost(DOMAIN)), vhost);
    assert_eq!(read(&site.pool(DOMAIN)), pool);
    assert_eq!(site.calls()[before..], ["nginx -t"]);

    let got = harness.call(Who::Admin, GET, json!({ "id": id })).await;
    let config = data(&got);
    assert_eq!(config["server"], json!("client_max_body_size 64m;"));
    assert_eq!(config["rendered"]["server"], json!(vhost));
    let state = rows(
        &harness,
        &format!("SELECT state FROM hosting_site WHERE id = '{id}'"),
    )
    .await;
    assert_eq!(state, ["active"]);

    site.break_check(&format!("php-fpm{PHP}"), true);
    let refused = harness
        .call(
            Who::Admin,
            SET,
            json!({ "id": id, "server": "client_max_body_size 64m;", "pool": "pm.max_children = 9" }),
        )
        .await;
    let error = refused.error().expect("php-fpm -t refuses the pool");
    assert_eq!(error.fields[0].path, "pool", "{error:?}");
    assert!(
        error.fields[0].params["detail"].as_str().is_some_and(
            |output| output.contains("[emerg] the configuration of the test is broken")
        ),
        "{error:?}"
    );
    assert_eq!(read(&site.pool(DOMAIN)), pool);
    assert_eq!(read(&site.vhost(DOMAIN)), vhost);
    site.break_check(&format!("php-fpm{PHP}"), false);
    harness.finish().await;
}
