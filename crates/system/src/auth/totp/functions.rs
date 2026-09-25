use std::sync::Arc;

use keel_sdk::prelude::*;
use keel_sdk::system::SecretCipher;

use super::super::functions::{
    LoggedIn, Nothing, below_caller, http_client, login_principal, open_session,
};
use super::super::limits::Lockout;
use super::challenge::{self, Attempts};
use super::{
    confirm_with, confirmed, enroll as enroll_row, redeem, remove, replace_recovery_codes, row,
};

pub const PERMISSION_MANAGE: &str = "auth.totp.manage";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Enrolled {
    #[schemars(extend("x-keel" = json!({ "once": true })))]
    pub secret: String,
    #[schemars(extend("x-keel" = json!({ "once": true })))]
    pub uri: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Confirm {
    #[schemars(extend("x-keel" = json!({ "secret": true })))]
    pub code: Secret,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub struct Confirmed {
    #[schemars(extend("x-keel" = json!({ "once": true })))]
    pub recovery_codes: Vec<String>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Disable {
    #[schemars(extend("x-keel" = json!({ "level": "admin" })))]
    pub principal: Option<Id>,
    #[schemars(extend("x-keel" = json!({ "secret": true })))]
    pub code: Option<Secret>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Disabled {
    pub principal: Id,
    pub disabled: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Verify {
    #[schemars(extend("x-keel" = json!({ "secret": true })))]
    pub challenge: Secret,
    #[schemars(extend("x-keel" = json!({ "secret": true })))]
    pub code: Secret,
}

fn enrolled_already() -> KeelError {
    KeelError::conflict("auth.error.totp_enrolled")
}

fn not_enrolled() -> KeelError {
    KeelError::validation("auth.error.totp_not_enrolled")
}

fn wrong_code() -> KeelError {
    KeelError::validation("auth.error.code").field("code")
}

fn challenge_refused() -> KeelError {
    KeelError::unauthenticated("auth.error.challenge")
}

pub fn enroll() -> FunctionDef {
    function::<Nothing, Enrolled>("auth.totp.enroll")
        .kind(Kind::Action)
        .access(Level::User, PERMISSION_MANAGE)
        .unowned()
        .handler(|call: Call<Nothing>| async move {
            let caller = call.principal();
            if caller.level >= Level::Owner {
                return Err(KeelError::forbidden("auth.error.totp_owner"));
            }
            if confirmed(call.db(), caller.id).await?.is_some() {
                return Err(enrolled_already());
            }
            let cipher = call.require::<dyn SecretCipher>()?;
            let now = call.require::<dyn Clock>()?.now();
            let enrolment = enroll_row(call.tx()?, cipher.as_ref(), caller.id, &caller.login, now)
                .await?
                .ok_or_else(enrolled_already)?;
            Ok(Enrolled {
                secret: enrolment.secret,
                uri: enrolment.uri,
            })
        })
}

pub fn confirm() -> FunctionDef {
    function::<Confirm, Confirmed>("auth.totp.confirm")
        .kind(Kind::Form)
        .access(Level::User, PERMISSION_MANAGE)
        .unowned()
        .ui(Form::new("auth.totp.confirm.title").page("main", ["code"]))
        .handler(|call: Call<Confirm>| async move {
            let caller = call.principal();
            let factor = row(call.db(), caller.id).await?.ok_or_else(not_enrolled)?;
            if factor.confirmed_at.is_some() {
                return Err(enrolled_already());
            }
            let cipher = call.require::<dyn SecretCipher>()?;
            let now = call.require::<dyn Clock>()?.now();
            let tx = call.tx()?;
            let code = call.params().code.expose();
            if !confirm_with(tx, cipher.as_ref(), &factor, caller.id, code, now).await? {
                return Err(wrong_code());
            }
            let recovery_codes = replace_recovery_codes(tx, caller.id).await?;
            Ok(Confirmed { recovery_codes })
        })
}

pub fn disable() -> FunctionDef {
    function::<Disable, Disabled>("auth.totp.disable")
        .kind(Kind::Form)
        .access(Level::User, PERMISSION_MANAGE)
        .unowned()
        .ui(Form::new("auth.totp.disable.title").page("main", ["principal", "code"]))
        .handler(|call: Call<Disable>| async move {
            let params = call.params();
            let tx = call.tx()?;
            if let Some(target) = params.principal {
                let target = below_caller(&call, target).await?;
                let disabled = remove(tx, target).await?;
                return Ok(Disabled {
                    principal: target,
                    disabled,
                });
            }
            let Some(code) = &params.code else {
                return Err(KeelError::validation("auth.error.code_required").field("code"));
            };
            let caller = call.principal();
            let factor = row(call.db(), caller.id).await?.ok_or_else(not_enrolled)?;
            let cipher = call.require::<dyn SecretCipher>()?;
            let now = call.require::<dyn Clock>()?.now();
            if !redeem(tx, cipher.as_ref(), &factor, caller.id, code.expose(), now).await? {
                return Err(wrong_code());
            }
            let disabled = remove(tx, caller.id).await?;
            Ok(Disabled {
                principal: caller.id,
                disabled,
            })
        })
}

pub fn verify(attempts: Arc<Attempts>, lockout: Arc<Lockout>) -> FunctionDef {
    function::<Verify, LoggedIn>("auth.totp.verify")
        .kind(Kind::Form)
        .mode(Mode::Write)
        .level(Level::Guest)
        .unowned()
        .exempt_from_read_only()
        .ui(Form::new("auth.totp.verify.title").page("main", ["challenge", "code"]))
        .handler(move |call: Call<Verify>| {
            let attempts = attempts.clone();
            let lockout = lockout.clone();
            async move {
                let client = http_client(&call)?;
                let now = call.require::<dyn Clock>()?.now();
                let params = call.params();
                let challenge_hash = challenge::hash(params.challenge.expose());
                let pending = challenge::pending(call.db(), &challenge_hash, now)
                    .await?
                    .ok_or_else(challenge_refused)?;
                let principal_id = challenge::principal_id(&pending)?;
                let principal = login_principal(&call, principal_id, challenge_refused).await?;
                let lockout_policy = lockout
                    .check(&call, &principal.login, client.ip, now)
                    .await?;
                if !attempts.admit(&challenge_hash, pending.expires_at, now) {
                    return Err(challenge_refused());
                }
                let factor = confirmed(call.db(), principal_id)
                    .await?
                    .ok_or_else(challenge_refused)?;
                let cipher = call.require::<dyn SecretCipher>()?;
                let tx = call.tx()?;
                let code = params.code.expose();
                if !redeem(tx, cipher.as_ref(), &factor, principal_id, code, now).await? {
                    lockout.failed(&principal.login, client.ip, now, lockout_policy);
                    return Err(KeelError::unauthenticated("auth.error.code"));
                }
                if !challenge::consume(tx, &challenge_hash, now).await? {
                    return Err(challenge_refused());
                }
                lockout.succeeded(&principal.login, client.ip);
                let principal = open_session(&call, principal, client, now).await?;
                Ok(LoggedIn::authenticated(principal))
            }
        })
}
