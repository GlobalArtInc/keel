use keel_sdk::prelude::*;
use keel_sdk::system::{PeerReach, PrincipalSource};
use keel_sdk::types::{MetaPrincipal, Via};

use super::super::functions::{below_caller, http_client, login_principal, open_session};
use super::super::mail::{SETTING_SHELL_URL, shell_base};
use super::{
    MAX_TTL_SECONDS, hash, is_local_path, issue as issue_row, issued_by, principal_id,
    redeem as redeem_row, url_of,
};

pub const PERMISSION_ISSUE: &str = "auth.key.issue";
pub const ERROR_TARGET: &str = "auth.error.key_target";
pub const ERROR_EXTERNAL_ID: &str = "auth.error.external_id";
pub const MAX_EXTERNAL_ID: usize = 256;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub struct Issue {
    #[serde(default)]
    pub principal: Option<Id>,
    #[serde(default)]
    #[schemars(length(min = 1, max = MAX_EXTERNAL_ID))]
    pub external_id: Option<String>,
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

fn through_peer(principal: &Principal) -> bool {
    matches!(principal.via, Via::Peer(_))
}

fn unknown_target() -> KeelError {
    KeelError::not_found("keel.error.not_found")
}

async fn target_of(call: &Call<Issue>) -> Result<Id> {
    let params = call.params();
    let link = match call.principal().via {
        Via::Peer(link) => Some(link),
        _ => None,
    };
    match (params.principal, params.external_id.as_deref(), link) {
        (Some(_), Some(_), _) => Err(KeelError::validation(ERROR_TARGET).field("externalId")),
        (None, None, _) => Err(KeelError::validation(ERROR_TARGET).field("principal")),
        (Some(principal), None, None) => Ok(principal),
        (Some(principal), None, Some(link)) => {
            let found = call
                .require::<dyn PrincipalSource>()?
                .by_id(principal, call.db())
                .await?;
            reached(call, link, found).await?.ok_or_else(unknown_target)
        }
        (None, Some(_), None) => Err(KeelError::validation(ERROR_EXTERNAL_ID).field("externalId")),
        (None, Some(external_id), Some(link)) => {
            let found = by_external_id(call, link, external_id).await?;
            reached(call, link, found)
                .await?
                .ok_or_else(|| unknown_target().field("externalId"))
        }
    }
}

async fn by_external_id(
    call: &Call<Issue>,
    link: Id,
    external_id: &str,
) -> Result<Option<Principal>> {
    let caller = call.principal();
    if caller.actor.is_none() {
        return call
            .require::<dyn PeerReach>()?
            .customer(link, external_id, call.db())
            .await;
    }
    match caller.origin.as_deref() {
        Some(origin) => {
            call.require::<dyn PrincipalSource>()?
                .by_external(origin, external_id, call.db())
                .await
        }
        None => Ok(None),
    }
}

async fn reached(call: &Call<Issue>, link: Id, found: Option<Principal>) -> Result<Option<Id>> {
    let Some(principal) = found else {
        return Ok(None);
    };
    let reach = call.require::<dyn PeerReach>()?;
    Ok(reach
        .reaches(link, &principal, call.db())
        .await?
        .then_some(principal.id))
}

async fn login_url(call: &Call<Issue>, key: &str) -> Result<String> {
    let url = url_of(key);
    if !through_peer(call.principal()) {
        return Ok(url);
    }
    let shell: String = call.setting(SETTING_SHELL_URL).await?;
    Ok(match shell_base(&shell) {
        Some(base) => format!("{base}{url}"),
        None => url,
    })
}

pub fn issue() -> FunctionDef {
    function::<Issue, Issued>("auth.key.issue")
        .kind(Kind::Form)
        .access(Level::Reseller, PERMISSION_ISSUE)
        .unowned()
        .ui(Form::new("auth.key.issue.title")
            .page("main", ["principal", "ttl", "redirect"])
            .field("principal")
            .required_if("externalId == null"))
        .handler(|call: Call<Issue>| async move {
            let params = call.params();
            let target = below_caller(&call, target_of(&call).await?).await?;
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
                url: login_url(&call, key.expose()).await?,
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
        .exempt_from_read_only()
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
