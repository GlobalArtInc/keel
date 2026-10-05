use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::Expr;
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use keel_sdk::serde_json;
use keel_sdk::system::{Caller, CallerOptions, Features, SecretCipher};
use keel_sdk::types::InstanceInfo;

use super::check::{Verdict, verdict};
use super::continuation;
use super::entity::product;
use super::functions::{EVENT_ROTATED, Pick, not_found, retire};
use super::pairing::{ERROR_INSTANCE_KEY, ERROR_PROTOCOL, ERROR_TOKEN};
use super::remote::{Presented, Remote};
use crate::auth::api::{is_peer_token, peer_token, token_hash};

pub const ROTATE: &str = "peer.rotate";
pub const RENEW: &str = "peer.renew";
pub const REKEY: &str = "peer.rekey";
pub const ERROR_NO_URL: &str = "peer.error.no_url";
pub const ERROR_PROOF: &str = "peer.error.proof";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub(super) struct Renew {
    #[schemars(extend("x-keel" = json!({ "secret": true })))]
    token: Secret,
    #[schemars(extend("x-keel" = json!({ "secret": true })))]
    inbound_token: Secret,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub(super) struct Renewed {
    #[schemars(extend("x-keel" = json!({ "once": true })))]
    token: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub(super) struct Rekey {
    id: Id,
    #[schemars(extend("x-keel" = json!({ "secret": true })))]
    inbound_token: Secret,
    #[schemars(extend("x-keel" = json!({ "secret": true })))]
    outbound_token: Secret,
    instance: InstanceInfo,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub(super) struct Rotated {
    rotated: bool,
}

pub(super) fn presented_by(row: &product::Model, presented: &Presented, url: &str) -> Result<()> {
    match verdict(row, presented) {
        Verdict::Refused => {
            Err(KeelError::forbidden(ERROR_INSTANCE_KEY)
                .param("instanceId", row.instance_id.as_str()))
        }
        Verdict::Unproven => Err(KeelError::forbidden(ERROR_PROOF)
            .param("instanceId", row.instance_id.as_str())
            .param("url", url)),
        Verdict::Incompatible => Err(KeelError::conflict(ERROR_PROTOCOL)
            .param("protocol", presented.instance.protocol)
            .param("supported", keel_sdk::types::PROTOCOL)),
        Verdict::Compatible => Ok(()),
    }
}

pub(super) fn rotate() -> FunctionDef {
    function::<Pick, Rotated>(ROTATE)
        .kind(Kind::Action)
        .mode(Mode::Read)
        .level(Level::Admin)
        .key("id")
        .unowned()
        .handler(|call: Call<Pick>| async move {
            let principal = continuation(
                call.principal(),
                call.require::<dyn Features>().ok().as_deref(),
                ROTATE,
            )?;
            let id = call.params().id;
            let row = product::Entity::find_by_id(id.to_string())
                .one(call.db())
                .await?
                .ok_or_else(not_found)?;
            let Some(url) = row.url.as_deref() else {
                return Err(
                    KeelError::conflict(ERROR_NO_URL).param("instanceId", row.instance_id.as_str())
                );
            };
            let remote = Remote::new(url)?;
            let presented = remote.proven().await?;
            presented_by(&row, &presented, remote.base())?;
            let presented: InstanceInfo = presented.instance;
            let cipher = call.require::<dyn SecretCipher>()?;
            let outbound =
                String::from_utf8(cipher.open(&row.outbound_token_enc)?).map_err(|_| {
                    KeelError::internal(super::functions::ERROR_STORED).param("id", row.id.as_str())
                })?;
            let inbound = peer_token()?;
            let renewed: Renewed = remote
                .call(
                    RENEW,
                    &json!({ "token": outbound, "inboundToken": inbound.expose() }),
                )
                .await?;
            if !is_peer_token(&renewed.token) {
                return Err(
                    KeelError::peer(super::remote::ERROR_ANSWER).param("url", remote.base())
                );
            }
            call.require::<dyn Caller>()?
                .call(
                    principal,
                    REKEY,
                    json!({
                        "id": id,
                        "inboundToken": inbound.expose(),
                        "outboundToken": renewed.token,
                        "instance": presented,
                    }),
                    CallerOptions {
                        request_id: Some(call.request_id().to_owned()),
                        task: None,
                    },
                )
                .await?;
            Ok(Rotated { rotated: true })
        })
}

pub(super) fn renew() -> FunctionDef {
    function::<Renew, Renewed>(RENEW)
        .kind(Kind::Action)
        .level(Level::Guest)
        .unowned()
        .handler(|call: Call<Renew>| async move {
            let params = call.params();
            if !is_peer_token(params.inbound_token.expose()) {
                return Err(KeelError::validation(ERROR_TOKEN).field("inboundToken"));
            }
            let now = call.require::<dyn Clock>()?.now();
            let cipher = call.require::<dyn SecretCipher>()?;
            let token = peer_token()?;
            let renewed = product::Entity::update_many()
                .col_expr(
                    product::Column::InboundTokenHash,
                    Expr::value(token_hash(token.expose())),
                )
                .col_expr(
                    product::Column::OutboundTokenEnc,
                    Expr::value(cipher.seal(params.inbound_token.expose().as_bytes())?),
                )
                .col_expr(product::Column::LastSeenAt, Expr::value(now))
                .filter(product::Column::InboundTokenHash.eq(token_hash(params.token.expose())))
                .exec_with_returning(call.tx()?)
                .await?
                .pop()
                .ok_or_else(|| KeelError::unauthenticated(ERROR_TOKEN))?;
            retire(&call, &renewed).await?;
            call.emit(
                EVENT_ROTATED,
                &json!({ "id": renewed.id, "instanceId": renewed.instance_id }),
            )?;
            Ok(Renewed {
                token: token.into_inner(),
            })
        })
}

pub(super) fn rekey() -> FunctionDef {
    function::<Rekey, Rotated>(REKEY)
        .kind(Kind::Action)
        .level(Level::Admin)
        .key("id")
        .unowned()
        .internal()
        .handler(|call: Call<Rekey>| async move {
            let params = call.params();
            let now = call.require::<dyn Clock>()?.now();
            let cipher = call.require::<dyn SecretCipher>()?;
            let instance = &params.instance;
            let rekeyed = product::Entity::update_many()
                .col_expr(
                    product::Column::InboundTokenHash,
                    Expr::value(token_hash(params.inbound_token.expose())),
                )
                .col_expr(
                    product::Column::OutboundTokenEnc,
                    Expr::value(cipher.seal(params.outbound_token.expose().as_bytes())?),
                )
                .col_expr(
                    product::Column::ProductsJson,
                    Expr::value(serde_json::to_value(&instance.products)?),
                )
                .col_expr(
                    product::Column::KeelVersion,
                    Expr::value(instance.keel.as_str()),
                )
                .col_expr(
                    product::Column::Protocol,
                    Expr::value(i32::try_from(instance.protocol).unwrap_or(i32::MAX)),
                )
                .col_expr(product::Column::LastSeenAt, Expr::value(now))
                .filter(product::Column::Id.eq(params.id.to_string()))
                .filter(product::Column::InstanceId.eq(instance.instance_id.to_string()))
                .filter(product::Column::PublicKey.eq(instance.public_key.as_str()))
                .exec_with_returning(call.tx()?)
                .await?
                .pop()
                .ok_or_else(not_found)?;
            retire(&call, &rekeyed).await?;
            call.emit(
                EVENT_ROTATED,
                &json!({ "id": rekeyed.id, "instanceId": rekeyed.instance_id }),
            )?;
            Ok(Rotated { rotated: true })
        })
}
