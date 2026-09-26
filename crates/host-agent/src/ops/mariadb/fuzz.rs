use proptest::prelude::*;
use proptest::test_runner::Config;

use super::{
    PASSWORD_LIMIT, account_of, grant_scope, identifier, literal, password_of, valid_name,
};

const CASES: u32 = 2048;
const PREFIX: &str = "hp_";

fn config() -> Config {
    let from_environment = Config::default();
    Config {
        cases: from_environment.cases.max(CASES),
        failure_persistence: None,
        ..from_environment
    }
}

fn unescaped(escaped: char) -> char {
    match escaped {
        '0' => '\0',
        'b' => '\u{8}',
        'n' => '\n',
        'r' => '\r',
        't' => '\t',
        'Z' => '\u{1a}',
        other => other,
    }
}

fn lex_literal(sql: &str) -> Option<(String, &str)> {
    let rest = sql.strip_prefix('\'')?;
    let mut value = String::new();
    let mut chars = rest.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        match c {
            '\\' => value.push(unescaped(chars.next()?.1)),
            '\'' if chars.peek().is_some_and(|(_, next)| *next == '\'') => {
                chars.next();
                value.push('\'');
            }
            '\'' => return Some((value, &rest[at + 1..])),
            other => value.push(other),
        }
    }
    None
}

fn tricky() -> impl Strategy<Value = char> {
    prop_oneof![
        4 => any::<char>(),
        3 => proptest::sample::select(vec!['\'', '\\', '"', '`', ';', '%', '_', '\n', '\r', '\0', '\u{1a}', '\u{bf}', '\u{5c}', '\u{a0}', '\u{2028}']),
        1 => proptest::sample::select(vec!['\u{7bf}', '\u{ffbf}', '\u{10ffff}', '\u{303f}']),
    ]
}

fn text(max: usize) -> impl Strategy<Value = String> {
    prop::collection::vec(tricky(), 0..max).prop_map(|chars| chars.into_iter().collect())
}

fn name() -> impl Strategy<Value = String> {
    prop_oneof![
        2 => "hp_[a-z0-9_-]{1,61}",
        1 => text(70).prop_map(|rest| format!("{PREFIX}{rest}")),
        1 => text(70),
    ]
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn an_accepted_password_reads_back_as_one_literal_that_ends_the_statement(password in text(PASSWORD_LIMIT + 8)) {
        match password_of(&password) {
            Ok(quoted) => {
                let statement = format!("CREATE USER {} IDENTIFIED BY {quoted};", account_of("hp_alice_app", PREFIX).unwrap());
                let tail = statement
                    .strip_prefix("CREATE USER 'hp_alice_app'@'localhost' IDENTIFIED BY ")
                    .unwrap();
                let (value, rest) = lex_literal(tail).unwrap();
                prop_assert_eq!(value, password.clone());
                prop_assert_eq!(rest, ";");
            }
            Err(_) => prop_assert!(
                password.is_empty()
                    || password.len() > PASSWORD_LIMIT
                    || password.chars().any(char::is_control)
            ),
        }
    }

    #[test]
    fn any_value_reads_back_from_its_literal(value in text(128)) {
        let quoted = literal(&value);
        let (read, rest) = lex_literal(&quoted).unwrap();
        prop_assert_eq!(read, value);
        prop_assert_eq!(rest, "");
    }

    #[test]
    fn an_accepted_name_needs_no_quoting_and_its_grant_scope_matches_only_itself(value in name()) {
        if valid_name(&value, PREFIX) {
            prop_assert!(value.starts_with(PREFIX) && value.len() <= 64);
            prop_assert!(value.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-'));
            prop_assert_eq!(identifier(&value), format!("`{value}`"));
            let scope = grant_scope(&value, PREFIX).unwrap();
            let inner = scope.strip_prefix('`').and_then(|rest| rest.strip_suffix('`')).unwrap();
            prop_assert_eq!(inner.replace("\\_", "_"), value.clone());
            prop_assert!(!inner.replace("\\_", "").contains('_'));
            prop_assert_eq!(account_of(&value, PREFIX).unwrap(), format!("'{value}'@'localhost'"));
        } else {
            prop_assert!(grant_scope(&value, PREFIX).is_err());
            prop_assert!(account_of(&value, PREFIX).is_err());
        }
    }
}
