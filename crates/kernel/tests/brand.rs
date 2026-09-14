use std::sync::Arc;

use keel_sdk::Registrar;
use keel_sdk::prelude::*;
use keel_sdk::system::{ManifestPatch, ManifestSource};
use keel_testkit::{Answer, Harness, TestHttp, Who, tokio};

const DEMO_LOGO: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 8 8\"><circle cx=\"4\" cy=\"4\" r=\"3\" fill=\"#1f9d6a\"/></svg>";
const HOST_LOGO: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 8 8\"><rect width=\"8\" height=\"8\" fill=\"#c41e3a\"/></svg>";
const KEEL_MARK: &str = include_str!("../assets/keel.svg");
const WHITE_LABEL_HOST: &str = "panel.example";

static DEMO_BUNDLE: ProductManifest = ProductManifest {
    id: "demo",
    name: "product.demo.brand",
    version: "0.1.0",
    api: keel_types::DESCRIPTOR_VERSION,
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

static BARE_BUNDLE: ProductManifest = ProductManifest {
    id: "bare",
    name: "product.bare.brand",
    version: "0.1.0",
    api: keel_types::DESCRIPTOR_VERSION,
    brand: Brand {
        logo: "product.bare.logo",
        hue: 30,
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
    i18n: &[("en", r#"{ "product": { "bare": { "brand": "Bare" } } }"#)],
    assets: &[],
};

struct HostPatches;

#[async_trait]
impl ManifestSource for HostPatches {
    async fn patch(&self, host: &str, _: &Db) -> Result<Option<ManifestPatch>> {
        Ok((host == WHITE_LABEL_HOST).then(|| ManifestPatch {
            name: Some("Panel Example".into()),
            logo: Some(HOST_LOGO.into()),
            hue: Some(350),
            chroma: None,
        }))
    }
}

static PATCHES: ModuleDef = ModuleDef {
    id: "patches",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: patches_init,
};

fn patches_init(r: &mut Registrar) -> Result<()> {
    r.provide::<dyn ManifestSource>(Arc::new(HostPatches));
    Ok(())
}

async fn svg(http: &TestHttp, path: &str, host: &str) -> Answer {
    http.request("GET", path, &[("host", host)], None).await
}

fn assert_svg(answer: &Answer, body: &str, path: &str) {
    assert_eq!(answer.status, 200, "{path}: {}", answer.body);
    assert_eq!(answer.body, body, "{path}");
    assert_eq!(
        answer.header("content-type"),
        Some("image/svg+xml"),
        "{path}"
    );
    assert_eq!(answer.header("cache-control"), Some("no-store"), "{path}");
    assert_eq!(
        answer.header("x-content-type-options"),
        Some("nosniff"),
        "{path}"
    );
    assert!(answer.header("x-keel-request-id").is_some(), "{path}");
}

fn assert_not_found(answer: &Answer, path: &str) {
    assert_eq!(answer.status, 404, "{path}: {}", answer.body);
    let body = answer.json();
    assert_eq!(body["ok"], json!(false), "{path}");
    assert_eq!(
        body["error"]["key"],
        json!("keel.error.not_found"),
        "{path}"
    );
}

#[tokio::test]
async fn a_product_logo_is_served_from_its_asset_and_the_effective_one_follows_the_brand_rule() {
    let harness = Harness::new()
        .product(&DEMO_BUNDLE)
        .product(&BARE_BUNDLE)
        .boot()
        .await;
    let http = harness.http().await;
    let demo = svg(&http, "/brand/demo/logo.svg", "keel.test").await;
    assert_svg(&demo, DEMO_LOGO, "/brand/demo/logo.svg");
    assert_not_found(
        &svg(&http, "/brand/bare/logo.svg", "keel.test").await,
        "/brand/bare/logo.svg",
    );
    assert_not_found(
        &svg(&http, "/brand/nothing/logo.svg", "keel.test").await,
        "/brand/nothing/logo.svg",
    );
    let both = svg(&http, "/brand/logo.svg", "keel.test").await;
    assert_svg(&both, KEEL_MARK, "/brand/logo.svg with two products");
    let boot = http.get(Who::Guest, "/api/v1/boot").await;
    assert_eq!(boot.json()["brand"]["name"], json!("keel.brand"));
    assert!(boot.json()["brand"].get("logo").is_none(), "{}", boot.body);
    http.stop().await.expect("the server stops");
    harness.finish().await;

    let single = Harness::new().product(&DEMO_BUNDLE).boot().await;
    let http = single.http().await;
    let effective = svg(&http, "/brand/logo.svg", "keel.test").await;
    assert_svg(&effective, DEMO_LOGO, "/brand/logo.svg with one product");
    let boot = http.get(Who::Guest, "/api/v1/boot").await;
    assert_eq!(boot.json()["brand"]["logo"], json!("/brand/logo.svg"));
    http.stop().await.expect("the server stops");
    single.finish().await;

    let none = Harness::new().boot().await;
    let http = none.http().await;
    let mark = svg(&http, "/brand/logo.svg", "keel.test").await;
    assert_svg(&mark, KEEL_MARK, "/brand/logo.svg without a product");
    assert!(mark.body.contains("<svg"), "{}", mark.body);
    assert!(!mark.body.contains("<script"), "{}", mark.body);
    assert_not_found(
        &svg(&http, "/brand/demo/logo.svg", "keel.test").await,
        "/brand/demo/logo.svg without the product",
    );
    http.stop().await.expect("the server stops");
    none.finish().await;

    let asset_less = Harness::new().product(&BARE_BUNDLE).boot().await;
    let http = asset_less.http().await;
    assert_not_found(
        &svg(&http, "/brand/logo.svg", "keel.test").await,
        "/brand/logo.svg with a product without an asset",
    );
    http.stop().await.expect("the server stops");
    asset_less.finish().await;
}

#[tokio::test]
async fn a_manifest_patch_for_the_request_host_changes_boot_and_the_effective_logo() {
    let harness = Harness::new()
        .module(&PATCHES)
        .product(&DEMO_BUNDLE)
        .boot()
        .await;
    let http = harness.http().await;
    let patched = http
        .request(
            "GET",
            "/api/v1/boot",
            &[("host", "Panel.Example:1500")],
            None,
        )
        .await;
    assert_eq!(patched.status, 200, "{}", patched.body);
    assert_eq!(
        patched.json()["brand"],
        json!({ "name": "Panel Example", "logo": "/brand/logo.svg", "hue": 350, "chroma": 0.1 })
    );
    assert_eq!(
        patched.json()["products"][0]["brand"]["name"],
        json!("product.demo.brand")
    );
    let plain = http
        .request("GET", "/api/v1/boot", &[("host", "127.0.0.1:1500")], None)
        .await;
    assert_eq!(
        plain.json()["brand"],
        json!({ "name": "product.demo.brand", "logo": "/brand/logo.svg", "hue": 150, "chroma": 0.1 })
    );
    assert_eq!(patched.json()["meta"], plain.json()["meta"]);
    let logo = svg(&http, "/brand/logo.svg", "panel.example:1500").await;
    assert_svg(&logo, HOST_LOGO, "/brand/logo.svg for the patched host");
    let own = svg(&http, "/brand/logo.svg", "127.0.0.1").await;
    assert_svg(&own, DEMO_LOGO, "/brand/logo.svg for another host");
    let by_id = svg(&http, "/brand/demo/logo.svg", "panel.example").await;
    assert_svg(&by_id, DEMO_LOGO, "/brand/demo/logo.svg is never patched");
    http.stop().await.expect("the server stops");

    let socket = harness.socket().await;
    let keel_types::Envelope::Ok { data, .. } = socket.boot().await else {
        panic!("boot over the socket failed");
    };
    assert_eq!(data.brand.name, "product.demo.brand");
    socket.stop().await.expect("the socket stops");
    harness.finish().await;
}
