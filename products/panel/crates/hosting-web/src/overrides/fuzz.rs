use proptest::prelude::*;
use proptest::test_runner::Config;

use super::{BLOCKS, DEPTH_MAX, DIRECTIVES, pool, server};
use crate::names::{ALIASES_MAX, aliases, host_name};

const CASES: u32 = 2048;
const NAMES: [&str; 12] = [
    "add_header",
    "return",
    "rewrite",
    "try_files",
    "location",
    "if",
    "access_log",
    "include",
    "server_name",
    "root",
    "server",
    "fastcgi_pass",
];
const PIECES: [&str; 22] = [
    " ",
    "\t",
    ";",
    "{",
    "}",
    "#",
    "\"",
    "'",
    "\\",
    r"\;",
    "$",
    "${uri}",
    "..",
    "/etc/shadow",
    "on",
    "64m",
    "^/(.*)$",
    "=",
    "~",
    "'self'",
    "\n",
    "\r",
];
const NGINX_SPECIAL: &[u8] = b" \t\r\n;{}#\"'\\$";

#[derive(Debug, PartialEq)]
enum Event {
    Directive(Vec<String>),
    Open(Vec<String>),
    Close,
}

fn config() -> Config {
    let from_environment = Config::default();
    Config {
        cases: from_environment.cases.max(CASES),
        failure_persistence: None,
        ..from_environment
    }
}

fn nginx_events(text: &str) -> Result<Vec<Event>, String> {
    let mut events = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut last_space = true;
    let mut need_space = false;
    let mut comment = false;
    let mut escaped = false;
    let mut variable = false;
    let mut quote: Option<char> = None;
    for ch in text.chars() {
        if comment {
            comment = ch != '\n';
            continue;
        }
        if escaped {
            escaped = false;
            word.push(ch);
            continue;
        }
        if need_space {
            match ch {
                ' ' | '\t' | '\r' | '\n' => {
                    last_space = true;
                    need_space = false;
                    continue;
                }
                ';' => {
                    events.push(Event::Directive(std::mem::take(&mut words)));
                    last_space = true;
                    need_space = false;
                    continue;
                }
                '{' => {
                    events.push(Event::Open(std::mem::take(&mut words)));
                    last_space = true;
                    need_space = false;
                    continue;
                }
                other => return Err(format!("unexpected {other:?} after a quoted word")),
            }
        }
        if last_space {
            match ch {
                ' ' | '\t' | '\r' | '\n' => {}
                ';' | '{' if words.is_empty() => return Err(format!("unexpected {ch:?}")),
                ';' => events.push(Event::Directive(std::mem::take(&mut words))),
                '{' => events.push(Event::Open(std::mem::take(&mut words))),
                '}' if !words.is_empty() => return Err("unexpected }".to_owned()),
                '}' => events.push(Event::Close),
                '#' => comment = true,
                '\\' => {
                    escaped = true;
                    last_space = false;
                }
                '"' | '\'' => {
                    quote = Some(ch);
                    last_space = false;
                }
                '$' => {
                    variable = true;
                    last_space = false;
                    word.push(ch);
                }
                other => {
                    last_space = false;
                    word.push(other);
                }
            }
            continue;
        }
        if ch == '{' && variable {
            word.push(ch);
            continue;
        }
        variable = false;
        if let Some(open) = quote {
            if ch == '\\' {
                escaped = true;
            } else if ch == open {
                quote = None;
                need_space = true;
                words.push(std::mem::take(&mut word));
            } else {
                word.push(ch);
            }
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '$' => {
                variable = true;
                word.push(ch);
            }
            ' ' | '\t' | '\r' | '\n' | ';' | '{' => {
                words.push(std::mem::take(&mut word));
                last_space = true;
                match ch {
                    ';' => events.push(Event::Directive(std::mem::take(&mut words))),
                    '{' => events.push(Event::Open(std::mem::take(&mut words))),
                    _ => {}
                }
            }
            other => word.push(other),
        }
    }
    if !words.is_empty() || !word.is_empty() || quote.is_some() || escaped {
        return Err("the text ends inside a directive".to_owned());
    }
    Ok(events)
}

fn server_block(lines: &[&str]) -> String {
    let mut block = String::from("server {\n    server_name site.example;\n");
    for line in lines {
        if !line.is_empty() {
            block.push_str("    ");
            block.push_str(line);
        }
        block.push('\n');
    }
    block.push_str("    index index.html;\n}\n");
    block
}

fn holds_the_server_block(lines: &[&str]) -> Result<(), String> {
    let events = nginx_events(&server_block(lines))?;
    let mut depth = 0usize;
    for (index, event) in events.iter().enumerate() {
        match event {
            Event::Open(words) if index == 0 => {
                if words != &["server"] {
                    return Err(format!("the block opens with {words:?}"));
                }
                depth = 1;
            }
            Event::Open(words) => {
                if depth == 0 || !BLOCKS.contains(&words[0].as_str()) || depth > DEPTH_MAX {
                    return Err(format!("{words:?} opens a block at depth {depth}"));
                }
                depth += 1;
            }
            Event::Close => {
                depth = depth.checked_sub(1).ok_or("a } closes nothing")?;
                if depth == 0 && index + 1 != events.len() {
                    return Err("the server block closes before its end".to_owned());
                }
            }
            Event::Directive(words) => {
                if depth == 0 {
                    return Err(format!("{words:?} stands outside the server block"));
                }
                let name = words[0].as_str();
                let built = index == 1 || index + 2 == events.len();
                if !built && !DIRECTIVES.contains(&name) {
                    return Err(format!("{name} passed as a directive"));
                }
                if words
                    .iter()
                    .any(|word| word.contains('\n') || word.contains(".."))
                {
                    return Err(format!("{words:?} carries a line break or a climb"));
                }
            }
        }
    }
    if depth == 0 {
        Ok(())
    } else {
        Err(format!("the server block is left open at depth {depth}"))
    }
}

fn noisy_line() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop_oneof![
            3 => proptest::sample::select(PIECES.to_vec()).prop_map(str::to_owned),
            2 => proptest::sample::select(NAMES.to_vec()).prop_map(str::to_owned),
            1 => "[a-z0-9/._-]{1,8}",
        ],
        0..10,
    )
    .prop_map(|pieces| pieces.concat())
}

fn shaped_line() -> impl Strategy<Value = String> {
    (
        proptest::sample::select(NAMES.to_vec()),
        prop::collection::vec(noisy_line(), 0..3),
        proptest::sample::select(vec![";", " {", "", "}"]),
    )
        .prop_map(|(name, args, end)| format!("{name} {}{end}", args.join(" ")))
}

fn override_text() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop_oneof![shaped_line(), noisy_line(), Just("}".to_owned())],
        0..8,
    )
    .prop_map(|lines| lines.join("\n"))
}

fn host_like() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => "[a-zA-Z0-9-]{1,12}(\\.[a-zA-Z0-9-]{1,12}){1,3}",
        1 => prop::collection::vec(proptest::sample::select(PIECES.to_vec()), 1..6).prop_map(|pieces| format!("a{}.org", pieces.concat())),
        1 => any::<String>(),
    ]
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn an_accepted_server_override_stays_inside_the_server_block_and_names_only_allowed_directives(text in override_text()) {
        if let Ok(lines) = server(&text) {
            prop_assert_eq!(holds_the_server_block(&lines), Ok(()), "{:?}", text);
        }
    }

    #[test]
    fn an_accepted_pool_override_never_opens_a_section_or_sets_an_identity(text in override_text()) {
        if let Ok(lines) = pool(&text.replace(';', "=")) {
            for line in lines.iter().map(|line| line.trim()) {
                prop_assert!(!line.starts_with('['), "{:?}", line);
                let key = line.split_once('=').map_or("", |(key, _)| key.trim());
                prop_assert!(!["user", "group", "listen", "chroot", "chdir", "include", "access.log", "slowlog"].contains(&key), "{:?}", line);
            }
        }
    }

    #[test]
    fn an_accepted_domain_and_its_aliases_carry_no_byte_nginx_reads_as_syntax(
        domain in host_like(),
        others in prop::collection::vec(host_like(), 0..ALIASES_MAX + 2),
    ) {
        if let Ok(name) = host_name(&domain, "domain") {
            prop_assert!(name.bytes().all(|byte| !NGINX_SPECIAL.contains(&byte) && !byte.is_ascii_uppercase()), "{:?}", name);
            if let Ok(kept) = aliases(&name, &others) {
                prop_assert!(kept.len() <= ALIASES_MAX);
                for alias in &kept {
                    prop_assert!(alias != &name);
                    prop_assert!(alias.bytes().all(|byte| !NGINX_SPECIAL.contains(&byte) && !byte.is_ascii_uppercase()), "{:?}", alias);
                }
                let line = format!("server_name {name} {};", kept.join(" "));
                let events = nginx_events(&line).map_err(TestCaseError::fail)?;
                prop_assert_eq!(events.len(), 1);
                if let Event::Directive(words) = &events[0] {
                    prop_assert_eq!(words.len(), kept.len() + 2);
                } else {
                    prop_assert!(false, "{:?}", events);
                }
            }
        }
    }
}

#[test]
fn the_nginx_model_reads_quotes_escapes_comments_and_blocks_like_nginx() {
    assert_eq!(
        nginx_events("a \"b c\" d\\;e;\n# x;\nf { g 'h'; }").unwrap(),
        [
            Event::Directive(vec!["a".into(), "b c".into(), "d;e".into()]),
            Event::Open(vec!["f".into()]),
            Event::Directive(vec!["g".into(), "h".into()]),
            Event::Close,
        ]
    );
    assert!(nginx_events("a \"b\"c;").is_err());
    assert!(nginx_events("a \"b;\n").is_err());
    assert_eq!(
        nginx_events("set $x ${uri}a;").unwrap(),
        [Event::Directive(vec![
            "set".into(),
            "$x".into(),
            "${uri}a".into()
        ])]
    );
    assert_eq!(
        holds_the_server_block(&["access_log /tmp/x;"]),
        Err("access_log passed as a directive".to_owned())
    );
}
