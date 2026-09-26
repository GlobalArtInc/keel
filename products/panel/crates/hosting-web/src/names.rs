use keel::prelude::*;
use keel::types::Check;

pub(crate) const ALIASES_MAX: usize = 16;
pub(crate) const ROOT_MAX: usize = 200;
pub(crate) const ROOT_PATTERN: &str =
    r"^[A-Za-z0-9_][A-Za-z0-9._-]*(/[A-Za-z0-9_][A-Za-z0-9._-]*)*$";
pub(crate) const PHP_PATTERN: &str = r"^[0-9]{1,2}\.[0-9]{1,2}$";

const NAME: &str = "hosting.site.error.name";
const ALIAS_TWICE: &str = "hosting.site.error.alias_twice";
const ROOT: &str = "hosting.site.error.root";
const PHP: &str = "hosting.site.error.php";
const HOME: &str = "hosting.site.error.home";

pub(crate) fn host_name(value: &str, field: &str) -> Result<String> {
    let name = value.to_ascii_lowercase();
    if Check::Domain.matches(&name) {
        Ok(name)
    } else {
        Err(KeelError::validation(NAME)
            .field(field)
            .param("name", value))
    }
}

pub(crate) fn aliases(domain: &str, values: &[String]) -> Result<Vec<String>> {
    if values.len() > ALIASES_MAX {
        return Err(KeelError::validation(NAME)
            .field("aliases")
            .param("name", format!("{} names", values.len())));
    }
    let mut kept: Vec<String> = Vec::with_capacity(values.len());
    for value in values {
        let name = host_name(value, "aliases")?;
        if name == domain || kept.contains(&name) {
            return Err(KeelError::validation(ALIAS_TWICE)
                .field("aliases")
                .param("name", name));
        }
        kept.push(name);
    }
    Ok(kept)
}

pub(crate) fn root(value: &str) -> Result<String> {
    if value.len() <= ROOT_MAX && is_relative(value) {
        Ok(value.to_owned())
    } else {
        Err(KeelError::validation(ROOT)
            .field("root")
            .param("root", value))
    }
}

pub(crate) fn php(value: Option<&str>) -> Result<Option<String>> {
    match value {
        None => Ok(None),
        Some(version) if is_version(version) => Ok(Some(version.to_owned())),
        Some(version) => Err(KeelError::validation(PHP)
            .field("php")
            .param("php", version)),
    }
}

pub(crate) fn home(value: &str) -> Result<&str> {
    let plain = value.strip_prefix('/').is_some_and(|rest| {
        rest.split('/').all(|segment| {
            !matches!(segment, "" | "." | "..")
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
    });
    if plain {
        Ok(value)
    } else {
        Err(KeelError::internal(HOME).param("home", value))
    }
}

fn is_relative(value: &str) -> bool {
    !value.is_empty()
        && value.split('/').all(|segment| {
            let mut bytes = segment.bytes();
            bytes
                .next()
                .is_some_and(|first| first.is_ascii_alphanumeric() || first == b'_')
                && bytes
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
}

fn is_version(value: &str) -> bool {
    value.split_once('.').is_some_and(|(major, minor)| {
        [major, minor].iter().all(|part| {
            (1..=2).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_digit())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_name_is_lowercased_and_anything_else_is_refused_on_its_field() {
        assert_eq!(host_name("Example.COM", "domain").unwrap(), "example.com");
        for bad in [
            "example.com;",
            "a b.com",
            "example.com\n",
            "*.example.com",
            "example",
            "exa$mple.com",
            "{x}.com",
        ] {
            let error = host_name(bad, "domain").unwrap_err();
            assert_eq!(error.key(), NAME, "{bad}");
        }
    }

    #[test]
    fn aliases_are_host_names_other_than_the_domain_and_each_other() {
        assert_eq!(
            aliases("example.com", &["WWW.example.com".to_owned()]).unwrap(),
            ["www.example.com"]
        );
        assert_eq!(
            aliases("example.com", &["example.com".to_owned()])
                .unwrap_err()
                .key(),
            ALIAS_TWICE
        );
        assert_eq!(
            aliases("example.com", &["a.org".to_owned(), "A.org".to_owned()])
                .unwrap_err()
                .key(),
            ALIAS_TWICE
        );
        assert_eq!(
            aliases("example.com", &["a.org\"; include /etc/passwd".to_owned()])
                .unwrap_err()
                .key(),
            NAME
        );
        let many: Vec<String> = (0..=ALIASES_MAX).map(|n| format!("a{n}.org")).collect();
        assert!(aliases("example.com", &many).is_err());
    }

    #[test]
    fn a_root_is_a_relative_path_of_plain_segments() {
        assert_eq!(root("example.com/public").unwrap(), "example.com/public");
        for bad in [
            "", "/etc", "../x", "a/../b", "a//b", ".hidden", "a/", "a b", "a;b",
        ] {
            assert!(root(bad).is_err(), "{bad}");
        }
        assert!(root(&"a".repeat(ROOT_MAX + 1)).is_err());
    }

    #[test]
    fn a_php_version_is_major_dot_minor() {
        assert_eq!(php(None).unwrap(), None);
        assert_eq!(php(Some("8.4")).unwrap().as_deref(), Some("8.4"));
        for bad in ["8", "8.4.1", "x.4", "8.", "..", "123.4"] {
            assert!(php(Some(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_home_is_an_absolute_path_of_plain_segments() {
        assert_eq!(
            home("/var/www/keel-panel-alice").unwrap(),
            "/var/www/keel-panel-alice"
        );
        assert_eq!(home("/tmp/.tmpA1/www/x").unwrap(), "/tmp/.tmpA1/www/x");
        for bad in [
            "var/www",
            "/",
            "/var/www/",
            "/var/../etc",
            "/var/./etc",
            "/var/www;",
            "/var/ www",
        ] {
            assert!(home(bad).is_err(), "{bad}");
        }
    }
}
