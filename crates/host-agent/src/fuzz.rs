use proptest::prelude::*;
use proptest::test_runner::Config;
use serde_json::{Map, Value, json};

use crate::protocol::{ERROR_FRAME, FIELD_LIMIT, LINE_LIMIT, Request, parse_request};

const CASES: u32 = 4096;
const FRAME_KEYS: [&str; 9] = [
    "id",
    "op",
    "actor",
    "login",
    "level",
    "request_id",
    "product",
    "args",
    "extra",
];

fn config() -> Config {
    let from_environment = Config::default();
    Config {
        cases: from_environment.cases.max(CASES),
        failure_persistence: None,
        ..from_environment
    }
}

fn acceptable(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= FIELD_LIMIT
        && !value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
}

fn holds_the_frame_rules(line: &[u8]) {
    match parse_request(line) {
        Ok(request) => {
            for value in [
                &request.op,
                &request.actor.login,
                &request.actor.level,
                &request.request_id,
                &request.product,
            ] {
                assert!(acceptable(value), "{value:?} passed the parser");
            }
        }
        Err((_, failure)) => assert_eq!(failure.key, ERROR_FRAME),
    }
}

fn key() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => proptest::sample::select(FRAME_KEYS.to_vec()).prop_map(str::to_owned),
        1 => any::<String>(),
    ]
}

fn json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<u64>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        any::<f64>().prop_map(Value::from),
        field().prop_map(Value::String),
    ];
    leaf.prop_recursive(6, 48, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            prop::collection::btree_map(key(), inner, 0..6)
                .prop_map(|map| Value::Object(map.into_iter().collect::<Map<_, _>>())),
        ]
    })
}

fn field() -> impl Strategy<Value = String> {
    let character = prop_oneof![
        12 => (b'a'..=b'z').prop_map(char::from),
        2 => proptest::sample::select(vec!['.', '-', '_', '=', '"', '\\']),
        1 => proptest::sample::select(vec![' ', '\t', '\n', '\r', '\u{0}', '\u{7}', '\u{85}', '\u{2028}', '\u{3000}']),
        1 => any::<char>(),
    ];
    prop_oneof![
        8 => prop::collection::vec(character, 0..24).prop_map(String::from_iter),
        1 => (FIELD_LIMIT - 1..FIELD_LIMIT + 3).prop_map(|length| "a".repeat(length)),
    ]
}

fn frame() -> impl Strategy<Value = (u64, [String; 5], Value)> {
    (
        any::<u64>(),
        [field(), field(), field(), field(), field()],
        json_value(),
    )
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn any_bytes_parse_to_a_request_or_fail_as_a_frame(line in prop::collection::vec(any::<u8>(), 0..1024)) {
        holds_the_frame_rules(&line);
    }

    #[test]
    fn any_json_parses_to_a_request_or_fails_as_a_frame(value in json_value()) {
        holds_the_frame_rules(value.to_string().as_bytes());
    }

    #[test]
    fn a_frame_is_accepted_exactly_when_its_fields_follow_the_rules(
        (id, [op, login, level, request_id, product], args) in frame()
    ) {
        let line = json!({
            "id": id,
            "op": op,
            "actor": { "login": login, "level": level },
            "request_id": request_id,
            "product": product,
            "args": args,
        })
        .to_string();
        prop_assume!(line.len() <= LINE_LIMIT);
        let expected = [&op, &login, &level, &request_id, &product]
            .into_iter()
            .all(|value| acceptable(value));
        match parse_request(line.as_bytes()) {
            Ok(request) => {
                prop_assert!(expected);
                let parsed: Request = serde_json::from_str(&line).unwrap();
                prop_assert_eq!(request, parsed);
            }
            Err((answered, failure)) => {
                prop_assert!(!expected);
                prop_assert_eq!(answered, id);
                prop_assert_eq!(failure.key, ERROR_FRAME);
            }
        }
    }

    #[test]
    fn a_design_frame_with_its_bytes_changed_never_passes_a_broken_field(
        edits in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>(), 0..3u8), 1..8)
    ) {
        let mut line = br#"{"id":1,"op":"host.template.apply","actor":{"login":"alice","level":"user"},"request_id":"req-1a2b","product":"dns","args":{"template":"bind/zone.j2","vars":{"a":1},"dest":{"root":"zones","name":"keel-dns-a.org.zone"},"mode":"0640"}}"#.to_vec();
        for (index, byte, kind) in edits {
            let at = index.index(line.len().max(1));
            match kind {
                0 => line[at] = byte,
                1 => line.insert(at, byte),
                _ => {
                    line.remove(at);
                }
            }
            if line.is_empty() {
                line.push(byte);
            }
        }
        holds_the_frame_rules(&line);
    }
}
