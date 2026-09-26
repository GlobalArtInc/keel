use keel::prelude::*;
use keel::sdk::tokio;
use rand::TryRng;
use rand::rngs::SysRng;
use sha_crypt::{PasswordHasher, ShaCrypt};

pub(crate) const NAME: &str = r"^[a-z][a-z0-9_-]{0,28}$";
pub(crate) const NAME_MAX: u32 = 29;
pub(crate) const DIRECTORY: &str =
    r"^$|^[A-Za-z0-9_][A-Za-z0-9._-]*(/[A-Za-z0-9_][A-Za-z0-9._-]*)*$";
pub(crate) const DIRECTORY_MAX: usize = 200;
pub(crate) const PASSWORD_MIN: u32 = 8;
pub(crate) const PASSWORD_MAX: u32 = 128;

const LOGIN_PREFIX: &str = "hp_";
const SALT_BYTES: usize = 12;
const NAME_ERROR: &str = "hosting.ftp.error.name";
const DIRECTORY_ERROR: &str = "hosting.ftp.error.directory";
const PASSWORD: &str = "hosting.ftp.error.password";
const HASHING: &str = "hosting.ftp.error.hashing";

pub(crate) fn login(name: &str) -> Result<String> {
    let mut bytes = name.bytes();
    let plain = name.len() <= NAME_MAX as usize
        && bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        && bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        });
    if plain {
        Ok(format!("{LOGIN_PREFIX}{name}"))
    } else {
        Err(KeelError::validation(NAME_ERROR)
            .field("name")
            .param("name", name))
    }
}

pub(crate) fn directory(value: &str) -> Result<&str> {
    if value.is_empty() || (value.len() <= DIRECTORY_MAX && is_relative(value)) {
        Ok(value)
    } else {
        Err(KeelError::validation(DIRECTORY_ERROR)
            .field("directory")
            .param("directory", value))
    }
}

pub(crate) fn chroot(root: &str, directory: &str) -> String {
    if directory.is_empty() {
        root.to_owned()
    } else {
        format!("{root}/{directory}")
    }
}

fn is_relative(value: &str) -> bool {
    value.split('/').all(|segment| {
        let mut bytes = segment.bytes();
        bytes
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric() || first == b'_')
            && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    })
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
        let mut salt = [0_u8; SALT_BYTES];
        SysRng
            .try_fill_bytes(&mut salt)
            .map_err(|error| KeelError::internal(HASHING).param("detail", error.to_string()))?;
        ShaCrypt::SHA512
            .hash_password_with_salt(&bytes, &salt)
            .map(|hash| hash.to_string())
            .map_err(|error| KeelError::internal(HASHING).param("detail", error.to_string()))
    })
    .await
    .map_err(|error| KeelError::internal(HASHING).source(error))?
}

#[cfg(test)]
mod tests {
    use sha_crypt::PasswordVerifier;

    use super::*;

    #[test]
    fn a_login_carries_the_prefix_of_the_accounts_of_the_panel() {
        assert_eq!(login("upload").unwrap(), "hp_upload");
        assert_eq!(login("a_1-b").unwrap(), "hp_a_1-b");
        for name in [
            "",
            "Upload",
            "1up",
            "up load",
            "up/load",
            "up.load",
            &"a".repeat(30),
        ] {
            let error = login(name).unwrap_err();
            assert_eq!(error.key(), NAME_ERROR, "{name}");
        }
    }

    #[test]
    fn a_directory_stays_below_the_document_root_of_its_site() {
        for value in ["", "uploads", "public/uploads", "_a/b-c.d", "a.b"] {
            assert_eq!(directory(value).unwrap(), value);
        }
        for value in [
            "..",
            "../other",
            "a/../../etc",
            "/etc",
            "a//b",
            "a/",
            ".ssh",
            "a/./b",
            "a b",
            "a\nlocal_root=/",
            &"a".repeat(201),
        ] {
            let error = directory(value).unwrap_err();
            assert_eq!(error.key(), DIRECTORY_ERROR, "{value:?}");
        }
        assert_eq!(
            chroot("/var/www/keel-panel-a/public", ""),
            "/var/www/keel-panel-a/public"
        );
        assert_eq!(
            chroot("/var/www/keel-panel-a/public", "uploads"),
            "/var/www/keel-panel-a/public/uploads"
        );
    }

    #[test]
    fn the_crypt_scheme_matches_the_reference_vector_of_sha512_crypt() {
        let reference = "$6$rounds=10000$saltstringsaltst$OW1/O6BYHV6BcXZu8QVeXbDWra3Oeqh0sbHbbMCVNSnCM/UrjmM0Dp8vOuZeHBy/YTBmSK6H9qs/y3RnOaw5v.";
        assert!(
            ShaCrypt::default()
                .verify_password(b"Hello world!", reference)
                .is_ok()
        );
    }

    #[tokio::test]
    async fn the_password_is_hashed_as_sha512_crypt_with_a_salt_crypt_reads_whole() {
        let password = Secret::from("correct horse battery".to_owned());
        let first = hash(&password).await.unwrap();
        let second = hash(&password).await.unwrap();
        assert_ne!(first, second);
        let parts: Vec<&str> = first.split('$').collect();
        assert_eq!(parts.len(), 5, "{first}");
        assert_eq!(parts[..3], ["", "6", "rounds=5000"], "{first}");
        assert_eq!(parts[3].len(), 16, "{first}");
        assert_eq!(parts[4].len(), 86, "{first}");
        assert!(!first.contains([':', '\n']));
        assert!(
            ShaCrypt::default()
                .verify_password(b"correct horse battery", first.as_str())
                .is_ok()
        );
        assert!(
            ShaCrypt::default()
                .verify_password(b"wrong horse battery", first.as_str())
                .is_err()
        );
        let refused = check_password(&Secret::from("pass\nword".to_owned())).unwrap_err();
        assert_eq!(refused.key(), PASSWORD);
    }
}
