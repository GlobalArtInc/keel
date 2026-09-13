use keel_sdk::prelude::*;
use keel_sdk::system::PrincipalSource;
use keel_sdk::types::{ActorVia, MetaActor, MetaPrincipal, Via};

use super::functions::{Nothing, below_caller, session_client};
use super::session;

pub const PERMISSION: &str = "auth.su";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Su {
    pub principal: Id,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Delegation {
    pub principal: MetaPrincipal,
}

fn not_delegated() -> KeelError {
    KeelError::validation("auth.error.not_delegated")
}

pub fn su() -> FunctionDef {
    function::<Su, Delegation>("auth.su")
        .kind(Kind::Action)
        .access(Level::Reseller, PERMISSION)
        .unowned()
        .handler(|call: Call<Su>| async move {
            let (client, id_hash) = session_client(&call)?;
            let caller = call.principal();
            if caller.actor.is_some() {
                return Err(KeelError::conflict("auth.error.su_nested"));
            }
            let target = below_caller(&call, call.params().principal).await?;
            let principals = call.require::<dyn PrincipalSource>()?;
            let entered = principals
                .by_id(target, call.db())
                .await?
                .ok_or_else(|| KeelError::not_found("keel.error.not_found").field("principal"))?;
            let tx = call.tx()?;
            let parent = session::row(tx, &id_hash)
                .await?
                .ok_or_else(|| KeelError::validation("auth.error.session_only"))?;
            let now = call.require::<dyn Clock>()?.now();
            session::open_su(tx, &parent, &entered, caller.id, client, now).await?;
            Ok(Delegation {
                principal: MetaPrincipal {
                    id: entered.id,
                    login: entered.login,
                    level: entered.level,
                    account: entered.account_id,
                    actor: Some(MetaActor {
                        id: caller.id,
                        login: caller.login.clone(),
                        level: caller.level,
                        via: ActorVia::Su,
                    }),
                },
            })
        })
}

pub fn exit() -> FunctionDef {
    function::<Nothing, Delegation>("auth.su.exit")
        .kind(Kind::Action)
        .level(Level::Service)
        .unowned()
        .handler(|call: Call<Nothing>| async move {
            let (_, id_hash) = session_client(&call)?;
            let actor = call
                .principal()
                .actor
                .as_deref()
                .filter(|actor| actor.via == Via::Su)
                .ok_or_else(not_delegated)?;
            let tx = call.tx()?;
            let child = session::row(tx, &id_hash)
                .await?
                .and_then(|parent| parent.su_id_hash)
                .ok_or_else(not_delegated)?;
            let now = call.require::<dyn Clock>()?.now();
            session::revoke(tx, &child, now, session::REASON_SU_EXIT).await?;
            session::delegate(tx, &id_hash, None).await?;
            Ok(Delegation {
                principal: MetaPrincipal {
                    id: actor.id,
                    login: actor.login.clone(),
                    level: actor.level,
                    account: actor.account_id,
                    actor: None,
                },
            })
        })
}
