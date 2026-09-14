use std::collections::BTreeMap;

use keel_types::{Id, Level, Principal, Scope, Via};

use crate::loader::REQUIRED_LANGUAGE;

pub const OWNER_LOGIN: &str = "owner";
const OWNER_ROLE: &str = "owner";

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the peer runs as uid {peer}; the control socket admits uid {owner} alone")]
pub struct Refused {
    pub peer: u32,
    pub owner: u32,
}

pub fn admit(peer: u32, owner: u32) -> Result<(), Refused> {
    if peer == owner {
        Ok(())
    } else {
        Err(Refused { peer, owner })
    }
}

pub fn owner() -> Principal {
    Principal {
        id: Id::ROOT_ACCOUNT,
        login: OWNER_LOGIN.to_owned(),
        level: Level::Owner,
        account_id: Id::ROOT_ACCOUNT,
        role: OWNER_ROLE.to_owned(),
        actor: None,
        scope: Scope::Full,
        via: Via::Socket,
        external_id: None,
        origin: None,
        lang: REQUIRED_LANGUAGE.to_owned(),
        grants: BTreeMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_peer_with_a_foreign_uid_is_refused_naming_both_uids() {
        assert_eq!(admit(1000, 1000), Ok(()));
        let refused = admit(0, 1000).unwrap_err();
        assert_eq!(
            refused,
            Refused {
                peer: 0,
                owner: 1000
            }
        );
        assert_eq!(
            refused.to_string(),
            "the peer runs as uid 0; the control socket admits uid 1000 alone"
        );
        assert!(admit(1001, 1000).is_err());
    }

    #[test]
    fn the_socket_peer_is_the_owner_of_the_root_account() {
        let owner = owner();
        assert_eq!(owner.level, Level::Owner);
        assert_eq!(owner.login, OWNER_LOGIN);
        assert_eq!(owner.account_id, Id::ROOT_ACCOUNT);
        assert_eq!(owner.via, Via::Socket);
        assert!(owner.actor.is_none());
        let meta = owner.meta().unwrap();
        assert_eq!(meta.account, Id::ROOT_ACCOUNT);
        assert_eq!(meta.level, Level::Owner);
    }
}
