use keel::prelude::*;
use keel_hosting_users::api::{HostingUser, HostingUsers};

use crate::USER_PREFIX;

pub(crate) const NAME: &str = r"^hp_[a-z][a-z0-9_-]*_[a-z0-9]{1,16}$";
const SUFFIX_LIMIT: usize = 16;
const OUTSIDE: &str = "hosting.database.error.prefix";
const PASSWORD: &str = "hosting.database.error.password";

fn owner_name(name: &str) -> Option<&str> {
    let (owner, suffix) = name.rsplit_once('_')?;
    let plain = !suffix.is_empty()
        && suffix.len() <= SUFFIX_LIMIT
        && suffix
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    (plain && owner.len() > USER_PREFIX.len() && owner.starts_with(USER_PREFIX)).then_some(owner)
}

pub(crate) async fn owner<P>(call: &Call<P>, name: &str) -> Result<Option<HostingUser>> {
    let Some(owner) = owner_name(name) else {
        return Ok(None);
    };
    let tx = call.tx()?;
    let Some(owner) = call.require::<dyn HostingUsers>()?.named(tx, owner).await? else {
        return Ok(None);
    };
    Ok(call
        .account_scope()
        .includes(tx, owner.account)
        .await?
        .then_some(owner))
}

pub(crate) async fn owner_of<P>(call: &Call<P>, name: &str) -> Result<HostingUser> {
    owner(call, name).await?.ok_or_else(|| {
        KeelError::validation(OUTSIDE)
            .field("name")
            .param("name", name)
    })
}

pub(crate) fn check_password(password: &Secret) -> Result<()> {
    if password.expose().chars().any(char::is_control) {
        Err(KeelError::validation(PASSWORD).field("password"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_owner_is_the_hosting_user_before_the_last_underscore() {
        assert_eq!(owner_name("hp_alice_wp"), Some("hp_alice"));
        assert_eq!(owner_name("hp_a_b_wp2"), Some("hp_a_b"));
        assert_eq!(owner_name("hp_alice_"), None);
        assert_eq!(owner_name("hp_alice_w-p"), None);
        assert_eq!(owner_name("hp_alice_Wp"), None);
        assert_eq!(owner_name(&format!("hp_alice_{}", "a".repeat(17))), None);
        assert_eq!(owner_name("hp__wp"), None);
        assert_eq!(owner_name("alice_wp"), None);
        assert_eq!(owner_name("wp"), None);
    }

    #[test]
    fn a_password_carries_no_control_character() {
        assert!(check_password(&Secret::new("it's \"fine\" \\ ;")).is_ok());
        assert!(check_password(&Secret::new("line\nbreak")).is_err());
    }
}
