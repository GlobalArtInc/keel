use std::collections::{BTreeMap, BTreeSet};

use keel_types::{Descriptor, ErrorCode, Kind};
use serde_json::{Map, Value, json};

use super::Suite;
use crate::client::{Answer, Credential};
use crate::report::{Aborted, Failure, ensure};

pub const PROBE: &str = "conformance";
const RAW: [Kind; 3] = [Kind::Stream, Kind::Download, Kind::Upload];
const DEPTH: usize = 16;
const NUMBER: &str = "number";
const INTEGER: &str = "integer";
const VALIDATION: &str = "keel.error.validation";

pub async fn run(suite: &mut Suite) -> Result<(), Aborted> {
    let outcome = suite.login().await;
    suite
        .report
        .check("auth.login as the admin for the field checks", outcome)?;
    let outcome = listing(suite).await;
    let wide = suite.report.check(
        "GET /api/v1/functions answers the admin's descriptors",
        outcome,
    )?;
    let outcome = suite.find_user().await;
    let user = suite.report.check(
        "keel.principal.list finds the user for the field checks",
        outcome,
    )?;
    let outcome = suite.su(&user).await;
    suite
        .report
        .check("auth.su into the user for the field checks", outcome)?;
    let outcome = listing(suite).await;
    let narrow = suite.report.check(
        "GET /api/v1/functions answers the user's descriptors",
        outcome,
    )?;
    let cases = cases(&wide, &narrow);
    let mut refused = Vec::new();
    let mut hidden = 0;
    for case in cases.iter().filter(|case| !case.hidden.is_empty()) {
        hidden += case.hidden.len();
        let params: Map<String, Value> = case
            .hidden
            .iter()
            .map(|name| (name.clone(), json!(PROBE)))
            .collect();
        let answer = call(suite, &case.function, Value::Object(params)).await;
        if let Err(failure) = forbidden(answer, &case.hidden) {
            refused.push(failure.0);
        }
    }
    suite.report.check(
        &format!(
            "each of the {hidden} fields the descriptors hide from the user is refused with 403 forbidden_field"
        ),
        ensure(refused.is_empty(), || refused.join("; ")),
    )?;
    let mut admitted = Vec::new();
    let mut shown = 0;
    for case in &cases {
        let Some(call_of) = mistyped(&case.schema, &case.shown, case.key.as_deref()) else {
            continue;
        };
        shown += call_of.count;
        let answer = call(suite, &case.function, call_of.params).await;
        if let Err(failure) = admitted_fields(answer, &call_of.wrong, &case.shown) {
            admitted.push(format!("{}: {}", case.function, failure.0));
        }
    }
    suite.report.check(
        &format!(
            "each of the {shown} fields the descriptors show the user passes the forbidden-field stage: a call carrying them all mistyped answers 422 keel.error.validation naming each mistyped field and none the user is not shown"
        ),
        ensure(admitted.is_empty(), || admitted.join("; ")),
    )?;
    let outcome = suite.su_exit().await;
    suite
        .report
        .check("auth.su.exit after the field checks", outcome)?;
    let outcome = suite.logout().await;
    suite
        .report
        .check("auth.logout after the field checks", outcome)?;
    Ok(())
}

async fn listing(suite: &mut Suite) -> Result<Vec<Descriptor>, Failure> {
    let answer = suite
        .client
        .get("/api/v1/functions", Credential::Session, &[])
        .await?;
    answer.status_is(200)?;
    answer.body_as()
}

async fn call(suite: &mut Suite, function: &str, params: Value) -> Result<Answer, Failure> {
    suite
        .client
        .call(function, params, Credential::Session, &[])
        .await
        .map(Answer::confidential)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Case {
    pub function: String,
    pub key: Option<String>,
    pub schema: Value,
    pub shown: BTreeSet<String>,
    pub hidden: BTreeSet<String>,
}

fn properties(schema: &Value) -> BTreeSet<String> {
    schema["properties"]
        .as_object()
        .map(|properties| properties.keys().cloned().collect())
        .unwrap_or_default()
}

pub fn cases(wide: &[Descriptor], narrow: &[Descriptor]) -> Vec<Case> {
    let wide: BTreeMap<&str, &Descriptor> = wide
        .iter()
        .map(|descriptor| (descriptor.name.as_str(), descriptor))
        .collect();
    narrow
        .iter()
        .filter(|descriptor| !RAW.contains(&descriptor.kind))
        .map(|descriptor| {
            let schema = descriptor.params.as_value().clone();
            let shown = properties(&schema);
            let hidden = wide
                .get(descriptor.name.as_str())
                .map(|admin| properties(admin.params.as_value()))
                .unwrap_or_default()
                .difference(&shown)
                .cloned()
                .collect();
            Case {
                function: descriptor.name.clone(),
                key: descriptor.key.clone(),
                schema,
                shown,
                hidden,
            }
        })
        .collect()
}

fn resolve<'a>(root: &'a Value, schema: &'a Value, depth: usize) -> Option<&'a Value> {
    if depth > DEPTH {
        return None;
    }
    match schema.get("$ref").and_then(Value::as_str) {
        Some(reference) => root
            .pointer(reference.strip_prefix('#')?)
            .and_then(|target| resolve(root, target, depth + 1)),
        None => Some(schema),
    }
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(number) if number.is_i64() || number.is_u64() => INTEGER,
        Value::Number(_) => NUMBER,
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

pub fn admitted_types(root: &Value, schema: &Value, depth: usize) -> Option<BTreeSet<String>> {
    let schema = resolve(root, schema, depth)?;
    let object = schema.as_object()?;
    if let Some(value) = object.get("const") {
        return Some(BTreeSet::from([kind_of(value).to_owned()]));
    }
    if let Some(values) = object.get("enum").and_then(Value::as_array) {
        return Some(
            values
                .iter()
                .map(|value| kind_of(value).to_owned())
                .collect(),
        );
    }
    match object.get("type") {
        Some(Value::String(kind)) => return Some(BTreeSet::from([kind.clone()])),
        Some(Value::Array(kinds)) => {
            return Some(
                kinds
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
            );
        }
        _ => {}
    }
    for combinator in ["anyOf", "oneOf"] {
        if let Some(branches) = object.get(combinator).and_then(Value::as_array) {
            let mut union = BTreeSet::new();
            for branch in branches {
                union.extend(admitted_types(root, branch, depth + 1)?);
            }
            return Some(union);
        }
    }
    None
}

pub fn outside(admitted: &BTreeSet<String>) -> Option<Value> {
    let numbers = admitted.contains(NUMBER);
    [
        ("boolean", json!(true)),
        ("string", json!(PROBE)),
        ("object", json!({ PROBE: true })),
        ("array", json!([PROBE])),
        (NUMBER, json!(1.5)),
    ]
    .into_iter()
    .find(|(kind, _)| !admitted.contains(*kind) && !(*kind == NUMBER && numbers))
    .map(|(_, value)| value)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mistyped {
    pub params: Value,
    pub count: usize,
    pub wrong: BTreeSet<String>,
}

pub fn mistyped(schema: &Value, shown: &BTreeSet<String>, key: Option<&str>) -> Option<Mistyped> {
    let mut params = Map::new();
    let mut wrong = BTreeSet::new();
    for name in shown.iter().filter(|name| Some(name.as_str()) != key) {
        let property = &schema["properties"][name];
        let value = match admitted_types(schema, property, 0)
            .as_ref()
            .and_then(outside)
        {
            Some(value) => {
                wrong.insert(name.clone());
                value
            }
            None => json!(PROBE),
        };
        params.insert(name.clone(), value);
    }
    let count = params.len();
    (!wrong.is_empty()).then_some(Mistyped {
        params: Value::Object(params),
        count,
        wrong,
    })
}

fn member(path: &str) -> &str {
    path.split(['.', '[']).next().unwrap_or(path)
}

fn forbidden(answer: Result<Answer, Failure>, hidden: &BTreeSet<String>) -> Result<(), Failure> {
    let answer = answer?;
    let error = answer.error(403, ErrorCode::ForbiddenField, "keel.error.forbidden_field")?;
    let paths: BTreeSet<String> = error
        .fields
        .iter()
        .map(|field| field.path.clone())
        .collect();
    ensure(&paths == hidden, || {
        answer
            .failure(format!("fields[] names {paths:?}, not {hidden:?}"))
            .0
    })
}

fn admitted_fields(
    answer: Result<Answer, Failure>,
    wrong: &BTreeSet<String>,
    shown: &BTreeSet<String>,
) -> Result<(), Failure> {
    let answer = answer?;
    match answer.json()?["error"]["code"].as_str() {
        Some(code) if code == ErrorCode::Validation.as_str() => {}
        Some(code) if code == ErrorCode::ForbiddenField.as_str() => {
            return Err(answer.failure("a field the descriptor shows is refused as forbidden"));
        }
        Some(code) => {
            return Err(answer.failure(format!("the call stopped at {code}, before validation")));
        }
        None => return Err(answer.failure("a call of mistyped fields was carried out")),
    }
    let error = answer.error(422, ErrorCode::Validation, VALIDATION)?;
    let paths: BTreeSet<String> = error
        .fields
        .iter()
        .map(|field| field.path.clone())
        .collect();
    let missed: Vec<&String> = wrong.difference(&paths).collect();
    ensure(missed.is_empty(), || {
        answer
            .failure(format!(
                "fields[] names {paths:?} and leaves out the mistyped {missed:?}"
            ))
            .0
    })?;
    let foreign: Vec<&String> = paths
        .iter()
        .filter(|path| !shown.contains(member(path)))
        .collect();
    ensure(foreign.is_empty(), || {
        answer
            .failure(format!(
                "fields[] names {foreign:?}, outside the fields the descriptor shows"
            ))
            .0
    })
}

#[cfg(test)]
mod tests {
    use keel_client::Response;
    use keel_client::http::{HeaderMap, HeaderValue, Method};

    use super::*;

    fn answer(status: u16, body: Value) -> Result<Answer, Failure> {
        let mut headers = HeaderMap::new();
        headers.insert("x-keel-request-id", HeaderValue::from_static("req-1"));
        let response = Response {
            url: "https://127.0.0.1:1500/api/v1/call/probe.set".into(),
            status,
            headers,
            body: serde_json::to_vec(&body).unwrap(),
        };
        Ok(Answer::new(
            Method::POST,
            "/api/v1/call/probe.set",
            response,
        ))
    }

    fn refusal(code: ErrorCode, key: &str, paths: &[&str]) -> Value {
        let fields: Vec<Value> = paths.iter().map(|path| json!({ "path": path })).collect();
        json!({ "ok": false, "error": { "code": code, "key": key, "requestId": "req-1", "fields": fields } })
    }

    #[test]
    fn a_hidden_field_passes_only_when_refused_as_forbidden_with_exactly_its_paths() {
        let hidden = BTreeSet::from(["account".to_owned()]);
        let key = "keel.error.forbidden_field";
        forbidden(
            answer(403, refusal(ErrorCode::ForbiddenField, key, &["account"])),
            &hidden,
        )
        .unwrap();
        assert!(
            forbidden(
                answer(
                    403,
                    refusal(ErrorCode::ForbiddenField, key, &["account", "name"])
                ),
                &hidden
            )
            .is_err()
        );
        assert!(
            forbidden(
                answer(
                    422,
                    refusal(ErrorCode::Validation, "keel.error.validation", &["name"])
                ),
                &hidden
            )
            .is_err()
        );
        let ok = json!({ "ok": true, "data": {}, "meta": { "requestId": "req-1" } });
        assert!(forbidden(answer(200, ok), &hidden).is_err());
    }

    #[test]
    fn a_shown_field_passes_only_when_the_call_stops_at_validation() {
        let shown = BTreeSet::from(["name".to_owned(), "count".to_owned(), "id".to_owned()]);
        let wrong = BTreeSet::from(["name".to_owned(), "count".to_owned()]);
        let admitted =
            |status: u16, body: Value| admitted_fields(answer(status, body), &wrong, &shown);
        admitted(
            422,
            refusal(ErrorCode::Validation, VALIDATION, &["count", "name"]),
        )
        .unwrap();
        admitted(
            422,
            refusal(ErrorCode::Validation, VALIDATION, &["count", "id", "name"]),
        )
        .unwrap();
        for (status, body) in [
            (
                422,
                refusal(
                    ErrorCode::Validation,
                    "svc.error.period",
                    &["count", "name"],
                ),
            ),
            (422, refusal(ErrorCode::Validation, VALIDATION, &["name"])),
            (
                422,
                refusal(
                    ErrorCode::Validation,
                    VALIDATION,
                    &["count", "name", "account"],
                ),
            ),
            (
                422,
                refusal(ErrorCode::Validation, VALIDATION, &["count", "name", ""]),
            ),
            (
                400,
                refusal(ErrorCode::Validation, VALIDATION, &["count", "name"]),
            ),
            (
                403,
                refusal(
                    ErrorCode::ForbiddenField,
                    "keel.error.forbidden_field",
                    &["name"],
                ),
            ),
            (
                404,
                refusal(ErrorCode::NotFound, "keel.error.not_found", &[]),
            ),
            (
                200,
                json!({ "ok": true, "data": {}, "meta": { "requestId": "req-1" } }),
            ),
        ] {
            assert!(admitted(status, body.clone()).is_err(), "{body}");
        }
        assert_eq!(member("records[0].ip"), "records");
        assert_eq!(member("nested.depth"), "nested");
        assert_eq!(member("name"), "name");
    }

    fn descriptor(name: &str, kind: &str, params: Value) -> Descriptor {
        serde_json::from_value(json!({
            "keel": 1,
            "name": name,
            "module": "probe",
            "kind": kind,
            "mode": "write",
            "access": { "level": "user" },
            "params": params,
            "result": {}
        }))
        .unwrap()
    }

    #[test]
    fn a_field_the_admin_sees_and_the_user_does_not_is_hidden_and_raw_routes_are_left_out() {
        let admin = json!({ "type": "object", "properties": { "name": { "type": "string" }, "account": { "type": "string" } } });
        let user = json!({ "type": "object", "properties": { "name": { "type": "string" } } });
        let wide = [
            descriptor("probe.set", "form", admin),
            descriptor("probe.file", "upload", json!({})),
        ];
        let narrow = [
            descriptor("probe.set", "form", user),
            descriptor("probe.file", "upload", json!({})),
            descriptor("probe.mine", "action", json!({ "type": "object" })),
        ];
        let found = cases(&wide, &narrow);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].function, "probe.set");
        assert_eq!(found[0].shown, BTreeSet::from(["name".to_owned()]));
        assert_eq!(found[0].hidden, BTreeSet::from(["account".to_owned()]));
        assert_eq!(found[1].function, "probe.mine");
        assert!(found[1].shown.is_empty() && found[1].hidden.is_empty());
    }

    #[test]
    fn every_shown_field_gets_a_value_outside_its_types_when_one_exists() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "minLength": 1 },
                "count": { "type": ["integer", "null"] },
                "ratio": { "type": "number" },
                "id": { "$ref": "#/$defs/Id" },
                "mode": { "enum": ["a", "b"] },
                "choice": { "anyOf": [{ "type": "string" }, { "type": "boolean" }] },
                "any": {},
                "deep": { "properties": { "x": { "type": "string" } } }
            },
            "$defs": { "Id": { "type": "string" } }
        });
        let shown: BTreeSet<String> = properties(&schema);
        let built = mistyped(&schema, &shown, None).unwrap();
        let params = &built.params;
        assert_eq!(built.count, 8);
        assert_eq!(params["name"], json!(true));
        assert_eq!(params["count"], json!(true));
        assert_eq!(params["ratio"], json!(true));
        assert_eq!(params["id"], json!(true));
        assert_eq!(params["mode"], json!(true));
        assert_eq!(params["choice"], json!({ PROBE: true }));
        assert_eq!(params["any"], json!(PROBE));
        assert_eq!(params["deep"], json!(PROBE));
        assert_eq!(
            built.wrong,
            BTreeSet::from(["choice", "count", "id", "mode", "name", "ratio"].map(str::to_owned))
        );
        let loose = json!({ "type": "object", "properties": { "any": true } });
        assert_eq!(mistyped(&loose, &properties(&loose), None), None);
        let keyed = mistyped(&schema, &shown, Some("id")).unwrap();
        assert_eq!(keyed.count, 7);
        assert!(keyed.params.get("id").is_none());
        assert!(!keyed.wrong.contains("id"));
        let only = json!({ "type": "object", "properties": { "id": { "type": "string" } } });
        assert_eq!(mistyped(&only, &properties(&only), Some("id")), None);
        let everything =
            BTreeSet::from(["boolean", "string", "object", "array", "number"].map(str::to_owned));
        assert_eq!(outside(&everything), None);
        let integers =
            BTreeSet::from(["boolean", "string", "object", "array", "integer"].map(str::to_owned));
        assert_eq!(outside(&integers), Some(json!(1.5)));
    }
}
