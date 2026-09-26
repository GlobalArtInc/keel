use argon2::Argon2;
use argon2::password_hash::PasswordHasher;
use keel::prelude::*;
use keel::sdk::tokio;
use keel::types::Check;

pub(crate) const LOCAL_PATTERN: &str = r"^[A-Za-z0-9]([A-Za-z0-9._-]{0,62}[A-Za-z0-9])?$";
const LOCAL_MAX: usize = 64;
pub(crate) const NAMES_MAX: usize = 16;
pub(crate) const PASSWORD_MIN: u32 = 8;
pub(crate) const PASSWORD_MAX: u32 = 128;
pub(crate) const QUOTA_MAX: u32 = 1_048_576;
pub(crate) const QUOTA_DEFAULT: u32 = 1024;
const SCHEME: &str = "{ARGON2ID}";
const DOMAIN: &str = "hosting.mail.error.domain";
const LOCAL: &str = "hosting.mail.error.local";
const FORWARD: &str = "hosting.mail.error.forward";
const TWICE: &str = "hosting.mail.error.twice";
const TOO_MANY: &str = "hosting.mail.error.too_many";
const PASSWORD: &str = "hosting.mail.error.password";
const HASHING: &str = "hosting.mail.error.hashing";

pub(crate) fn domain(value: &str) -> Result<String> {
    let name = value.to_ascii_lowercase();
    if Check::Domain.matches(&name) {
        Ok(name)
    } else {
        Err(KeelError::validation(DOMAIN)
            .field("name")
            .param("name", value))
    }
}

pub(crate) fn local(value: &str, field: &str) -> Result<String> {
    let name = value.to_ascii_lowercase();
    if plain_local(&name) {
        Ok(name)
    } else {
        Err(KeelError::validation(LOCAL)
            .field(field)
            .param("name", value))
    }
}

pub(crate) fn aliases(own: &str, values: &[String]) -> Result<Vec<String>> {
    within_count(values, "aliases")?;
    let mut kept: Vec<String> = Vec::with_capacity(values.len());
    for value in values {
        let name = local(value, "aliases")?;
        if name == own || kept.contains(&name) {
            return Err(KeelError::validation(TWICE)
                .field("aliases")
                .param("name", name));
        }
        kept.push(name);
    }
    Ok(kept)
}

pub(crate) fn forwards(address: &str, values: &[String]) -> Result<Vec<String>> {
    within_count(values, "forwards")?;
    let mut kept: Vec<String> = Vec::with_capacity(values.len());
    for value in values {
        let target = value.to_ascii_lowercase();
        if !plain_forward(&target) {
            return Err(KeelError::validation(FORWARD)
                .field("forwards")
                .param("name", value.as_str()));
        }
        if target == address || kept.contains(&target) {
            return Err(KeelError::validation(TWICE)
                .field("forwards")
                .param("name", target));
        }
        kept.push(target);
    }
    Ok(kept)
}

pub(crate) fn check_password(password: &Secret) -> Result<()> {
    if password.expose().chars().any(char::is_control) {
        Err(KeelError::validation(PASSWORD).field("password"))
    } else {
        Ok(())
    }
}

pub(crate) async fn hash(password: &Secret) -> Result<String> {
    let bytes = password.expose().as_bytes().to_vec();
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(&bytes)
            .map(|hash| format!("{SCHEME}{hash}"))
            .map_err(|error| KeelError::internal(HASHING).source(error))
    })
    .await
    .map_err(|error| KeelError::internal(HASHING).source(error))?
}

fn within_count(values: &[String], field: &str) -> Result<()> {
    if values.len() > NAMES_MAX {
        Err(KeelError::validation(TOO_MANY)
            .field(field)
            .param("max", NAMES_MAX))
    } else {
        Ok(())
    }
}

fn plain_local(name: &str) -> bool {
    let bytes = name.as_bytes();
    let edge = |byte: &u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    !bytes.is_empty()
        && bytes.len() <= LOCAL_MAX
        && bytes.first().is_some_and(edge)
        && bytes.last().is_some_and(edge)
        && !name.contains("..")
        && bytes
            .iter()
            .all(|byte| edge(byte) || matches!(byte, b'.' | b'_' | b'-'))
}

fn plain_forward(address: &str) -> bool {
    let Some((local, _)) = address.split_once('@') else {
        return false;
    };
    Check::Email.matches(address)
        && local.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'_' | b'-' | b'+')
        })
}

#[cfg(test)]
mod tests {
    use argon2::PasswordVerifier;
    use argon2::password_hash::phc::PasswordHash;

    use super::*;

    fn key(error: KeelError) -> String {
        error.to_call_error().fields[0]
            .key
            .clone()
            .unwrap_or_default()
    }

    #[test]
    fn a_local_part_is_lowercase_letters_digits_dots_underscores_and_dashes() {
        assert_eq!(local("Info", "local").unwrap(), "info");
        for good in ["a", "john.doe", "no-reply", "x_1", "9"] {
            assert_eq!(local(good, "local").unwrap(), good);
        }
        for bad in [
            "", ".a", "a.", "a..b", "a b", "a:b", "a@b", "a,b", "a\nb", "é", "-a",
        ] {
            let error = local(bad, "local").expect_err(bad).to_call_error();
            assert_eq!(error.fields[0].key.as_deref(), Some(LOCAL), "{bad}");
            assert_eq!(error.fields[0].path, "local", "{bad}");
        }
        assert!(local(&"a".repeat(65), "local").is_err());
    }

    #[test]
    fn aliases_and_forwards_refuse_the_mailbox_itself_repeats_and_separators() {
        assert_eq!(
            aliases("box", &["Info".to_owned(), "sales".to_owned()]).unwrap(),
            ["info", "sales"]
        );
        assert_eq!(key(aliases("box", &["box".to_owned()]).unwrap_err()), TWICE);
        assert_eq!(
            forwards("box@example.org", &["Me+tag@Example.NET".to_owned()]).unwrap(),
            ["me+tag@example.net"]
        );
        for bad in [
            "me@example.net, root@localhost",
            "me example.net",
            "|cmd@example.net",
            "me@example.net\nx",
            "no-at-sign",
            "me:x@example.net",
        ] {
            let error = forwards("box@example.org", &[bad.to_owned()]).expect_err(bad);
            assert_eq!(key(error), FORWARD, "{bad}");
        }
        assert_eq!(
            key(forwards("box@example.org", &["box@example.org".to_owned()]).unwrap_err()),
            TWICE
        );
        let many: Vec<String> = (0..=NAMES_MAX).map(|n| format!("a{n}")).collect();
        assert_eq!(key(aliases("box", &many).unwrap_err()), TOO_MANY);
    }

    #[test]
    fn a_mail_domain_is_a_lowercase_host_name() {
        assert_eq!(domain("Example.ORG").unwrap(), "example.org");
        for bad in ["example", "exa mple.org", "-a.org", "a..org"] {
            assert!(domain(bad).is_err(), "{bad}");
        }
    }

    #[keel::sdk::tokio::test(crate = "keel::sdk::tokio")]
    async fn the_password_is_hashed_as_argon2id_under_the_scheme_prefix_dovecot_reads() {
        let stored = hash(&Secret::new("correct horse")).await.unwrap();
        let phc = stored
            .strip_prefix("{ARGON2ID}")
            .expect("the scheme prefix");
        assert!(phc.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"), "{phc}");
        assert!(!stored.contains(':'), "{stored}");
        let parsed = PasswordHash::new(phc).unwrap();
        assert!(
            Argon2::default()
                .verify_password(b"correct horse", &parsed)
                .is_ok()
        );
        assert!(
            Argon2::default()
                .verify_password(b"wrong horse", &parsed)
                .is_err()
        );
        assert!(check_password(&Secret::new("line\nbreak")).is_err());
    }
}
