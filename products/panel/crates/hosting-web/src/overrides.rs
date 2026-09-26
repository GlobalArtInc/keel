use keel::prelude::*;

pub(crate) const SERVER: &str = "server";
pub(crate) const POOL: &str = "pool";

const DIRECTIVE: &str = "hosting.site.config.error.directive";
const SETTING: &str = "hosting.site.config.error.setting";
const BLOCK: &str = "hosting.site.config.error.block";
const DEPTH_MAX: usize = 2;
const INI_MAX: usize = 64;

const BLOCKS: [&str; 2] = ["location", "if"];
const DIRECTIVES: [&str; 47] = [
    "absolute_redirect",
    "add_header",
    "add_trailer",
    "allow",
    "autoindex",
    "autoindex_exact_size",
    "autoindex_format",
    "autoindex_localtime",
    "break",
    "charset",
    "client_body_buffer_size",
    "client_body_timeout",
    "client_max_body_size",
    "default_type",
    "deny",
    "error_page",
    "etag",
    "expires",
    "fastcgi_buffer_size",
    "fastcgi_buffers",
    "fastcgi_connect_timeout",
    "fastcgi_hide_header",
    "fastcgi_intercept_errors",
    "fastcgi_read_timeout",
    "fastcgi_send_timeout",
    "gzip",
    "gzip_comp_level",
    "gzip_min_length",
    "gzip_proxied",
    "gzip_types",
    "gzip_vary",
    "if_modified_since",
    "index",
    "internal",
    "keepalive_timeout",
    "limit_rate",
    "limit_rate_after",
    "log_not_found",
    "port_in_redirect",
    "return",
    "rewrite",
    "satisfy",
    "send_timeout",
    "server_name_in_redirect",
    "server_tokens",
    "set",
    "try_files",
];
const POOL_KEYS: [&str; 11] = [
    "catch_workers_output",
    "clear_env",
    "decorate_workers_output",
    "pm",
    "pm.max_children",
    "pm.max_requests",
    "pm.max_spare_servers",
    "pm.min_spare_servers",
    "pm.process_idle_timeout",
    "pm.start_servers",
    "request_terminate_timeout",
];
const INI_KINDS: [&str; 4] = ["php_value", "php_flag", "php_admin_value", "php_admin_flag"];
const INI_REFUSED: [&str; 3] = ["extension", "extension_dir", "zend_extension"];

enum Shape {
    Plain,
    Open,
    Close,
}

fn lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines
}

pub(crate) fn server(text: &str) -> Result<Vec<&str>> {
    let lines = lines(text);
    let mut open: Vec<usize> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        match server_line(line.trim()) {
            Some(Shape::Plain) => {}
            Some(Shape::Open) if open.len() < DEPTH_MAX => open.push(index),
            Some(Shape::Close) if open.pop().is_some() => {}
            _ => return Err(refused(SERVER, DIRECTIVE, index)),
        }
    }
    match open.first() {
        Some(index) => Err(refused(SERVER, BLOCK, *index)),
        None => Ok(lines),
    }
}

pub(crate) fn pool(text: &str) -> Result<Vec<&str>> {
    let lines = lines(text);
    for (index, line) in lines.iter().enumerate() {
        if !pool_line(line.trim()) {
            return Err(refused(POOL, SETTING, index));
        }
    }
    Ok(lines)
}

fn refused(field: &str, key: &str, index: usize) -> KeelError {
    KeelError::validation(key)
        .field(field)
        .param("line", index + 1)
}

fn server_line(line: &str) -> Option<Shape> {
    if line.is_empty() || line.starts_with('#') {
        return Some(Shape::Plain);
    }
    if line.contains("..") {
        return None;
    }
    if line == "}" {
        return Some(Shape::Close);
    }
    if let Some(head) = line.strip_suffix('{') {
        let words = words(head)?;
        let (name, args) = words.split_first()?;
        return (BLOCKS.contains(name) && !args.is_empty()).then_some(Shape::Open);
    }
    let words = words(line.strip_suffix(';')?)?;
    let (name, _) = words.split_first()?;
    DIRECTIVES.contains(name).then_some(Shape::Plain)
}

fn words(text: &str) -> Option<Vec<&str>> {
    let bytes = text.as_bytes();
    let mut words = Vec::new();
    let mut start: Option<usize> = None;
    let mut quote: Option<u8> = None;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        match quote {
            Some(open) if byte == open => {
                if bytes.get(index + 1).is_some_and(|next| !is_space(*next)) {
                    return None;
                }
                quote = None;
            }
            Some(_) if byte == b'\\' => return None,
            Some(_) => {}
            None if is_space(byte) => {
                if let Some(from) = start.take() {
                    words.push(&text[from..index]);
                }
            }
            None => match byte {
                b';' | b'{' | b'}' | b'#' => return None,
                b'"' | b'\'' if start.is_none() => {
                    start = Some(index);
                    quote = Some(byte);
                }
                b'"' | b'\'' => return None,
                b'\\' => {
                    if !bytes.get(index + 1).copied().is_some_and(escapable) {
                        return None;
                    }
                    start.get_or_insert(index);
                    index += 1;
                }
                _ => {
                    start.get_or_insert(index);
                }
            },
        }
        index += 1;
    }
    if quote.is_some() {
        return None;
    }
    if let Some(from) = start {
        words.push(&text[from..]);
    }
    Some(words)
}

const fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r')
}

fn escapable(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b".^$*+?()[]|/-_=:,!%@&~<>\\".contains(&byte)
}

fn pool_line(line: &str) -> bool {
    if line.is_empty() || line.starts_with(';') {
        return true;
    }
    let Some((key, value)) = line.split_once('=') else {
        return false;
    };
    let value = value.trim();
    pool_key(key.trim()) && !value.is_empty() && !value.chars().any(char::is_control)
}

fn pool_key(key: &str) -> bool {
    POOL_KEYS.contains(&key)
        || INI_KINDS.iter().any(|kind| {
            key.strip_prefix(kind)
                .and_then(|rest| rest.strip_prefix('['))
                .and_then(|rest| rest.strip_suffix(']'))
                .is_some_and(|name| ini_name(name) && !INI_REFUSED.contains(&name))
        })
}

fn ini_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= INI_MAX
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.'))
}

#[cfg(test)]
mod fuzz;

#[cfg(test)]
mod tests {
    use super::*;

    fn refused_at(result: Result<Vec<&str>>) -> (String, String, Value) {
        let error = result.expect_err("the override is refused");
        (
            error.key().to_owned(),
            error.field_path().unwrap_or_default().to_owned(),
            error.params()["line"].clone(),
        )
    }

    #[test]
    fn the_lines_of_a_configuration_drop_trailing_blanks_and_carriage_returns() {
        assert_eq!(
            lines("client_max_body_size 64m;  \r\n\r\nlocation /x {\n}\n\n  \n"),
            ["client_max_body_size 64m;", "", "location /x {", "}"]
        );
        assert!(lines("").is_empty());
    }

    #[test]
    fn a_server_override_takes_the_allowed_directives_blocks_and_comments() {
        let text = "client_max_body_size 64m;\n# a note\nadd_header Content-Security-Policy \"default-src 'self'; img-src *\" always;\nlocation ~ ^/api/(.*)\\.json$ {\n    if ($request_method = POST) {\n        return 405;\n    }\n    rewrite ^/api/(.*)$ /index.php?q=$1 last;\n}\nexpires 30d;\n";
        assert_eq!(server(text).expect("the override passes").len(), 10);
        assert!(server("").expect("an empty override").is_empty());
    }

    #[test]
    fn a_server_override_naming_a_file_a_socket_a_name_or_a_new_block_is_refused_on_its_line() {
        for (text, line) in [
            ("gzip on;\naccess_log /root/.profile;", 2),
            ("error_log /etc/cron.d/keel;", 1),
            ("include /etc/shadow;", 1),
            ("alias /etc/;", 1),
            ("root /;", 1),
            ("ssl_certificate_key /etc/ssl/keel/other.key;", 1),
            ("fastcgi_pass unix:/run/keel/keel.sock;", 1),
            ("proxy_pass http://127.0.0.1:8443;", 1),
            ("server_name bank.example;", 1),
            ("client_body_temp_path /etc;", 1),
            ("fastcgi_param PHP_ADMIN_VALUE open_basedir=/;", 1),
            ("gzip on; access_log /tmp/x;", 1),
            ("gzip on", 1),
            ("}\nserver {", 1),
            ("server {\n}", 1),
            ("add_header X \"a\nb\";", 1),
            ("add_header X a\"; access_log /tmp/x;\";", 1),
            ("add_header X \"a\\\"; access_log /tmp/x;\";", 1),
            ("add_header X a\\;access_log;", 1),
            ("add_header X y #;", 1),
            ("location /a {\nlocation /b {\nlocation /c {\n}\n}\n}", 3),
            ("location {", 1),
            ("add_header X ${uri};", 1),
            ("try_files $uri /../../etc/passwd =404;", 1),
            ("location ~ ^/(..)$ {", 1),
        ] {
            let (key, field, at) = refused_at(server(text));
            assert_eq!(
                (key.as_str(), field.as_str()),
                (DIRECTIVE, SERVER),
                "{text:?}"
            );
            assert_eq!(at, json!(line), "{text:?}");
        }
        let (key, field, at) = refused_at(server("gzip on;\nlocation /a {\n    return 204;"));
        assert_eq!(
            (key.as_str(), field.as_str(), at),
            (BLOCK, SERVER, json!(2))
        );
    }

    #[test]
    fn a_pool_override_takes_process_settings_and_ini_values_but_no_section_file_or_identity() {
        let text = "pm.max_children = 8\n; a note\nphp_admin_value[memory_limit] = 256M\nphp_flag[display_errors] = off\nrequest_terminate_timeout = 60s";
        assert_eq!(pool(text).expect("the override passes").len(), 5);
        for (text, line) in [
            ("pm.max_children = 8\n[www]", 2),
            ("user = root", 1),
            ("group = root", 1),
            ("listen = /run/keel/keel.sock", 1),
            ("chroot = /", 1),
            ("access.log = /etc/cron.d/keel", 1),
            ("slowlog = /root/.profile", 1),
            ("include = /etc/keel/*.conf", 1),
            ("security.limit_extensions = .php .txt", 1),
            ("php_admin_value[extension] = /tmp/x.so", 1),
            ("php_value[zend_extension] = /tmp/x.so", 1),
            ("php_value[a b] = 1", 1),
            ("env[LD_PRELOAD] = /tmp/x.so", 1),
            ("pm.max_children =", 1),
            ("pm.max_children", 1),
        ] {
            let (key, field, at) = refused_at(pool(text));
            assert_eq!((key.as_str(), field.as_str()), (SETTING, POOL), "{text:?}");
            assert_eq!(at, json!(line), "{text:?}");
        }
    }
}
