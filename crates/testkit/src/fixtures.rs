use std::collections::BTreeMap;
use std::fmt;

use keel_kernel::http::{GUEST_LOGIN, guest};
use keel_kernel::keel::{DEFAULT_LANG, DEFAULT_ROLE, RESELLER_KIND};
use keel_kernel::socket::{OWNER_LOGIN, owner};
use keel_kernel::{CallOptions, Dispatcher, Reply};
use keel_sdk::Db;
use keel_sdk::system::PrincipalSource;
use keel_types::{CallError, ErrorCode, Id, Level, Principal, Scope, Via};
use serde_json::{Value, json};

pub(crate) const USER_KIND: &str = "user";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Who {
    Guest,
    Service,
    Owner,
    Admin,
    Reseller,
    A1,
    A2,
    Unrelated,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Accounts {
    pub root: Id,
    pub reseller: Id,
    pub a1: Id,
    pub a2: Id,
    pub unrelated: Id,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Principals {
    pub guest: Principal,
    pub service: Principal,
    pub owner: Principal,
    pub admin: Principal,
    pub reseller: Principal,
    pub a1: Principal,
    pub a2: Principal,
    pub unrelated: Principal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fixtures {
    pub accounts: Accounts,
    pub principals: Principals,
}

#[derive(Debug, thiserror::Error)]
#[error("seeding the fixture tree failed in {function}: {error:?}")]
pub struct SeedError {
    pub function: String,
    pub error: Box<CallError>,
}

impl Who {
    pub const ALL: [Who; 8] = [
        Who::Guest,
        Who::Service,
        Who::Owner,
        Who::Admin,
        Who::Reseller,
        Who::A1,
        Who::A2,
        Who::Unrelated,
    ];

    pub const fn login(self) -> &'static str {
        match self {
            Who::Guest => GUEST_LOGIN,
            Who::Service => "service",
            Who::Owner => OWNER_LOGIN,
            Who::Admin => "admin",
            Who::Reseller => "reseller",
            Who::A1 => "a1",
            Who::A2 => "a2",
            Who::Unrelated => "unrelated",
        }
    }

    pub const fn level(self) -> Level {
        match self {
            Who::Guest => Level::Guest,
            Who::Service => Level::Service,
            Who::Owner => Level::Owner,
            Who::Admin => Level::Admin,
            Who::Reseller => Level::Reseller,
            Who::A1 | Who::A2 | Who::Unrelated => Level::User,
        }
    }

    pub const fn at(level: Level) -> Who {
        match level {
            Level::Guest => Who::Guest,
            Level::Service => Who::Service,
            Level::User => Who::A1,
            Level::Reseller => Who::Reseller,
            Level::Admin => Who::Admin,
            Level::Owner => Who::Owner,
        }
    }
}

impl fmt::Display for Who {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.login())
    }
}

impl Fixtures {
    pub fn principal(&self, who: Who) -> &Principal {
        match who {
            Who::Guest => &self.principals.guest,
            Who::Service => &self.principals.service,
            Who::Owner => &self.principals.owner,
            Who::Admin => &self.principals.admin,
            Who::Reseller => &self.principals.reseller,
            Who::A1 => &self.principals.a1,
            Who::A2 => &self.principals.a2,
            Who::Unrelated => &self.principals.unrelated,
        }
    }

    pub fn at(&self, level: Level) -> &Principal {
        self.principal(Who::at(level))
    }

    pub fn account_of(&self, who: Who) -> Id {
        self.principal(who).account_id
    }

    pub fn all(&self) -> impl Iterator<Item = (Who, &Principal)> {
        Who::ALL.into_iter().map(|who| (who, self.principal(who)))
    }
}

pub(crate) fn stored_principal(
    id: Id,
    login: &str,
    level: Level,
    account_id: Id,
    role: &str,
    lang: &str,
) -> Principal {
    Principal {
        id,
        login: login.to_owned(),
        level,
        account_id,
        role: role.to_owned(),
        actor: None,
        scope: Scope::Full,
        via: Via::Http,
        external_id: None,
        origin: None,
        lang: lang.to_owned(),
        grants: BTreeMap::new(),
    }
}

pub(crate) fn owner_principal() -> Principal {
    owner()
}

pub(crate) fn guest_principal() -> Principal {
    guest(DEFAULT_LANG)
}

pub(crate) async fn seed(dispatcher: &Dispatcher) -> Result<Fixtures, SeedError> {
    let owner = owner_principal();
    let root = Id::ROOT_ACCOUNT;
    let reseller = account(dispatcher, &owner, RESELLER_KIND, None).await?;
    let a1 = account(dispatcher, &owner, USER_KIND, Some(reseller)).await?;
    let a2 = account(dispatcher, &owner, USER_KIND, Some(reseller)).await?;
    let unrelated = account(dispatcher, &owner, USER_KIND, None).await?;
    let service = principal(dispatcher, &owner, Who::Service, root).await?;
    let admin = principal(dispatcher, &owner, Who::Admin, root).await?;
    let reseller_principal = principal(dispatcher, &owner, Who::Reseller, reseller).await?;
    let a1_principal = principal(dispatcher, &owner, Who::A1, a1).await?;
    let a2_principal = principal(dispatcher, &owner, Who::A2, a2).await?;
    let unrelated_principal = principal(dispatcher, &owner, Who::Unrelated, unrelated).await?;
    Ok(Fixtures {
        accounts: Accounts {
            root,
            reseller,
            a1,
            a2,
            unrelated,
        },
        principals: Principals {
            guest: guest_principal(),
            service,
            owner,
            admin,
            reseller: reseller_principal,
            a1: a1_principal,
            a2: a2_principal,
            unrelated: unrelated_principal,
        },
    })
}

pub(crate) async fn recover(dispatcher: &Dispatcher) -> Result<Fixtures, SeedError> {
    let source = dispatcher
        .registry()
        .services()
        .get::<dyn PrincipalSource>()
        .ok_or_else(|| missing("keel.principal.by_login"))?;
    let db = Db::from_connection(dispatcher.db().clone());
    let mut stored = Vec::with_capacity(6);
    for who in [
        Who::Service,
        Who::Admin,
        Who::Reseller,
        Who::A1,
        Who::A2,
        Who::Unrelated,
    ] {
        let found = source
            .by_login(who.login(), &db)
            .await
            .map_err(|error| SeedError {
                function: "keel.principal.by_login".to_owned(),
                error: Box::new(error.to_call_error()),
            })?
            .ok_or_else(|| missing(&format!("keel.principal.by_login {who}")))?;
        stored.push(stored_principal(
            found.id,
            who.login(),
            who.level(),
            found.account_id,
            DEFAULT_ROLE,
            DEFAULT_LANG,
        ));
    }
    let [service, admin, reseller, a1, a2, unrelated] =
        <[Principal; 6]>::try_from(stored).map_err(|_| missing("keel.principal.by_login"))?;
    Ok(Fixtures {
        accounts: Accounts {
            root: Id::ROOT_ACCOUNT,
            reseller: reseller.account_id,
            a1: a1.account_id,
            a2: a2.account_id,
            unrelated: unrelated.account_id,
        },
        principals: Principals {
            guest: guest_principal(),
            service,
            owner: owner_principal(),
            admin,
            reseller,
            a1,
            a2,
            unrelated,
        },
    })
}

fn missing(function: &str) -> SeedError {
    SeedError {
        function: function.to_owned(),
        error: Box::new(CallError::new(ErrorCode::NotFound)),
    }
}

async fn account(
    dispatcher: &Dispatcher,
    owner: &Principal,
    kind: &str,
    parent: Option<Id>,
) -> Result<Id, SeedError> {
    let reply = call(
        dispatcher,
        owner,
        "keel.account.create",
        json!({ "kind": kind, "parent": parent }),
    )
    .await?;
    saved_id("keel.account.create", &reply)
}

async fn principal(
    dispatcher: &Dispatcher,
    owner: &Principal,
    who: Who,
    account: Id,
) -> Result<Principal, SeedError> {
    let reply = call(
        dispatcher,
        owner,
        "keel.principal.create",
        json!({ "login": who.login(), "level": who.level(), "account": account }),
    )
    .await?;
    let id = saved_id("keel.principal.create", &reply)?;
    Ok(stored_principal(
        id,
        who.login(),
        who.level(),
        account,
        DEFAULT_ROLE,
        DEFAULT_LANG,
    ))
}

async fn call(
    dispatcher: &Dispatcher,
    principal: &Principal,
    function: &str,
    params: Value,
) -> Result<Reply, SeedError> {
    let reply = dispatcher
        .call(principal.clone(), function, params, CallOptions::default())
        .await;
    match reply.error() {
        Some(error) => Err(SeedError {
            function: function.to_owned(),
            error: Box::new(error.clone()),
        }),
        None => Ok(reply),
    }
}

fn saved_id(function: &str, reply: &Reply) -> Result<Id, SeedError> {
    reply
        .data()
        .and_then(|data| data.get("id"))
        .and_then(Value::as_str)
        .and_then(|id| id.parse().ok())
        .ok_or_else(|| SeedError {
            function: function.to_owned(),
            error: Box::new(CallError::new(ErrorCode::Internal)),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_level_maps_to_one_fixture_principal_and_back() {
        for level in Level::ALL {
            assert_eq!(Who::at(level).level(), level);
        }
        assert_eq!(Who::at(Level::User), Who::A1);
        for who in Who::ALL {
            assert_eq!(who.to_string(), who.login());
        }
    }

    #[test]
    fn the_synthetic_principals_match_the_kernel_ones() {
        let owner = owner_principal();
        assert_eq!(owner.level, Level::Owner);
        assert_eq!(owner.account_id, Id::ROOT_ACCOUNT);
        assert_eq!(owner.via, Via::Socket);
        let guest = guest_principal();
        assert_eq!(guest.level, Level::Guest);
        assert_eq!(guest.login, Who::Guest.login());
        assert_eq!(guest.lang, DEFAULT_LANG);
    }
}
