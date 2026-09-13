use std::sync::Arc;

use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::OnConflict;
use keel_sdk::sea_orm::{EntityTrait, Set};
use keel_sdk::system::{CallExt, Client, PrincipalSource, SessionChange};
use keel_sdk::types::{MetaPrincipal, Via};

use super::api::{LOCAL_METHOD, LoginMethod};
use super::entity::password;
use super::password::{Passwords, credentials};
use super::{State, session, totp};

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
                let target = match params.principal {
                    Some(target) => below_caller(&call, target).await?,
                    None => own_after_current(&call, &passwords).await?,
                };
                let hash = passwords.hash(&params.password).await?;
                let now = call.require::<dyn Clock>()?.now();
                password::Entity::insert(password::ActiveModel {
                    principal_id: Set(target.to_string()),
                    hash: Set(hash),
                    updated_at: Set(now),
                })
                .on_conflict(
                    OnConflict::column(password::Column::PrincipalId)
                        .update_columns([password::Column::Hash, password::Column::UpdatedAt])
                        .to_owned(),
                )
                .exec(call.tx()?)
                .await?;
                Ok(PasswordChanged { principal: target })
            }
        })
}

async fn own_after_current(call: &Call<PasswordSet>, passwords: &Passwords) -> Result<Id> {
    let Some(current) = &call.params().current else {
        return Err(KeelError::validation("auth.error.current_required").field("current"));
    };
    passwords
        .verify(&call.principal().login, current, call.db())
        .await
}

pub(super) async fn below_caller<P>(call: &Call<P>, target: Id) -> Result<Id> {
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
    Ok(target)
}

pub fn login(state: Arc<State>) -> FunctionDef {
    function::<Login, LoggedIn>("auth.login")
        .kind(Kind::Form)
        .mode(Mode::Write)
        .level(Level::Guest)
        .unowned()
        .ui(Form::new("auth.login.title").page("main", ["login", "password"]))
        .handler(move |call: Call<Login>| {
            let state = state.clone();
            async move {
                let client = http_client(&call)?;
                let params = call.params();
                let method = params.method.as_deref().unwrap_or(LOCAL_METHOD);
                let verifier = state.method(method).ok_or_else(|| {
                    KeelError::validation("auth.error.method_unknown")
                        .field("method")
                        .param("method", method)
                })?;
                let id = verifier
                    .verify(&params.login, &params.password, call.db())
                    .await?;
                let principal = login_principal(&call, id, credentials).await?;
                let now = call.require::<dyn Clock>()?.now();
                if totp::confirmed(call.db(), principal.id).await?.is_some() {
                    let challenge =
                        totp::challenge::create(call.tx()?, principal.id, client, now).await?;
                    return Ok(LoggedIn {
                        state: LoginState::Totp,
                        principal: None,
                        challenge: Some(challenge.into_inner()),
                    });
                }
                open_session(&call, principal, client, now).await
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

pub(super) async fn open_session<P>(
    call: &Call<P>,
    principal: Principal,
    client: &Client,
    now: DateTime<Utc>,
) -> Result<LoggedIn> {
    let tx = call.tx()?;
    if let Some(previous) = &client.session {
        session::revoke(tx, previous, now, session::REASON_REPLACED).await?;
    }
    let token = session::open(tx, &principal, client, now).await?;
    call.session(SessionChange::Set(token));
    Ok(LoggedIn {
        state: LoginState::Authenticated,
        principal: Some(MetaPrincipal {
            id: principal.id,
            login: principal.login,
            level: principal.level,
            account: principal.account_id,
            actor: None,
        }),
        challenge: None,
    })
}

pub fn logout() -> FunctionDef {
    function::<Nothing, LoggedOut>("auth.logout")
        .kind(Kind::Action)
        .level(Level::Service)
        .unowned()
        .handler(|call: Call<Nothing>| async move {
            let Some(id_hash) = call.client().and_then(|client| client.session.clone()) else {
                return Err(KeelError::validation("auth.error.session_only"));
            };
            let now = call.require::<dyn Clock>()?.now();
            let revoked =
                session::revoke(call.tx()?, &id_hash, now, session::REASON_LOGOUT).await?;
            call.session(SessionChange::Clear);
            Ok(LoggedOut { revoked })
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
