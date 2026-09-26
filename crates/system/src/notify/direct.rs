use std::sync::{Arc, OnceLock};

use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::Expr;
use keel_sdk::sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set,
};
use keel_sdk::serde_json;
use keel_sdk::system::{AddressSource, PrincipalSource, SecretCipher};

use super::channels::ChannelKind;
use super::channels::email::{self, Mail};
use super::consumer::{STATE_FAILED, STATE_PENDING, STATE_SENT};
use super::entity::{channel, direct};
use super::sender::{
    BATCH, Letter, channel_failed, channel_sent, dead_letter, mail_channel, next_attempt_at,
    reason_of, recipient,
};
use super::started::Started;
use super::templates::ERROR_MISSING;

const ERROR_PRINCIPAL: &str = "notify.error.principal_unknown";
const ERROR_NO_ADDRESS: &str = "notify.error.no_address";
const ERROR_NO_CHANNEL: &str = "notify.error.no_channel";

pub type Mailer = Arc<OnceLock<Arc<Started>>>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct SendTo {
    pub principal: Id,
    #[schemars(length(min = 1, max = 128))]
    pub template: String,
    #[serde(default)]
    #[schemars(extend("x-keel" = json!({ "secret": true })))]
    pub params: Value,
    #[serde(default)]
    pub unverified: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Sent {
    pub id: Id,
}

pub async fn enqueue(call: &Call<SendTo>, mailer: &Mailer) -> Result<Sent> {
    let params = call.params();
    let db = call.tx()?;
    if let Some(started) = mailer.get()
        && !started.templates.has(&params.template)
    {
        return Err(KeelError::validation(ERROR_MISSING)
            .field("template")
            .param("id", params.template.as_str()));
    }
    call.require::<dyn PrincipalSource>()?
        .by_id(params.principal, db)
        .await?
        .ok_or_else(|| KeelError::validation(ERROR_PRINCIPAL).field("principal"))?;
    let address = call
        .require::<dyn AddressSource>()?
        .email(params.principal, db)
        .await?
        .filter(|address| address.verified || params.unverified)
        .ok_or_else(|| KeelError::validation(ERROR_NO_ADDRESS).field("principal"))?;
    let channel = channel::Entity::find()
        .filter(channel::Column::Enabled.eq(true))
        .filter(channel::Column::Kind.eq(ChannelKind::Email.as_str()))
        .order_by_asc(channel::Column::CreatedAt)
        .order_by_asc(channel::Column::Id)
        .one(db)
        .await?
        .ok_or_else(|| KeelError::unavailable(ERROR_NO_CHANNEL))?;
    let sealed = call
        .require::<dyn SecretCipher>()?
        .seal(&serde_json::to_vec(&params.params)?)?;
    let now = call.require::<dyn Clock>()?.now();
    let id = Id::generate();
    direct::ActiveModel {
        id: Set(id.to_string()),
        principal_id: Set(params.principal.to_string()),
        channel_id: Set(channel.id),
        channel: Set(channel.name),
        template: Set(params.template.clone()),
        address: Set(address.value),
        params_sealed: Set(Some(sealed)),
        attempt: Set(0),
        state: Set(STATE_PENDING.to_owned()),
        next_at: Set(now),
        error: Set(None),
        sent_at: Set(None),
        created_at: Set(now),
    }
    .insert(db)
    .await?;
    if let Some(started) = mailer.get() {
        let started = started.clone();
        call.after_commit(move |_| async move {
            started.wake();
            Ok(())
        });
    }
    Ok(Sent { id })
}

pub async fn due(started: &Started) -> Result<usize> {
    let now = started.clock.now();
    let rows = direct::Entity::find()
        .filter(direct::Column::State.eq(STATE_PENDING))
        .filter(direct::Column::NextAt.lte(now))
        .order_by_asc(direct::Column::NextAt)
        .order_by_asc(direct::Column::Id)
        .limit(BATCH)
        .all(&started.db)
        .await?;
    let count = rows.len();
    for row in rows {
        attempt(started, row).await?;
    }
    Ok(count)
}

async fn attempt(started: &Started, row: direct::Model) -> Result<()> {
    let outcome = send(started, &row).await;
    let now = started.clock.now();
    let attempt = row.attempt + 1;
    let update = direct::Entity::update_many()
        .filter(direct::Column::Id.eq(row.id.clone()))
        .col_expr(direct::Column::Attempt, Expr::value(attempt));
    match outcome {
        Ok(()) => {
            update
                .col_expr(direct::Column::State, Expr::value(STATE_SENT))
                .col_expr(direct::Column::SentAt, Expr::value(Some(now)))
                .col_expr(direct::Column::Error, Expr::value(None::<String>))
                .col_expr(direct::Column::ParamsSealed, Expr::value(None::<String>))
                .exec(&started.db)
                .await?;
            channel_sent(&started.db, &row.channel_id, now).await?;
            tracing::info!(
                delivery = row.id,
                channel = row.channel,
                template = row.template,
                "delivered to one recipient"
            );
        }
        Err(error) => {
            let reason = reason_of(&error);
            tracing::warn!(delivery = row.id, channel = row.channel, attempt, error = %reason, "the direct delivery failed");
            let next = next_attempt_at(attempt, now);
            let update = match next {
                Some(at) => update
                    .col_expr(direct::Column::State, Expr::value(STATE_PENDING))
                    .col_expr(direct::Column::NextAt, Expr::value(at)),
                None => update
                    .col_expr(direct::Column::State, Expr::value(STATE_FAILED))
                    .col_expr(direct::Column::ParamsSealed, Expr::value(None::<String>)),
            };
            update
                .col_expr(direct::Column::Error, Expr::value(Some(reason.clone())))
                .exec(&started.db)
                .await?;
            channel_failed(&started.db, &row.channel_id, &reason).await?;
            if next.is_none() {
                dead_letter(
                    started.caller.as_ref(),
                    &Letter {
                        channel_id: &row.channel_id,
                        channel: &row.channel,
                        delivery: &row.id,
                        key: &row.template,
                    },
                    &reason,
                )
                .await;
            }
        }
    }
    Ok(())
}

async fn send(started: &Started, row: &direct::Model) -> Result<()> {
    let (channel, email, password) = mail_channel(started, &row.channel_id).await?;
    let params: Value = match &row.params_sealed {
        Some(sealed) => serde_json::from_slice(&started.cipher.open(sealed)?)?,
        None => Value::Null,
    };
    let (login, lang) = recipient(started, &row.principal_id).await?;
    let context = json!({
        "key": row.template,
        "principal": login,
        "lang": lang,
        "channel": channel.name,
        "params": params,
    });
    let rendered = started.templates.render(&row.template, &lang, &context)?;
    email::send(
        &email,
        password,
        &Mail {
            to: &row.address,
            subject: &rendered.subject,
            body: &rendered.body,
        },
    )
    .await
}
