use keel_sdk::prelude::*;
use keel_sdk::types::MetaPrincipal;

use super::super::functions::{below_caller, http_client, login_principal, open_session};
use super::{
    MAX_TTL_SECONDS, hash, is_local_path, issue as issue_row, issued_by, principal_id,
    redeem as redeem_row, url_of,
};

pub const PERMISSION_ISSUE: &str = "auth.key.issue";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Issue {
    pub principal: Id,
    #[schemars(range(min = 1, max = MAX_TTL_SECONDS))]
    pub ttl: u32,
    #[schemars(length(min = 1, max = 2048))]
    pub redirect: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Issued {
    #[schemars(extend("x-keel" = json!({ "once": true })))]
    pub key: String,
    #[schemars(extend("x-keel" = json!({ "once": true })))]
    pub url: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Redeem {
    #[schemars(extend("x-keel" = json!({ "secret": true })))]
    pub key: Secret,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub struct Redeemed {
    pub redirect: String,
    pub principal: MetaPrincipal,
    pub issued_by: Id,
}

fn key_refused() -> KeelError {
    KeelError::unauthenticated("auth.error.key")
}

pub fn issue() -> FunctionDef {
    function::<Issue, Issued>("auth.key.issue")
        .kind(Kind::Form)
        .access(Level::Reseller, PERMISSION_ISSUE)
        .unowned()
        .ui(Form::new("auth.key.issue.title").page("main", ["principal", "ttl", "redirect"]))
        .handler(|call: Call<Issue>| async move {
            let params = call.params();
            let target = below_caller(&call, params.principal).await?;
            if !is_local_path(&params.redirect) {
                return Err(KeelError::validation("auth.error.redirect").field("redirect"));
            }
            let now = call.require::<dyn Clock>()?.now();
            let key = issue_row(
                call.tx()?,
                target,
                call.principal().id,
                &params.redirect,
                params.ttl,
                now,
            )
            .await?;
            Ok(Issued {
                url: url_of(key.expose()),
                key: key.expose().to_owned(),
            })
        })
}

pub fn redeem() -> FunctionDef {
    function::<Redeem, Redeemed>("auth.key.redeem")
        .kind(Kind::Form)
        .mode(Mode::Write)
        .level(Level::Guest)
        .unowned()
        .ui(Form::new("auth.key.redeem.title").page("main", ["key"]))
        .handler(|call: Call<Redeem>| async move {
            let client = http_client(&call)?;
            let now = call.require::<dyn Clock>()?.now();
            let tx = call.tx()?;
            let row = redeem_row(tx, &hash(call.params().key.expose()), now)
                .await?
                .ok_or_else(key_refused)?;
            let issued_by = issued_by(&row)?;
            let principal = login_principal(&call, principal_id(&row)?, key_refused).await?;
            let principal = open_session(&call, principal, client, now).await?;
            Ok(Redeemed {
                redirect: row.redirect,
                principal,
                issued_by,
            })
        })
}
