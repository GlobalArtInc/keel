use std::path::{Path, PathBuf};

use keel_sdk::prelude::*;
use keel_system::{auth, settings};
use keel_testkit::{Answer, ErrorCode, Harness, Reply, TestHttp, UploadPart, Who, tokio};

const DEMO_LOGO: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 8 8\"><circle cx=\"4\" cy=\"4\" r=\"3\" fill=\"#1f9d6a\"/></svg>";
const PANEL_LOGO: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 8 8\"><rect width=\"8\" height=\"8\" fill=\"#c41e3a\"/></svg>";
const SCRIPTED_LOGO: &str =
    "<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>";
const HOST: &str = "panel.example";
const LOGO_ROUTE: &str = "/api/v1/x/settings/brand.logo";

static DEMO_BUNDLE: ProductManifest = ProductManifest {
    id: "demo",
    name: "product.demo.brand",
    version: "0.1.0",
    api: keel_sdk::types::DESCRIPTOR_VERSION,
    brand: Brand {
        logo: "product.demo.logo",
        hue: 150,
        chroma: 0.1,
    },
    modules: &[],
    menu: &[],
    levels: LevelLabels {
        labels: &[],
        disabled: &[],
    },
    account_kinds: &["user"],
    editions: &[],
    i18n: &[("en", r#"{ "product": { "demo": { "brand": "Demo" } } }"#)],
    assets: &[Asset {
        id: "product.demo.logo",
        content_type: "image/svg+xml",
        bytes: DEMO_LOGO.as_bytes(),
    }],
};

fn snapshots_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

async fn boot() -> Harness {
    Harness::new()
        .module(&auth::MODULE)
        .module(&settings::MODULE)
        .product(&DEMO_BUNDLE)
        .clock("2026-09-14T10:00:00Z".parse().expect("an instant"))
        .boot()
        .await
}

fn data(reply: &Reply) -> &Value {
    reply
        .data()
        .unwrap_or_else(|| panic!("a result was expected, got {:?}", reply.error()))
}

fn field_error(reply: &Reply, path: &str, key: &str) {
    let error = reply
        .error()
        .unwrap_or_else(|| panic!("an error was expected, got {:?}", reply.envelope));
    assert_eq!(reply.status(), 422, "{error:?}");
    assert_eq!(error.code, ErrorCode::Validation, "{error:?}");
    let field = error
        .fields
        .iter()
        .find(|field| field.path == path)
        .unwrap_or_else(|| panic!("no error on {path}: {error:?}"));
    assert_eq!(field.key.as_deref(), Some(key), "{error:?}");
}

async fn with_host(http: &TestHttp, path: &str, host: &str) -> Answer {
    http.request("GET", path, &[("host", host)], None).await
}

fn brand_of(answer: &Answer) -> Value {
    assert_eq!(answer.status, 200, "{}", answer.body);
    answer.json()["brand"].clone()
}

fn assert_svg(answer: &Answer, body: &str, what: &str) {
    assert_eq!(answer.status, 200, "{what}: {}", answer.body);
    assert_eq!(answer.body, body, "{what}");
    assert_eq!(
        answer.header("content-type"),
        Some("image/svg+xml"),
        "{what}"
    );
    assert_eq!(answer.header("cache-control"), Some("no-store"), "{what}");
}

#[tokio::test]
async fn a_brand_patch_for_a_host_changes_boot_and_the_effective_logo_for_that_host_alone() {
    let harness = boot().await;
    let http = harness.http().await;
    let demo = json!({ "name": "product.demo.brand", "logo": "/brand/logo.svg", "hue": 150, "chroma": 0.1 });
    assert_eq!(
        brand_of(&with_host(&http, "/api/v1/boot", "panel.example:1500").await),
        demo
    );

    let set = harness
        .call(
            Who::Admin,
            "settings.brand.set",
            json!({ "host": "Panel.Example", "name": "Panel Example", "hue": 350, "chroma": 0.2 }),
        )
        .await;
    let row = data(&set);
    assert_eq!(row["host"], json!(HOST));
    assert_eq!(row["name"], json!("Panel Example"));
    assert_eq!(row["logo"], Value::Null);
    assert_eq!(row["hue"], json!(350));
    assert_eq!(row["chroma"], json!(0.2));
    assert_eq!(row["updatedAt"], json!("2026-09-14T10:00:00Z"));
    assert_eq!(row["updatedBy"], json!(harness.principal(Who::Admin).id));
    let uploaded = harness
        .call(
            Who::Admin,
            "settings.brand.logo",
            json!({ "host": "Panel.Example", "logo": PANEL_LOGO }),
        )
        .await;
    assert_eq!(data(&uploaded)["logo"], json!(PANEL_LOGO));
    assert_eq!(data(&uploaded)["name"], json!("Panel Example"));

    let patched = with_host(&http, "/api/v1/boot", "panel.example:1500").await;
    assert_eq!(
        brand_of(&patched),
        json!({ "name": "Panel Example", "logo": "/brand/logo.svg", "hue": 350, "chroma": 0.2 })
    );
    assert_eq!(
        patched.json()["products"][0]["brand"]["name"],
        json!("product.demo.brand")
    );
    assert_eq!(
        brand_of(&with_host(&http, "/api/v1/boot", "127.0.0.1").await),
        demo
    );
    assert_eq!(
        brand_of(&with_host(&http, "/api/v1/boot", "127.0.0.1:1500").await),
        demo
    );
    assert_svg(
        &with_host(&http, "/brand/logo.svg", "panel.example:1500").await,
        PANEL_LOGO,
        "the patched host",
    );
    assert_svg(
        &with_host(&http, "/brand/logo.svg", "127.0.0.1").await,
        DEMO_LOGO,
        "another host",
    );
    assert_svg(
        &with_host(&http, "/brand/demo/logo.svg", "panel.example").await,
        DEMO_LOGO,
        "the product route",
    );

    let partial = harness
        .call(
            Who::Admin,
            "settings.brand.set",
            json!({ "host": "shop.example", "hue": 40 }),
        )
        .await;
    assert_eq!(data(&partial)["name"], Value::Null);
    assert_eq!(data(&partial)["logo"], Value::Null);
    assert_eq!(data(&partial)["chroma"], Value::Null);
    assert_eq!(
        brand_of(&with_host(&http, "/api/v1/boot", "shop.example").await),
        json!({ "name": "product.demo.brand", "logo": "/brand/logo.svg", "hue": 40, "chroma": 0.1 })
    );
    assert_svg(
        &with_host(&http, "/brand/logo.svg", "shop.example").await,
        DEMO_LOGO,
        "a patch without a logo",
    );

    let scripted = harness
        .call(
            Who::Admin,
            "settings.brand.logo",
            json!({ "host": HOST, "logo": SCRIPTED_LOGO }),
        )
        .await;
    field_error(&scripted, "logo", "settings.error.brand_svg");
    let broken = harness
        .call(
            Who::Admin,
            "settings.brand.logo",
            json!({ "host": HOST, "logo": "<svg><circle>" }),
        )
        .await;
    field_error(&broken, "logo", "settings.error.brand_svg");
    let hue = harness
        .call(
            Who::Admin,
            "settings.brand.set",
            json!({ "host": HOST, "hue": 400 }),
        )
        .await;
    field_error(&hue, "hue", "keel.schema.maximum");
    let chroma = harness
        .call(
            Who::Admin,
            "settings.brand.set",
            json!({ "host": HOST, "chroma": 0.5 }),
        )
        .await;
    field_error(&chroma, "chroma", "keel.schema.maximum");
    let host = harness
        .call(
            Who::Admin,
            "settings.brand.set",
            json!({ "host": "panel.example:1500", "hue": 1 }),
        )
        .await;
    field_error(&host, "host", "keel.check.domain");
    assert_eq!(
        brand_of(&with_host(&http, "/api/v1/boot", HOST).await)["hue"],
        json!(350)
    );

    let reseller = harness
        .call(
            Who::Reseller,
            "settings.brand.set",
            json!({ "host": HOST, "hue": 1 }),
        )
        .await;
    assert_eq!(reseller.status(), 403, "{:?}", reseller.error());
    let listed = harness
        .call(Who::Reseller, "settings.brand.list", json!({}))
        .await;
    assert_eq!(listed.status(), 403, "{:?}", listed.error());
    let read = harness
        .call(Who::Reseller, "settings.brand.get", json!({ "host": HOST }))
        .await;
    assert_eq!(read.status(), 403, "{:?}", read.error());
    let removed = harness
        .call(
            Who::Reseller,
            "settings.brand.delete",
            json!({ "host": HOST }),
        )
        .await;
    assert_eq!(removed.status(), 403, "{:?}", removed.error());

    let page = harness
        .call(Who::Admin, "settings.brand.list", json!({}))
        .await;
    let hosts: Vec<&str> = data(&page)["data"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["host"].as_str().expect("a host"))
        .collect();
    assert_eq!(hosts, [HOST, "shop.example"]);
    assert_eq!(data(&page)["total"], json!(2));
    let searched = harness
        .call(
            Who::Admin,
            "settings.brand.list",
            json!({ "search": "Panel Ex" }),
        )
        .await;
    assert_eq!(data(&searched)["total"], json!(1));
    let got = harness
        .call(
            Who::Admin,
            "settings.brand.get",
            json!({ "host": "PANEL.example" }),
        )
        .await;
    assert_eq!(data(&got)["logo"], json!(PANEL_LOGO));
    let missing = harness
        .call(
            Who::Admin,
            "settings.brand.get",
            json!({ "host": "nobody.example" }),
        )
        .await;
    assert_eq!(missing.status(), 404, "{:?}", missing.error());

    let deleted = harness
        .call(Who::Admin, "settings.brand.delete", json!({ "host": HOST }))
        .await;
    assert_eq!(data(&deleted), &json!({ "deleted": true }));
    let again = harness
        .call(Who::Admin, "settings.brand.delete", json!({ "host": HOST }))
        .await;
    assert_eq!(again.status(), 404, "{:?}", again.error());
    assert_eq!(
        brand_of(&with_host(&http, "/api/v1/boot", "panel.example:1500").await),
        demo
    );
    assert_svg(
        &with_host(&http, "/brand/logo.svg", "panel.example:1500").await,
        DEMO_LOGO,
        "after the delete",
    );
    let rows = harness
        .database()
        .rows("SELECT host FROM settings_brand ORDER BY host")
        .await;
    assert_eq!(rows, ["shop.example"]);
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

fn svg_part(bytes: &[u8]) -> UploadPart<'_> {
    UploadPart::new("logo", "logo.svg", "image/svg+xml", bytes)
}

#[tokio::test]
async fn a_plain_svg_logo_is_served_for_its_host_and_an_unsafe_one_is_refused_naming_the_reason() {
    let harness = boot().await;
    let http = harness.http().await;
    let plain = http
        .upload(
            Who::Admin,
            LOGO_ROUTE,
            &json!({ "host": "Panel.Example" }),
            &[svg_part(PANEL_LOGO.as_bytes())],
        )
        .await;
    assert_eq!(plain.status, 200, "{}", plain.body);
    assert_eq!(plain.json()["data"]["host"], json!(HOST));
    assert_svg(
        &with_host(&http, "/brand/logo.svg", "panel.example:1500").await,
        PANEL_LOGO,
        "the uploaded logo",
    );
    let listed = harness
        .call(Who::Admin, "settings.brand.list", json!({}))
        .await;
    assert_eq!(data(&listed)["data"][0]["logo"], json!(true));

    let scripted = http
        .upload(
            Who::Admin,
            LOGO_ROUTE,
            &json!({ "host": HOST }),
            &[svg_part(SCRIPTED_LOGO.as_bytes())],
        )
        .await;
    assert_eq!(scripted.status, 422, "{}", scripted.body);
    let field = &scripted.json()["error"]["fields"][0];
    assert_eq!(field["path"], json!("logo"));
    assert_eq!(field["key"], json!("settings.error.brand_svg"));
    assert_eq!(field["params"]["reason"], json!("a script element"));

    let png = http
        .upload(
            Who::Admin,
            LOGO_ROUTE,
            &json!({ "host": HOST }),
            &[UploadPart::new("logo", "logo.png", "image/png", b"\x89PNG")],
        )
        .await;
    assert_eq!(png.status, 422, "{}", png.body);
    assert_eq!(
        png.json()["error"]["fields"][0]["key"],
        json!("keel.error.file_type")
    );
    assert_svg(
        &with_host(&http, "/brand/logo.svg", "panel.example:1500").await,
        PANEL_LOGO,
        "after the refusals",
    );
    http.stop().await.expect("the server stops");
}

#[tokio::test]
async fn settings_brand_set_no_longer_takes_a_logo_and_leaves_a_stored_one_in_place() {
    let harness = boot().await;
    let uploaded = harness
        .call(
            Who::Admin,
            "settings.brand.logo",
            json!({ "host": HOST, "logo": PANEL_LOGO }),
        )
        .await;
    assert_eq!(data(&uploaded)["logo"], json!(PANEL_LOGO));
    let renamed = harness
        .call(
            Who::Admin,
            "settings.brand.set",
            json!({ "host": HOST, "name": "Panel Example", "hue": 20 }),
        )
        .await;
    assert_eq!(data(&renamed)["name"], json!("Panel Example"));
    assert_eq!(data(&renamed)["logo"], json!(PANEL_LOGO));
    let with_logo = harness
        .call(
            Who::Admin,
            "settings.brand.set",
            json!({ "host": HOST, "hue": 20, "logo": DEMO_LOGO }),
        )
        .await;
    assert_eq!(data(&with_logo)["logo"], json!(PANEL_LOGO));
    let got = harness
        .call(Who::Admin, "settings.brand.get", json!({ "host": HOST }))
        .await;
    assert_eq!(data(&got)["logo"], json!(PANEL_LOGO));
    assert_eq!(data(&got)["hue"], json!(20));
}

#[tokio::test]
async fn the_brand_descriptors_match_their_snapshots() {
    let harness = boot().await;
    for level in Level::ALL {
        let names = harness.snapshot_descriptors(level, snapshots_dir());
        let brand: Vec<&str> = names
            .iter()
            .map(String::as_str)
            .filter(|name| name.starts_with("settings.brand."))
            .collect();
        let expected: &[&str] = if level < Level::Admin {
            &[]
        } else {
            &[
                "settings.brand.delete",
                "settings.brand.get",
                "settings.brand.list",
                "settings.brand.logo",
                "settings.brand.set",
            ]
        };
        assert_eq!(brand, expected, "{level}");
    }
    harness.finish().await;
}

#[tokio::test]
async fn x_forwarded_host_from_a_trusted_proxy_selects_the_brand_patch() {
    let harness = boot().await;
    let behind = harness.http_behind(&["127.0.0.0/8"]).await;
    let direct = harness.http().await;
    let set = harness
        .call(
            Who::Admin,
            "settings.brand.set",
            json!({ "host": HOST, "name": "Panel Example", "hue": 350 }),
        )
        .await;
    assert!(set.data().is_some(), "{:?}", set.error());
    let logo = harness
        .call(
            Who::Admin,
            "settings.brand.logo",
            json!({ "host": HOST, "logo": PANEL_LOGO }),
        )
        .await;
    assert!(logo.data().is_some(), "{:?}", logo.error());
    let upstream = [("host", "upstream.internal:1500")];
    let forwarded = [
        ("host", "upstream.internal:1500"),
        ("x-forwarded-host", "Panel.Example:8443"),
    ];
    let patched = behind
        .request("GET", "/api/v1/boot", &forwarded, None)
        .await;
    assert_eq!(patched.status, 200, "{}", patched.body);
    assert_eq!(patched.json()["brand"]["name"], json!("Panel Example"));
    assert_eq!(patched.json()["brand"]["hue"], json!(350));
    let logo = behind
        .request("GET", "/brand/logo.svg", &forwarded, None)
        .await;
    assert_svg(
        &logo,
        PANEL_LOGO,
        "the forwarded host behind a trusted proxy",
    );

    let plain = behind.request("GET", "/api/v1/boot", &upstream, None).await;
    assert_eq!(plain.status, 200, "{}", plain.body);
    assert_eq!(plain.json()["brand"]["name"], json!("product.demo.brand"));
    let ignored = direct
        .request("GET", "/api/v1/boot", &forwarded, None)
        .await;
    assert_eq!(ignored.status, 200, "{}", ignored.body);
    assert_eq!(
        ignored.json()["brand"]["name"],
        json!("product.demo.brand"),
        "an untrusted peer cannot forward a host"
    );
    let ignored_logo = direct
        .request("GET", "/brand/logo.svg", &forwarded, None)
        .await;
    assert_svg(&ignored_logo, DEMO_LOGO, "the untrusted peer");
    let malformed = behind
        .request(
            "GET",
            "/api/v1/boot",
            &[
                ("host", "upstream.internal:1500"),
                ("x-forwarded-host", "https://panel.example/admin"),
            ],
            None,
        )
        .await;
    assert_eq!(malformed.status, 200, "{}", malformed.body);
    assert_eq!(
        malformed.json()["brand"]["name"],
        json!("product.demo.brand")
    );
    direct.stop().await.expect("the server stops");
    behind.stop().await.expect("the server stops");
    harness.finish().await;
}
