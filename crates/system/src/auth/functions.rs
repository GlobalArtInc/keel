use std::sync::Arc;

use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::system::{CallExt, Client, PrincipalSource, SessionChange};
use keel_sdk::types::{ErrorCode, MetaPrincipal, Via};

use super::api::{LOCAL_METHOD, LoginMethod};
use super::limits::Lockout;
use super::password::{self, Passwords, check_policy, credentials};
use super::{State, policy, session, totp};

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct PasswordSet {
    #[schemars(extend("x-keel" = json!({ "level": "reseller" })))]
    pub principal: Option<Id>,
    #[schemars(extend("x-keel" = json!({ "secret": true }), "x-ui" = json!({ "control": "password" })))]
    pub current: Option<Secret>,
    #[schemars(extend("x-keel" = json!({ "secret": true }), "x-ui" = json!({ "control": "password" })))]
    pub password: Secret,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct PasswordChanged {
    pub principal: Id,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Login {
    #[schemars(length(min = 1, max = 64))]
    pub login: String,
    #[schemars(extend("x-keel" = json!({ "secret": true }), "x-ui" = json!({ "control": "password" })))]
    pub password: Secret,
    pub method: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "snake_case")]
#[schemars(crate = "keel_sdk::schemars")]
pub enum LoginState {
    Authenticated,
    Totp,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct LoggedIn {
    pub state: LoginState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub principal: Option<MetaPrincipal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(extend("x-keel" = json!({ "once": true })))]
    pub challenge: Option<String>,
}

impl LoggedIn {
    pub fn authenticated(principal: MetaPrincipal) -> Self {
        Self {
            state: LoginState::Authenticated,
            principal: Some(principal),
            challenge: None,
        }
    }
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Nothing {}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct LoggedOut {
    pub revoked: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Unlock {
    #[schemars(length(min = 1, max = 64))]
    pub login: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Unlocked {
    pub login: String,
    pub cleared: u32,
}

pub fn password_set(passwords: Arc<Passwords>) -> FunctionDef {
    function::<PasswordSet, PasswordChanged>("auth.password.set")
        .kind(Kind::Form)
        .access(Level::User, "auth.password.manage")
        .unowned()
        .ui(Form::new("auth.password.set.title").page("main", ["principal", "current", "password"]))
        .handler(move |call: Call<PasswordSet>| {
            let passwords = passwords.clone();
            async move {
                let params = call.params();
                let (target, login) = match params.principal {
                    Some(target) => {
                        let target = below_caller(&call, target).await?;
                        (target, login_of(&call, target).await?)
                    }
                    None => (call.principal().id, call.principal().login.clone()),
                };
                check_policy(&params.password, &login, policy::min_length(&call).await?)?;
                if params.principal.is_none() {
                    verify_current(&call, &passwords).await?;
                }
                let hash = passwords.hash(&params.password).await?;
                let now = call.require::<dyn Clock>()?.now();
                password::store(call.tx()?, target, hash, now).await?;
                Ok(PasswordChanged { principal: target })
            }
        })
}

async fn verify_current(call: &Call<PasswordSet>, passwords: &Passwords) -> Result<()> {
    let Some(current) = &call.params().current else {
        return Err(KeelError::validation("auth.error.current_required").field("current"));
    };
    passwords
        .verify(&call.principal().login, current, call.db())
        .await?;
    Ok(())
}

async fn login_of<P>(call: &Call<P>, id: Id) -> Result<String> {
    let principals = call.require::<dyn PrincipalSource>()?;
    principals
        .by_id(id, call.db())
        .await?
        .map(|principal| principal.login)
        .ok_or_else(|| KeelError::not_found("keel.error.not_found"))
}

pub(super) async fn below_caller<P>(call: &Call<P>, target: Id) -> Result<Id> {
    below_caller_with_account(call, target)
        .await
        .map(|(target, _)| target)
}

pub(super) async fn below_caller_with_account<P>(call: &Call<P>, target: Id) -> Result<(Id, Id)> {
    let row = call
        .invoke("keel.principal.get", json!({ "id": target }))
        .await?;
    let level: Level = row["level"]
        .as_str()
        .and_then(|level| level.parse().ok())
        .ok_or_else(|| KeelError::internal("keel.principal.error.level_unknown"))?;
    if level >= call.principal().level {
        return Err(KeelError::forbidden("auth.error.not_below")
            .param("level", level.as_str())
            .param("caller", call.principal().level.as_str()));
    }
    let account: Id = row["accountId"]
        .as_str()
        .and_then(|account| account.parse().ok())
        .ok_or_else(|| {
            KeelError::internal("keel.principal.error.account_unknown")
                .param("id", target.to_string())
                .param("account", row["accountId"].to_string())
        })?;
    Ok((target, account))
}

pub fn login(state: Arc<State>, lockout: Arc<Lockout>) -> FunctionDef {
    function::<Login, LoggedIn>("auth.login")
        .kind(Kind::Form)
        .mode(Mode::Write)
        .level(Level::Guest)
        .unowned()
        .ui(Form::new("auth.login.title").page("main", ["login", "password"]))
        .handler(move |call: Call<Login>| {
            let state = state.clone();
            let lockout = lockout.clone();
            async move {
                let client = http_client(&call)?;
                let params = call.params();
                let now = call.require::<dyn Clock>()?.now();
                let lockout_policy = lockout.check(&call, &params.login, client.ip, now).await?;
                let method = params.method.as_deref().unwrap_or(LOCAL_METHOD);
                let verifier = state.method(method).ok_or_else(|| {
                    KeelError::validation("auth.error.method_unknown")
                        .field("method")
                        .param("method", method)
                })?;
                let id = match verifier
                    .verify(&params.login, &params.password, call.db())
                    .await
                {
                    Ok(id) => id,
                    Err(error) => {
                        if error.code() == ErrorCode::Unauthenticated {
                            lockout.failed(&params.login, client.ip, now, lockout_policy);
                        }
                        return Err(error);
                    }
                };
                let principal = login_principal(&call, id, credentials).await?;
                if totp::confirmed(call.db(), principal.id).await?.is_some() {
                    let challenge =
                        totp::challenge::create(call.tx()?, principal.id, client, now).await?;
                    return Ok(LoggedIn {
                        state: LoginState::Totp,
                        principal: None,
                        challenge: Some(challenge.into_inner()),
                    });
                }
                lockout.succeeded(&params.login, client.ip);
                let principal = open_session(&call, principal, client, now).await?;
                Ok(LoggedIn::authenticated(principal))
            }
        })
}

pub(super) async fn login_principal<P>(
    call: &Call<P>,
    id: Id,
    refused: fn() -> KeelError,
) -> Result<Principal> {
    let principals = call.require::<dyn PrincipalSource>()?;
    let Some(mut principal) = principals.by_id(id, call.db()).await? else {
        return Err(refused());
    };
    if principal.level >= Level::Owner {
        return Err(refused());
    }
    principal.via = Via::Http;
    Ok(principal)
}

pub(super) fn http_client<P>(call: &Call<P>) -> Result<&Client> {
    call.client()
        .filter(|_| call.principal().via == Via::Http)
        .ok_or_else(|| KeelError::validation("auth.error.http_only"))
}

pub(super) fn session_client<P>(call: &Call<P>) -> Result<(&Client, String)> {
    let session_only = || KeelError::validation("auth.error.session_only");
    let client = call.client().ok_or_else(session_only)?;
    let id_hash = client.session.clone().ok_or_else(session_only)?;
    Ok((client, id_hash))
}

pub(super) async fn open_session<P>(
    call: &Call<P>,
    principal: Principal,
    client: &Client,
    now: DateTime<Utc>,
) -> Result<MetaPrincipal> {
    let lifetime = policy::lifetime(call).await?;
    let tx = call.tx()?;
    if let Some(previous) = &client.session {
        session::revoke(tx, previous, now, session::REASON_REPLACED).await?;
    }
    let token = session::open(tx, &principal, client, now, lifetime).await?;
    call.session(SessionChange::Set(token));
    Ok(MetaPrincipal {
        id: principal.id,
        login: principal.login,
        level: principal.level,
        account: principal.account_id,
        actor: None,
    })
}

pub fn logout() -> FunctionDef {
    function::<Nothing, LoggedOut>("auth.logout")
        .kind(Kind::Action)
        .level(Level::Service)
        .unowned()
        .handler(|call: Call<Nothing>| async move {
            let (_, id_hash) = session_client(&call)?;
            let now = call.require::<dyn Clock>()?.now();
            let revoked =
                session::revoke(call.tx()?, &id_hash, now, session::REASON_LOGOUT).await?;
            call.session(SessionChange::Clear);
            Ok(LoggedOut { revoked })
        })
}

pub fn unlock(lockout: Arc<Lockout>) -> FunctionDef {
    function::<Unlock, Unlocked>("auth.unlock")
        .kind(Kind::Action)
        .level(Level::Admin)
        .unowned()
        .handler(move |call: Call<Unlock>| {
            let lockout = lockout.clone();
            async move {
                let login = call.params().login.clone();
                let cleared = lockout.unlock(&login);
                Ok(Unlocked {
                    login,
                    cleared: u32::try_from(cleared).unwrap_or(u32::MAX),
                })
            }
        })
}

pub struct Local {
    passwords: Arc<Passwords>,
}

impl Local {
    pub fn new(passwords: Arc<Passwords>) -> Self {
        Self { passwords }
    }
}

#[async_trait]
impl LoginMethod for Local {
    fn id(&self) -> &str {
        LOCAL_METHOD
    }

    async fn verify(&self, login: &str, secret: &Secret, db: &Db) -> Result<Id> {
        self.passwords.verify(login, secret, db).await
    }
}
