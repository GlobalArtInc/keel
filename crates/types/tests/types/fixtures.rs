use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use keel_types::{
    Boot, Check, Context, Descriptor, Envelope, ErrorCode, Event, Expr, FilterOp, Kind, Level,
    ListQuery, Mode, Page, WidgetKind,
};
use serde::Deserialize;
use serde_json::Value;

const KNOWN: [&str; 13] = [
    "boot-none.json",
    "boot.json",
    "check-cases.json",
    "descriptor-dns-zone-edit.json",
    "descriptor-dns-zone-list.json",
    "envelope-200.json",
    "envelope-403.json",
    "envelope-422.json",
    "error-codes.json",
    "event.json",
    "expr-cases.json",
    "list-query.json",
    "page.json",
];

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/fixtures")
}

fn load(name: &str) -> Value {
    let path = fixtures_dir().join(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn round_trip<T>(fixture: Value) -> T
where
    T: for<'de> Deserialize<'de> + serde::Serialize,
{
    let typed: T = serde_json::from_value(fixture.clone()).expect("fixture deserialises");
    let back = serde_json::to_value(&typed).expect("fixture serialises");
    assert_eq!(
        back, fixture,
        "re-serialised fixture differs from the original"
    );
    typed
}

#[test]
fn every_fixture_file_has_a_test() {
    let present: BTreeSet<String> = fs::read_dir(fixtures_dir())
        .expect("contracts/fixtures exists")
        .map(|entry| entry.expect("readable entry"))
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    let known: BTreeSet<String> = KNOWN.iter().map(|name| (*name).to_owned()).collect();
    assert_eq!(
        present, known,
        "every fixture file must be covered by a test in this module"
    );
}

#[derive(Deserialize)]
struct ExprCase {
    name: String,
    source: String,
    #[serde(default)]
    invalid: bool,
    #[serde(default)]
    ast: Option<Expr>,
    #[serde(default)]
    context: Context,
    #[serde(default)]
    expect: Option<bool>,
}

#[test]
fn expr_cases_parse_and_evaluate_to_their_expected_value() {
    let raw = load("expr-cases.json");
    let cases: Vec<ExprCase> = serde_json::from_value(raw.clone()).expect("expr cases deserialise");
    assert!(cases.len() >= 60);
    for (case, raw_case) in cases.iter().zip(raw.as_array().expect("array")) {
        if case.invalid {
            assert!(
                Expr::parse(&case.source).is_err(),
                "{}: {:?} should not parse",
                case.name,
                case.source
            );
            continue;
        }
        let expected_ast = case
            .ast
            .as_ref()
            .unwrap_or_else(|| panic!("{}: a valid case carries an ast", case.name));
        let parsed = Expr::parse(&case.source).unwrap_or_else(|e| panic!("{}: {e}", case.name));
        assert_eq!(&parsed, expected_ast, "{}: parsed AST differs", case.name);
        assert_eq!(
            serde_json::to_value(&parsed).expect("ast serialises"),
            raw_case["ast"],
            "{}: serialised AST differs from the fixture",
            case.name
        );
        let expect = case
            .expect
            .unwrap_or_else(|| panic!("{}: a valid case carries an expectation", case.name));
        let value = expected_ast
            .eval(&case.context)
            .unwrap_or_else(|e| panic!("{}: {e}", case.name));
        assert_eq!(value, expect, "{}: evaluation differs", case.name);
    }
}

#[derive(Deserialize)]
struct CheckCase {
    check: Check,
    value: String,
    expect: bool,
}

#[test]
fn check_cases_match_their_expected_verdict() {
    let cases: Vec<CheckCase> =
        serde_json::from_value(load("check-cases.json")).expect("check cases deserialise");
    let covered: BTreeSet<&str> = cases.iter().map(|case| case.check.as_str()).collect();
    assert_eq!(covered.len(), Check::ALL.len(), "every preset has cases");
    for case in cases {
        assert_eq!(
            case.check.matches(&case.value),
            case.expect,
            "{} {:?} should be {}",
            case.check.as_str(),
            case.value,
            case.expect
        );
    }
}

#[test]
fn descriptor_fixture_round_trips_without_loss() {
    let descriptor: Descriptor = round_trip(load("descriptor-dns-zone-list.json"));
    assert_eq!(descriptor.keel, keel_types::DESCRIPTOR_VERSION);
    assert_eq!(descriptor.name, "dns.zone.list");
    assert_eq!(descriptor.kind, Kind::List);
    assert_eq!(descriptor.mode, Mode::Read);
    assert_eq!(descriptor.access.level, Level::User);
    assert_eq!(descriptor.key.as_deref(), Some("id"));
    let ui = descriptor.ui.expect("list ui");
    assert_eq!(ui.columns.len(), 5);
    assert_eq!(ui.actions.len(), 6);
    assert_eq!(ui.row_actions, ["edit", "records", "suspend", "delete"]);
    let hidden = ui.actions[3]
        .hidden_if
        .as_ref()
        .expect("suspend carries hidden_if");
    assert_eq!(
        hidden,
        &Expr::parse("status == 'suspended'").expect("parses")
    );
}

#[test]
fn form_descriptor_fixture_round_trips_without_loss() {
    let descriptor: Descriptor = round_trip(load("descriptor-dns-zone-edit.json"));
    assert_eq!(descriptor.name, "dns.zone.edit");
    assert_eq!(descriptor.kind, Kind::Form);
    assert_eq!(descriptor.mode, Mode::Write);
    assert_eq!(descriptor.key.as_deref(), Some("id"));
    let ui = descriptor.ui.expect("form ui");
    assert_eq!(ui.read.as_deref(), Some("dns.zone.get"));
    assert_eq!(ui.pages.len(), 2);
    assert_eq!(
        ui.pages[1].show_if.as_ref(),
        Some(&Expr::parse("kind == 'slave'").expect("parses"))
    );
    assert_eq!(ui.multi, ["ttl"]);
}

#[test]
fn envelope_fixtures_deserialise_and_round_trip() {
    let ok: Envelope<Value> = round_trip(load("envelope-200.json"));
    match ok {
        Envelope::Ok { data, meta } => {
            assert_eq!(data["id"], "01J9Q3W6B1R8N5X2K4T7M0V9Z3");
            assert_eq!(meta.request_id, "req-1a2b");
            let principal = meta.principal.expect("principal");
            assert_eq!(principal.level, Level::User);
            assert_eq!(principal.actor.expect("actor").login, "root");
        }
        Envelope::Err { .. } => panic!("envelope-200 is an ok envelope"),
    }

    let validation: Envelope<Value> = round_trip(load("envelope-422.json"));
    match validation {
        Envelope::Err { error } => {
            assert_eq!(error.code, ErrorCode::Validation);
            assert_eq!(error.status(), 422);
            assert_eq!(error.fields.len(), 2);
            assert_eq!(error.fields[0].path, "master_ips[1]");
            assert_eq!(
                error.fields[0].code.as_deref(),
                Some(Check::Ip.code().as_str())
            );
            assert_eq!(
                error.fields[0].key.as_deref(),
                Some(Check::Ip.key().as_str())
            );
        }
        Envelope::Ok { .. } => panic!("envelope-422 is an error envelope"),
    }

    let forbidden: Envelope<Value> = round_trip(load("envelope-403.json"));
    match forbidden {
        Envelope::Err { error } => {
            assert_eq!(error.code, ErrorCode::ForbiddenField);
            assert_eq!(error.status(), 403);
            assert_eq!(error.fields[0].path, "account");
            assert!(error.fields[0].code.is_none());
        }
        Envelope::Ok { .. } => panic!("envelope-403 is an error envelope"),
    }
}

#[derive(Deserialize)]
struct CodeRow {
    code: ErrorCode,
    status: u16,
}

#[test]
fn error_code_status_table_matches_the_contracts_document() {
    let rows: Vec<CodeRow> =
        serde_json::from_value(load("error-codes.json")).expect("table deserialises");
    assert_eq!(rows.len(), ErrorCode::ALL.len());
    let listed: BTreeSet<&str> = rows.iter().map(|row| row.code.as_str()).collect();
    assert_eq!(
        listed.len(),
        ErrorCode::ALL.len(),
        "every code appears once"
    );
    for row in rows {
        assert_eq!(row.code.status(), row.status, "{}", row.code.as_str());
    }
}

#[test]
fn list_query_fixture_round_trips() {
    let query: ListQuery = round_trip(load("list-query.json"));
    assert_eq!(query.page, 2);
    assert_eq!(query.page_size, 25);
    assert_eq!(query.search, "www");
    assert_eq!(query.filters[0].op, FilterOp::Eq);
    assert_eq!(query.filters[1].op, FilterOp::Gte);
    assert_eq!(query.scope["zone_id"], "01J9Q3W6B1R8N5X2K4T7M0V9Z3");
}

#[test]
fn page_fixture_round_trips() {
    let page: Page<Value> = round_trip(load("page.json"));
    assert_eq!(page.total, 7);
    assert_eq!(page.data.len(), 1);
    assert!(page.totals.is_none());
}

#[test]
fn event_fixture_round_trips() {
    let event: Event = round_trip(load("event.json"));
    assert_eq!(event.kind, "dns.zone.created");
    assert_eq!(event.module, "dns");
    assert_eq!(event.seq, 8812);
    assert_eq!(event.subject.expect("subject").kind, "dns.zone");
}

#[test]
fn boot_none_fixture_round_trips_with_no_product_no_view_the_platform_group_alone_and_the_accounts_widget()
 {
    let boot: Boot = round_trip(load("boot-none.json"));
    assert_eq!(boot.keel.api, keel_types::DESCRIPTOR_VERSION);
    assert!(boot.products.is_empty());
    assert!(boot.views.is_empty());
    assert_eq!(boot.brand.name, "keel.brand");
    assert!(boot.brand.logo.is_none());
    assert_eq!(boot.brand.hue, 264);
    let principal = boot.principal.expect("the owner");
    assert_eq!(principal.level, Level::Owner);
    assert!(principal.actor.is_none());
    let groups: Vec<(&str, Option<&str>)> = boot
        .menu
        .iter()
        .map(|group| (group.id.as_str(), group.product.as_deref()))
        .collect();
    assert_eq!(
        groups,
        [("access", None), ("monitor", None), ("system", None)]
    );
    assert!(
        boot.menu
            .iter()
            .flat_map(|group| &group.items)
            .all(|item| item.kind.is_some())
    );
    assert!(boot.features.is_empty());
    let widgets: Vec<(WidgetKind, &str)> = boot
        .dashboard
        .iter()
        .map(|widget| (widget.kind, widget.func.as_str()))
        .collect();
    assert_eq!(widgets, [(WidgetKind::Stat, "keel.widget.accounts")]);
    assert!(boot.license.is_none());
    assert!(boot.problems.is_none());
}

#[test]
fn boot_fixture_round_trips_with_the_platform_brand_and_a_product_on_every_group() {
    let boot: Boot = round_trip(load("boot.json"));
    assert_eq!(boot.keel.api, keel_types::DESCRIPTOR_VERSION);
    assert_eq!(boot.products.len(), 2);
    assert!(
        boot.products
            .iter()
            .all(|product| product.edition.is_none())
    );
    assert_eq!(boot.products[0].brand.name, boot.products[0].name);
    assert_eq!(
        boot.products[0].brand.logo.as_deref(),
        Some("/brand/dns/logo.svg")
    );
    assert_eq!(boot.products[1].brand.hue, 264);
    assert_eq!(boot.brand.name, "keel.brand");
    assert!(boot.brand.logo.is_none());
    let principal = boot.principal.expect("a signed-in principal");
    assert_eq!(principal.level, Level::Reseller);
    assert_eq!(principal.lang, "ru");
    assert!(principal.tz.is_none());
    assert!(!principal.totp);
    let products: Vec<Option<&str>> = boot
        .menu
        .iter()
        .map(|group| group.product.as_deref())
        .collect();
    assert_eq!(products, [Some("dns"), Some("panel"), None, None]);
    assert_eq!(boot.menu[0].items[0].kind, Some(Kind::List));
    assert_eq!(
        boot.menu[0].items[0]
            .badge
            .as_ref()
            .map(|badge| badge.problem.as_str()),
        Some("dns.zone_stale")
    );
    assert!(boot.license.is_none());
    assert!(boot.dashboard.is_empty());
    assert!(boot.problems.is_none());
    assert!(boot.meta.starts_with("m-"));
}
