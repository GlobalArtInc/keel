use std::sync::Arc;

use keel_sdk::chrono::{DateTime, Duration, Utc};
use keel_sdk::prelude::*;
use keel_sdk::system::Caller;

use super::super::entity::principal_address;
use super::super::functions::below_caller;
use super::super::mail::{self, Purpose, SETTING_SHELL_URL, VERIFY_TEMPLATE};
use super::super::password::Passwords;
use super::{confirm, row, store};

pub const GET: &str = "auth.address.get";
pub const SET: &str = "auth.address.set";
pub const VERIFY: &str = "auth.address.verify";
const PERMISSION: &str = "auth.password.manage";
const VERIFY_HOURS: i64 = 24;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct AddressQuery {
    #[schemars(extend("x-keel" = json!({ "level": "admin" })))]
    principal: Option<Id>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
struct AddressView {
    principal: Id,
    value: Option<String>,
    verified: bool,
    verified_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct AddressSet {
    #[schemars(extend("x-keel" = json!({ "level": "admin" })))]
    principal: Option<Id>,
    #[schemars(length(min = 3, max = 254), extend("x-keel" = json!({ "check": "email" })))]
    value: String,
    #[schemars(extend("x-keel" = json!({ "secret": true }), "x-ui" = json!({ "control": "password" })))]
    current: Option<Secret>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct AddressVerify {
    #[schemars(extend("x-keel" = json!({ "secret": true })))]
    token: Secret,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Verified {
    value: String,
}

fn view(principal: Id, found: Option<principal_address::Model>) -> AddressView {
    match found {
        Some(row) => AddressView {
            principal,
            verified: row.verified_at.is_some(),
            verified_at: row.verified_at,
            value: Some(row.value),
        },
        None => AddressView {
            principal,
            value: None,
            verified: false,
            verified_at: None,
        },
    }
}

async fn other<P>(call: &Call<P>, requested: Option<Id>) -> Result<Option<Id>> {
    let Some(target) = requested.filter(|target| *target != call.principal().id) else {
        return Ok(None);
    };
    below_caller(call, target).await.map(Some)
}

fn token_refused() -> KeelError {
    KeelError::validation("auth.error.verify_token").field("token")
}

pub fn get() -> FunctionDef {
    function::<AddressQuery, AddressView>(GET)
        .kind(Kind::Get)
        .access(Level::User, PERMISSION)
        .unowned()
        .handler(|call: Call<AddressQuery>| async move {
            let principal = other(&call, call.params().principal)
                .await?
                .unwrap_or(call.principal().id);
            Ok(view(principal, row(call.db(), principal).await?))
        })
}

pub fn set(passwords: Arc<Passwords>) -> FunctionDef {
    function::<AddressSet, AddressView>(SET)
        .kind(Kind::Form)
        .access(Level::User, PERMISSION)
        .unowned()
        .ui(Form::new("auth.address.set.title").page("main", ["principal", "value", "current"]))
        .handler(move |call: Call<AddressSet>| {
            let passwords = passwords.clone();
            async move {
                let params = call.params();
                let value = params.value.trim().to_owned();
                let now = call.require::<dyn Clock>()?.now();
                if let Some(target) = other(&call, params.principal).await? {
                    let tx = call.tx()?;
                    store(tx, target, &value, Some(now), now).await?;
                    mail::consume_all(tx, Purpose::Verify, target, now).await?;
                    return Ok(view(target, row(tx, target).await?));
                }
                let me = call.principal().id;
                if let Some(existing) = row(call.db(), me).await?
                    && existing.value == value
                    && existing.verified_at.is_some()
                {
                    return Ok(view(me, Some(existing)));
                }
                let Some(current) = &params.current else {
                    return Err(
                        KeelError::validation("auth.error.current_required").field("current")
                    );
                };
                passwords
                    .verify(&call.principal().login, current, call.db())
                    .await?;
                let url: String = call.setting(SETTING_SHELL_URL).await?;
                let base = mail::shell_base(&url)
                    .ok_or_else(|| KeelError::unavailable("auth.error.shell_url"))?
                    .to_owned();
                let tx = call.tx()?;
                store(tx, me, &value, None, now).await?;
                mail::consume_all(tx, Purpose::Verify, me, now).await?;
                let token = mail::issue(
                    tx,
                    Purpose::Verify,
                    me,
                    Some(&value),
                    Duration::hours(VERIFY_HOURS),
                    now,
                )
                .await?;
                let params = json!({
                    "link": mail::link(&base, Purpose::Verify, &token),
                    "address": value,
                    "hours": VERIFY_HOURS,
                });
                let caller = call.require::<dyn Caller>()?;
                call.after_commit(move |_| async move {
                    if let Err(error) =
                        mail::send(caller.as_ref(), me, VERIFY_TEMPLATE, params, true).await
                    {
                        tracing::warn!(principal = %me, error = %error, "the verification mail was not queued");
                    }
                    Ok(())
                });
                Ok(view(me, row(tx, me).await?))
            }
        })
}

pub fn verify() -> FunctionDef {
    function::<AddressVerify, Verified>(VERIFY)
        .kind(Kind::Form)
        .mode(Mode::Write)
        .level(Level::Guest)
        .unowned()
        .ui(Form::new("auth.address.verify.title").page("main", ["token"]))
        .handler(|call: Call<AddressVerify>| async move {
            let now = call.require::<dyn Clock>()?.now();
            let tx = call.tx()?;
            let found = mail::redeem(tx, Purpose::Verify, call.params().token.expose(), now)
                .await?
                .ok_or_else(token_refused)?;
            let principal = mail::principal_of(&found)?;
            let value = found.value.ok_or_else(token_refused)?;
            if !confirm(tx, principal, &value, now).await? {
                return Err(token_refused());
            }
            Ok(Verified { value })
        })
}
