use std::fs;

use keel::prelude::*;
use keel::sdk::sea_orm::{ConnectionTrait, DbBackend, Statement};
use keel::types::ErrorCode;
use keel_testkit::{Harness, Reply, Who, tokio};

use crate::support::{Site, create, data, dns, eventually, rows, state_becomes};

const LIMIT: &str = "dns.record.max";

async fn record(harness: &Harness, who: Who, params: Value) -> Value {
    let reply = harness.call(who, "dns.record.create", params).await;
    data(&reply).clone()
}

async fn serial(harness: &Harness, zone: &str) -> i64 {
    let found = rows(
        harness,
        &format!("SELECT serial::text FROM dns_zone WHERE id = '{zone}'"),
    )
    .await;
    found
        .first()
        .and_then(|serial| serial.parse().ok())
        .expect("the serial of the zone")
}

async fn zone_file_has(site: &Site, zone: &str, lines: &[String]) {
    eventually(
        &format!("the zone file of {zone} with {lines:?}"),
        async || {
            fs::read_to_string(site.zone_file(zone))
                .is_ok_and(|file| lines.iter().all(|line| file.lines().any(|at| at == line)))
        },
    )
    .await;
}

async fn served(harness: &Harness, site: &Site, zone: &str, name: &str) -> i64 {
    state_becomes(harness, zone, "active").await;
    let serial = serial(harness, zone).await;
    eventually(
        &format!("the serial {serial} of {name} on disk"),
        async || {
            fs::read_to_string(site.zone_file(name))
                .is_ok_and(|file| file.contains(&format!("( {serial} ")))
        },
    )
    .await;
    serial
}

fn refusal(reply: &Reply) -> (String, String) {
    let error = reply
        .error()
        .unwrap_or_else(|| panic!("a refusal was expected, got {:?}", reply.data()));
    assert_eq!(error.code, ErrorCode::Validation, "{error:?}");
    assert_eq!(error.fields.len(), 1, "{error:?}");
    (
        error.fields[0].path.clone(),
        error.fields[0].code.clone().unwrap_or_default(),
    )
}

#[tokio::test]
async fn a_record_of_every_type_validates_its_value_and_lands_in_the_rendered_zone() {
    let site = Site::new();
    let harness = dns(&site).await;
    let a1 = harness.fixtures().accounts.a1;
    let zone = create(&harness, Who::A1, "example.org").await;
    served(&harness, &site, &zone, "example.org").await;

    let entries = [
        json!({ "type": "A", "name": "@", "value": "192.0.2.1" }),
        json!({ "type": "AAAA", "name": "WWW", "value": "2001:db8::1", "ttl": 300 }),
        json!({ "type": "CNAME", "name": "alias", "value": "Web.Example.NET" }),
        json!({ "type": "MX", "name": "@", "value": "mail.example.net", "priority": 10 }),
        json!({ "type": "TXT", "name": "@", "value": "\"v=spf1 -all\" \"second part\"" }),
        json!({ "type": "NS", "name": "sub", "value": "ns1.example.net" }),
        json!({ "type": "SRV", "name": "_sip._tcp", "value": "sip.example.net", "priority": 10, "weight": 5, "port": 5060 }),
        json!({ "type": "CAA", "name": "@", "value": "0 issue \"letsencrypt.org\"" }),
    ];
    for mut entry in entries {
        entry["zone"] = json!(zone);
        let created = record(&harness, Who::A1, entry.clone()).await;
        assert_eq!(created["zoneId"], json!(zone));
        assert_eq!(created["accountId"], json!(a1.to_string()));
        assert_eq!(created["type"], entry["type"]);
    }
    let lines = [
        "@ 3600 IN A 192.0.2.1",
        "www 300 IN AAAA 2001:db8::1",
        "alias 3600 IN CNAME web.example.net.",
        "@ 3600 IN MX 10 mail.example.net.",
        "@ 3600 IN TXT \"v=spf1 -all\" \"second part\"",
        "sub 3600 IN NS ns1.example.net.",
        "_sip._tcp 3600 IN SRV 10 5 5060 sip.example.net.",
        "@ 3600 IN CAA 0 issue \"letsencrypt.org\"",
    ]
    .map(str::to_owned);
    zone_file_has(&site, "example.org", &lines).await;
    served(&harness, &site, &zone, "example.org").await;

    let listed = harness
        .call(
            Who::A1,
            "dns.record.list",
            json!({ "scope": { "zone_id": zone }, "pageSize": 50 }),
        )
        .await;
    assert_eq!(data(&listed)["total"], json!(8));
    let other = create(&harness, Who::A1, "other.example").await;
    let narrowed = harness
        .call(
            Who::A1,
            "dns.record.list",
            json!({ "scope": { "zone_id": other } }),
        )
        .await;
    assert_eq!(data(&narrowed)["total"], json!(0));
    let foreign = harness
        .call(
            Who::A2,
            "dns.record.list",
            json!({ "scope": { "zone_id": zone } }),
        )
        .await;
    assert_eq!(data(&foreign)["total"], json!(0));
    let intruder = harness
        .call(
            Who::A2,
            "dns.record.create",
            json!({ "zone": zone, "type": "A", "name": "x", "value": "192.0.2.9" }),
        )
        .await;
    assert_eq!(
        intruder.error().map(|error| error.code),
        Some(ErrorCode::NotFound)
    );
    let above = harness
        .call(
            Who::Reseller,
            "dns.record.list",
            json!({ "scope": { "zone_id": zone }, "pageSize": 50 }),
        )
        .await;
    assert_eq!(data(&above)["total"], json!(8));
    harness.finish().await;
}

#[tokio::test]
async fn an_invalid_value_is_refused_on_the_field_before_the_transaction() {
    let site = Site::new();
    let harness = dns(&site).await;
    let zone = create(&harness, Who::A1, "example.org").await;
    let before = served(&harness, &site, &zone, "example.org").await;

    let descriptor = harness
        .registry()
        .function("dns.record.create")
        .expect("dns.record.create is registered")
        .descriptor()
        .clone();
    assert_eq!(descriptor.module, "dns_record");
    let cases = &descriptor.params.as_value()["properties"]["value"]["x-keel"]["checkBy"];
    assert_eq!(cases["field"], json!("type"));
    assert_eq!(
        cases["cases"],
        json!({
            "A": "ipv4", "AAAA": "ipv6", "CNAME": "domain", "MX": "domain",
            "TXT": "quoted", "NS": "domain", "SRV": "domain", "CAA": "caa"
        })
    );

    let invalid = [
        ("A", "2001:db8::1", "check.ipv4"),
        ("A", "192.0.2", "check.ipv4"),
        ("AAAA", "192.0.2.1", "check.ipv6"),
        ("CNAME", "not a host", "check.domain"),
        ("MX", "mail_server.example.net", "check.domain"),
        ("NS", "ns1", "check.domain"),
        ("SRV", "sip.example.net.", "check.domain"),
        ("TXT", "v=spf1 -all", "check.quoted"),
        ("TXT", "\"line\nbreak\"", "check.quoted"),
        ("CAA", "0 issue letsencrypt.org", "check.caa"),
        ("CAA", "300 issue \"letsencrypt.org\"", "check.caa"),
    ];
    let unknown = Id::generate().to_string();
    for (kind, value, code) in invalid {
        for target in [zone.as_str(), unknown.as_str()] {
            let reply = harness
                .call(
                    Who::A1,
                    "dns.record.create",
                    json!({
                        "zone": target, "type": kind, "name": "www", "value": value,
                        "priority": 10, "weight": 1, "port": 5060
                    }),
                )
                .await;
            assert_eq!(
                refusal(&reply),
                ("value".to_owned(), code.to_owned()),
                "{kind} {value:?}"
            );
        }
    }
    let owner = harness
        .call(
            Who::A1,
            "dns.record.create",
            json!({ "zone": zone, "type": "A", "name": "a..b", "value": "192.0.2.1" }),
        )
        .await;
    assert_eq!(
        refusal(&owner),
        ("name".to_owned(), "schema.pattern".to_owned())
    );
    let mx = harness
        .call(
            Who::A1,
            "dns.record.create",
            json!({ "zone": zone, "type": "MX", "name": "@", "value": "mail.example.net" }),
        )
        .await;
    assert_eq!(
        refusal(&mx),
        ("priority".to_owned(), "schema.required".to_owned())
    );

    let kept = record(
        &harness,
        Who::A1,
        json!({ "zone": zone, "type": "A", "name": "www", "value": "192.0.2.1" }),
    )
    .await;
    let edited = harness
        .call(
            Who::A1,
            "dns.record.edit",
            json!({ "id": kept["id"], "type": "AAAA", "name": "www", "value": "192.0.2.1", "ttl": 3600 }),
        )
        .await;
    assert_eq!(
        refusal(&edited),
        ("value".to_owned(), "check.ipv6".to_owned())
    );
    assert_eq!(serial(&harness, &zone).await, before + 1);
    assert_eq!(
        rows(&harness, "SELECT type || ' ' || value FROM dns_record").await,
        ["A 192.0.2.1"]
    );
    harness.finish().await;
}

#[tokio::test]
async fn every_change_bumps_the_serial_once_per_transaction() {
    let site = Site::new();
    let harness = dns(&site).await;
    let zone = create(&harness, Who::A1, "example.org").await;
    let other = create(&harness, Who::A1, "example.net").await;
    let start = served(&harness, &site, &zone, "example.org").await;
    let other_start = served(&harness, &site, &other, "example.net").await;

    let mut ids = Vec::new();
    for (step, name) in ["a", "b", "c"].into_iter().enumerate() {
        let created = record(
            &harness,
            Who::A1,
            json!({ "zone": zone, "type": "A", "name": name, "value": "192.0.2.1" }),
        )
        .await;
        ids.push(created["id"].as_str().expect("an id").to_owned());
        assert_eq!(
            serial(&harness, &zone).await,
            start + i64::try_from(step).expect("a step") + 1
        );
    }
    let foreign = record(
        &harness,
        Who::A1,
        json!({ "zone": other, "type": "A", "name": "d", "value": "192.0.2.4" }),
    )
    .await;
    assert_eq!(serial(&harness, &other).await, other_start + 1);
    let created = served(&harness, &site, &zone, "example.org").await;
    assert_eq!(created, start + 3);

    let grouped = harness
        .call(
            Who::A1,
            "dns.record.edit",
            json!({ "ids": ids, "fields": { "ttl": 300 }, "mask": ["ttl"] }),
        )
        .await;
    assert_eq!(data(&grouped)["updated"], json!(3));
    assert_eq!(serial(&harness, &zone).await, created + 1);
    zone_file_has(
        &site,
        "example.org",
        &["a", "b", "c"].map(|name| format!("{name} 300 IN A 192.0.2.1")),
    )
    .await;
    let grouped = served(&harness, &site, &zone, "example.org").await;

    let single = harness
        .call(
            Who::A1,
            "dns.record.edit",
            json!({ "id": ids[0], "type": "A", "name": "a", "value": "192.0.2.10", "ttl": 300 }),
        )
        .await;
    assert_eq!(data(&single)["value"], json!("192.0.2.10"));
    assert_eq!(serial(&harness, &zone).await, grouped + 1);

    let deleted = harness
        .call(
            Who::A1,
            "dns.record.delete",
            json!({ "ids": [ids[1], ids[2], foreign["id"]] }),
        )
        .await;
    assert_eq!(data(&deleted)["deleted"], json!(3));
    assert_eq!(serial(&harness, &zone).await, grouped + 2);
    assert_eq!(serial(&harness, &other).await, other_start + 2);
    zone_file_has(&site, "example.org", &["a 300 IN A 192.0.2.10".to_owned()]).await;
    let last = served(&harness, &site, &zone, "example.org").await;
    assert_eq!(last, grouped + 2);
    let file = fs::read_to_string(site.zone_file("example.org")).expect("the zone file");
    assert!(!file.contains("\nb "), "{file}");
    assert!(!file.contains("\nc "), "{file}");
    served(&harness, &site, &other, "example.net").await;
    let file = fs::read_to_string(site.zone_file("example.net")).expect("the zone file");
    assert!(!file.contains("\nd "), "{file}");

    rows(
        &harness,
        &format!("UPDATE dns_zone SET serial = 2020010105 WHERE id = '{other}' RETURNING id"),
    )
    .await;
    record(
        &harness,
        Who::A1,
        json!({ "zone": other, "type": "A", "name": "e", "value": "192.0.2.5" }),
    )
    .await;
    assert_eq!(
        rows(
            &harness,
            &format!(
                "SELECT (serial = to_char(now() AT TIME ZONE 'UTC', 'YYYYMMDD')::bigint * 100)::text FROM dns_zone WHERE id = '{other}'"
            )
        )
        .await,
        ["true"],
        "a serial from an earlier day jumps to the first serial of today"
    );

    let events = rows(
        &harness,
        "SELECT type FROM keel_outbox WHERE type LIKE 'dns.record.%' ORDER BY seq",
    )
    .await;
    assert_eq!(
        events,
        [
            "dns.record.created",
            "dns.record.created",
            "dns.record.created",
            "dns.record.created",
            "dns.record.updated",
            "dns.record.updated",
            "dns.record.updated",
            "dns.record.updated",
            "dns.record.deleted",
            "dns.record.deleted",
            "dns.record.deleted",
            "dns.record.created",
        ]
    );
    harness.finish().await;
}

#[tokio::test]
async fn records_of_a_zone_over_the_limit_are_refused() {
    let site = Site::new();
    let harness = dns(&site).await;
    let a1 = harness.fixtures().accounts.a1;
    harness
        .db()
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO keel_limit (account_id, key, value) VALUES ($1, $2, '2'::jsonb)",
            [a1.to_string().into(), LIMIT.into()],
        ))
        .await
        .expect("the limit of two records");
    let zone = create(&harness, Who::A1, "example.org").await;
    let other = create(&harness, Who::A1, "example.net").await;
    let start = served(&harness, &site, &zone, "example.org").await;

    for name in ["a", "b"] {
        record(
            &harness,
            Who::A1,
            json!({ "zone": zone, "type": "A", "name": name, "value": "192.0.2.1" }),
        )
        .await;
    }
    let refused = harness
        .call(
            Who::A1,
            "dns.record.create",
            json!({ "zone": zone, "type": "A", "name": "c", "value": "192.0.2.1" }),
        )
        .await;
    let error = refused.error().expect("the third record is over the limit");
    assert_eq!(error.code, ErrorCode::LimitExceeded);
    assert_eq!(error.key, "keel.error.limit_exceeded");
    assert_eq!(error.params["limit"], json!(LIMIT));
    assert_eq!(error.params["max"], json!(2));
    assert_eq!(serial(&harness, &zone).await, start + 2);

    for name in ["a", "b"] {
        record(
            &harness,
            Who::A1,
            json!({ "zone": other, "type": "A", "name": name, "value": "192.0.2.2" }),
        )
        .await;
    }
    let reseller = harness
        .call(
            Who::Reseller,
            "dns.record.create",
            json!({ "zone": zone, "type": "A", "name": "c", "value": "192.0.2.1" }),
        )
        .await;
    assert_eq!(
        reseller.error().map(|error| error.code),
        Some(ErrorCode::LimitExceeded)
    );
    assert_eq!(
        rows(
            &harness,
            &format!("SELECT count(*)::text FROM dns_record WHERE zone_id = '{zone}'")
        )
        .await,
        ["2"]
    );
    harness.finish().await;
}
