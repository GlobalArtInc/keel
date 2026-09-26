use std::sync::Arc;

use keel_sdk::prelude::*;
use keel_sdk::system::{CallExt, Caller, PrincipalSource};

use super::super::functions::Nothing;
use super::super::mail::{self, Purpose, SETTING_SHELL_URL};
use super::super::password::{self, Passwords, check_policy};
use super::super::policy;
use super::{Request, ResetLimit, SETTING_MINUTES, deliver, revoke_sessions};

pub const REQUEST: &str = "auth.password.reset.request";
pub const CONFIRM: &str = "auth.password.reset.confirm";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct ResetRequest {
    #[schemars(length(min = 1, max = 64))]
    login: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct ResetConfirm {
    #[schemars(extend("x-keel" = json!({ "secret": true })))]
    token: Secret,
    #[schemars(extend("x-keel" = json!({ "secret": true }), "x-ui" = json!({ "control": "password" })))]
    password: Secret,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct ResetDone {
    login: String,
}

fn token_refused() -> KeelError {
    KeelError::validation("auth.error.reset_token").field("token")
}

pub fn request(limit: Arc<ResetLimit>) -> FunctionDef {
    function::<ResetRequest, Nothing>(REQUEST)
        .kind(Kind::Form)
        .mode(Mode::Write)
        .level(Level::Guest)
        .unowned()
        .ui(Form::new("auth.password.reset.request.title").page("main", ["login"]))
        .handler(move |call: Call<ResetRequest>| {
            let limit = limit.clone();
            async move {
                let login = call.params().login.trim().to_owned();
                let now = call.require::<dyn Clock>()?.now();
                let ip = call.client().and_then(|client| client.ip);
                if !limit.admit(&login, ip, now) {
                    return Err(KeelError::rate_limited("auth.error.reset_limited"));
                }
                let url: String = call.setting(SETTING_SHELL_URL).await?;
                let request = Request {
                    login,
                    minutes: call.setting(SETTING_MINUTES).await?,
                    base: mail::shell_base(&url).map(str::to_owned),
                    principals: call.require::<dyn PrincipalSource>()?,
                    caller: call.require::<dyn Caller>()?,
                    now,
                };
                call.after_commit(
                    move |context| async move { deliver(context.db()?, request).await },
                );
                Ok(Nothing {})
            }
        })
}

pub fn confirm(passwords: Arc<Passwords>) -> FunctionDef {
    function::<ResetConfirm, ResetDone>(CONFIRM)
        .kind(Kind::Form)
        .mode(Mode::Write)
        .level(Level::Guest)
        .unowned()
        .ui(Form::new("auth.password.reset.confirm.title").page("main", ["token", "password"]))
        .handler(move |call: Call<ResetConfirm>| {
            let passwords = passwords.clone();
            async move {
                let params = call.params();
                let now = call.require::<dyn Clock>()?.now();
                let tx = call.tx()?;
                let found = mail::redeem(tx, Purpose::Reset, params.token.expose(), now)
                    .await?
                    .ok_or_else(token_refused)?;
                let principal = mail::principal_of(&found)?;
                let login = call
                    .require::<dyn PrincipalSource>()?
                    .by_id(principal, tx)
                    .await?
                    .map(|principal| principal.login)
                    .ok_or_else(token_refused)?;
                check_policy(&params.password, &login, policy::min_length(&call).await?)?;
                let hash = passwords.hash(&params.password).await?;
                password::store(tx, principal, hash, now).await?;
                mail::consume_all(tx, Purpose::Reset, principal, now).await?;
                revoke_sessions(tx, principal, now).await?;
                Ok(ResetDone { login })
            }
        })
}
