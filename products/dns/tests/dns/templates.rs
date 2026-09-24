use keel::prelude::*;
use keel::sdk::sea_orm::{ConnectionTrait, DbBackend, Statement};
use keel::types::ErrorCode;
use keel_testkit::{Harness, Reply, Who, tokio};

use crate::support::{Site, data, dns, names, rows, state_becomes, zone_file_has};

const WEB: &str = "00000000000000000000000001";
const EMPTY: &str = "00000000000000000000000002";
const ROOT: &str = "00000000000000000000000000";

async fn template(harness: &Harness, who: Who, name: &str, records: Value) -> String {
    let reply = harness
        .call(
            who,
            "dns.template.create",
            json!({ "name": name, "records": records }),
        )
        .await;
    data(&reply)["id"].as_str().expect("an id").to_owned()
}

async fn zone(harness: &Harness, who: Who, params: Value) -> Value {
    let reply = harness.call(who, "dns.zone.create", params).await;
    data(&reply).clone()
}

async fn records_of(harness: &Harness, zone: &str) -> Vec<String> {
    rows(
        harness,
        &format!(
            "SELECT type || ' ' || name || ' ' || value || ' ' || ttl || ' ' || coalesce(priority::text, '-') \
             FROM dns_record WHERE zone_id = '{zone}' ORDER BY type, name, value"
        ),
    )
    .await
}

async fn offered(harness: &Harness, who: Who) -> Vec<String> {
    let reply = harness
        .call(who, "dns.template.options", json!({ "limit": 100 }))
        .await;
    data(&reply)["data"]
        .as_array()
        .expect("choices")
        .iter()
        .map(|choice| choice["value"].as_str().expect("a value").to_owned())
        .collect()
}

fn refused_on(reply: &Reply) -> (ErrorCode, String, String) {
    let error = reply
        .error()
        .unwrap_or_else(|| panic!("a refusal was expected, got {:?}", reply.data()));
    let field = error.fields.first().expect("a field error");
    (
        error.code,
        field.path.clone(),
        field.key.clone().unwrap_or_default(),
    )
}

#[tokio::test]
async fn a_zone_created_with_a_template_receives_its_expanded_records() {
    let site = Site::new();
    let harness = dns(&site).await;
    let records = json!([
        { "type": "A", "name": "@", "value": "{ip}" },
        { "type": "AAAA", "name": "@", "value": "{ip6}", "ttl": 600 },
        { "type": "CNAME", "name": "www", "value": "{zone}" },
        { "type": "A", "name": "mail", "value": "{ip}" },
        { "type": "MX", "name": "@", "value": "mail.{zone}", "priority": 10 },
        { "type": "TXT", "name": "@", "value": "\"v=spf1 ip4:{ip} -all\"" },
    ]);
    let id = template(&harness, Who::A1, "Mail and web", records).await;

    let created = zone(
        &harness,
        Who::A1,
        json!({ "name": "example.org", "template": id, "ip": "192.0.2.10", "ip6": "2001:db8::10" }),
    )
    .await;
    let zone_id = created["id"].as_str().expect("an id").to_owned();
    assert_eq!(created["template"], json!(id));
    assert_eq!(
        records_of(&harness, &zone_id).await,
        [
            "A @ 192.0.2.10 3600 -",
            "A mail 192.0.2.10 3600 -",
            "AAAA @ 2001:db8::10 600 -",
            "CNAME www example.org 3600 -",
            "MX @ mail.example.org 3600 10",
            "TXT @ \"v=spf1 ip4:192.0.2.10 -all\" 3600 -",
        ]
    );
    let lines = [
        "@ 3600 IN A 192.0.2.10",
        "mail 3600 IN A 192.0.2.10",
        "@ 600 IN AAAA 2001:db8::10",
        "www 3600 IN CNAME example.org.",
        "@ 3600 IN MX 10 mail.example.org.",
        "@ 3600 IN TXT \"v=spf1 ip4:192.0.2.10 -all\"",
    ]
    .map(str::to_owned);
    zone_file_has(&site, "example.org", &lines).await;
    state_becomes(&harness, &zone_id, "active").await;

    let without_ip6 = zone(
        &harness,
        Who::A1,
        json!({ "name": "example.net", "template": id, "ip": "192.0.2.20" }),
    )
    .await;
    let without_ip6 = without_ip6["id"].as_str().expect("an id");
    let kept = records_of(&harness, without_ip6).await;
    assert_eq!(kept.len(), 5, "{kept:?}");
    assert!(kept.iter().all(|record| !record.starts_with("AAAA ")));

    let bad = harness
        .call(
            Who::A1,
            "dns.template.create",
            json!({ "name": "Broken", "records": [
                { "type": "CNAME", "name": "www", "value": "{zone}" },
                { "type": "A", "name": "@", "value": "{ip6}" },
            ] }),
        )
        .await;
    assert_eq!(
        refused_on(&bad),
        (
            ErrorCode::Validation,
            "records[1].value".to_owned(),
            "keel.check.ipv4".to_owned()
        )
    );
    let taken = harness
        .call(
            Who::A1,
            "dns.template.create",
            json!({ "name": "Mail and web", "records": [] }),
        )
        .await;
    assert_eq!(
        refused_on(&taken),
        (
            ErrorCode::Validation,
            "name".to_owned(),
            "dns.template.error.exists".to_owned()
        )
    );

    harness
        .db()
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE dns_template SET records = $2::jsonb WHERE id = $1",
            [
                id.as_str().into(),
                r#"[{ "type": "A", "name": "@", "value": "{zone}" }]"#.into(),
            ],
        ))
        .await
        .expect("a record set stored past the checks");
    let stale = harness
        .call(
            Who::A1,
            "dns.zone.create",
            json!({ "name": "example.com", "template": id }),
        )
        .await;
    assert_eq!(
        refused_on(&stale),
        (
            ErrorCode::Validation,
            "template".to_owned(),
            "dns.zone.error.template".to_owned()
        )
    );
    assert_eq!(
        rows(
            &harness,
            "SELECT count(*)::text FROM dns_zone WHERE name = 'example.com'"
        )
        .await,
        ["0"]
    );
    harness.finish().await;
}

#[tokio::test]
async fn a_template_of_another_account_is_not_offered_and_not_applied() {
    let site = Site::new();
    let harness = dns(&site).await;
    let records = json!([{ "type": "A", "name": "@", "value": "{ip}" }]);
    let own = template(&harness, Who::A1, "Own", records.clone()).await;
    let foreign = template(&harness, Who::A2, "Foreign", records).await;

    let choices = offered(&harness, Who::A1).await;
    assert_eq!(choices, [WEB, EMPTY, own.as_str()]);
    let listed = harness
        .call(Who::A1, "dns.template.list", json!({ "pageSize": 50 }))
        .await;
    assert_eq!(names(&listed), ["Empty", "Own", "Web host"]);
    let reseller = offered(&harness, Who::Reseller).await;
    assert!(reseller.contains(&own) && reseller.contains(&foreign));

    let refused = harness
        .call(
            Who::A1,
            "dns.zone.create",
            json!({ "name": "example.org", "template": foreign, "ip": "192.0.2.1" }),
        )
        .await;
    assert_eq!(
        refused_on(&refused),
        (
            ErrorCode::Validation,
            "template".to_owned(),
            "dns.template.error.unknown".to_owned()
        )
    );
    assert_eq!(
        rows(&harness, "SELECT count(*)::text FROM dns_zone").await,
        ["0"]
    );
    assert_eq!(
        rows(&harness, "SELECT count(*)::text FROM dns_record").await,
        ["0"]
    );

    for (function, params) in [
        ("dns.template.get", json!({ "id": foreign })),
        (
            "dns.template.edit",
            json!({ "id": foreign, "name": "Taken over", "records": [] }),
        ),
        ("dns.template.delete", json!({ "ids": [foreign] })),
        (
            "dns.template.edit",
            json!({ "id": WEB, "name": "Taken over", "records": [] }),
        ),
        ("dns.template.delete", json!({ "ids": [EMPTY] })),
    ] {
        let reply = harness.call(Who::A1, function, params).await;
        assert_eq!(
            reply.error().map(|error| error.code),
            Some(ErrorCode::NotFound),
            "{function}"
        );
    }
    assert_eq!(
        rows(&harness, "SELECT name FROM dns_template ORDER BY name").await,
        ["Empty", "Foreign", "Own", "Web host"]
    );

    let applied = zone(
        &harness,
        Who::A1,
        json!({ "name": "example.org", "template": own, "ip": "192.0.2.1" }),
    )
    .await;
    let applied = applied["id"].as_str().expect("an id");
    assert_eq!(
        records_of(&harness, applied).await,
        ["A @ 192.0.2.1 3600 -"]
    );
    harness.finish().await;
}

#[tokio::test]
async fn the_platform_templates_are_present_after_the_first_start() {
    let site = Site::new();
    let harness = dns(&site).await;
    assert_eq!(
        rows(
            &harness,
            "SELECT id || ' ' || account_id || ' ' || name || ' ' || jsonb_array_length(records) \
             FROM dns_template ORDER BY id"
        )
        .await,
        [
            format!("{WEB} {ROOT} Web host 4"),
            format!("{EMPTY} {ROOT} Empty 0"),
        ]
    );
    for who in [Who::A1, Who::Reseller, Who::Admin] {
        assert_eq!(offered(&harness, who).await, [WEB, EMPTY], "{who:?}");
    }
    let web = harness
        .call(Who::A1, "dns.template.get", json!({ "id": WEB }))
        .await;
    let kinds: Vec<&str> = data(&web)["records"]
        .as_array()
        .expect("records")
        .iter()
        .map(|record| record["type"].as_str().expect("a type"))
        .collect();
    assert_eq!(kinds, ["A", "AAAA", "CNAME", "MX"]);

    let hosted = zone(
        &harness,
        Who::A1,
        json!({ "name": "example.org", "template": WEB, "ip": "192.0.2.1" }),
    )
    .await;
    let hosted = hosted["id"].as_str().expect("an id");
    assert_eq!(
        records_of(&harness, hosted).await,
        [
            "A @ 192.0.2.1 3600 -",
            "CNAME www example.org 3600 -",
            "MX @ example.org 3600 10",
        ]
    );
    let empty = zone(
        &harness,
        Who::A1,
        json!({ "name": "example.net", "template": EMPTY }),
    )
    .await;
    assert_eq!(empty["template"], json!(EMPTY));
    let empty = empty["id"].as_str().expect("an id");
    assert!(records_of(&harness, empty).await.is_empty());

    let renamed = harness
        .call(
            Who::Admin,
            "dns.template.edit",
            json!({ "id": EMPTY, "name": "Blank", "records": [] }),
        )
        .await;
    assert_eq!(data(&renamed)["name"], json!("Blank"));
    harness.finish().await;
}
