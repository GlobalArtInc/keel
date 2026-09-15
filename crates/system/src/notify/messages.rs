use std::collections::BTreeSet;

use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ActiveModelTrait, Set};
use keel_sdk::system::{Frame, LiveEvents, PrincipalSource};
use keel_sdk::types::{NotificationFrame, NotificationLevel};

use super::api::{Created, EVENT_MESSAGE_CREATED, FRAME_KIND, Message, Recipients, level_of};
use super::entity::message;

pub const ERROR_ID: &str = "notify.error.id";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub struct Note {
    pub id: Id,
    pub key: String,
    pub params: Value,
    pub level: NotificationLevel,
    pub source: String,
    pub problem: Option<Id>,
    pub unread: bool,
    pub read_at: Option<keel_sdk::chrono::DateTime<keel_sdk::chrono::Utc>>,
    pub created_at: keel_sdk::chrono::DateTime<keel_sdk::chrono::Utc>,
}

pub fn note_of(row: message::Model) -> Result<Note> {
    let id: Id = row
        .id
        .parse()
        .map_err(|_| KeelError::internal(ERROR_ID).param("id", row.id.as_str()))?;
    let problem = row
        .problem_id
        .as_deref()
        .map(|problem| {
            problem
                .parse::<Id>()
                .map_err(|_| KeelError::internal(ERROR_ID).param("id", problem))
        })
        .transpose()?;
    Ok(Note {
        id,
        key: row.key,
        params: row.params_json,
        level: level_of(&row.level, &row.id)?,
        source: row.source,
        problem,
        unread: row.unread,
        read_at: row.read_at,
        created_at: row.created_at,
    })
}

async fn resolve(call: &Call<Message>) -> Result<Vec<Principal>> {
    let source = call.require::<dyn PrincipalSource>()?;
    let db = call.db();
    let mut seen = BTreeSet::new();
    let mut resolved = Vec::new();
    for recipients in &call.params().recipients {
        let found = match recipients {
            Recipients::Admins => source.at_or_above(Level::Admin, db).await?,
            Recipients::Principal(id) => source.by_id(*id, db).await?.into_iter().collect(),
            Recipients::Account(account) => source.of_account(*account, db).await?,
        };
        for principal in found {
            if seen.insert(principal.id) {
                resolved.push(principal);
            }
        }
    }
    Ok(resolved)
}

#[derive(Serialize)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
struct CreatedEvent<'a> {
    id: Id,
    principal: Id,
    account: Id,
    key: &'a str,
    level: NotificationLevel,
    source: &'a str,
    problem: Option<Id>,
    channel: Option<Id>,
}

pub struct Outgoing<'a> {
    pub key: &'a str,
    pub params: &'a Value,
    pub level: NotificationLevel,
    pub source: &'a str,
    pub problem: Option<Id>,
    pub channel: Option<Id>,
}

pub(super) async fn insert<P: Send + Sync>(
    call: &Call<P>,
    recipient: &Principal,
    outgoing: &Outgoing<'_>,
    now: keel_sdk::chrono::DateTime<keel_sdk::chrono::Utc>,
) -> Result<(Id, Frame)> {
    let id = Id::generate();
    message::ActiveModel {
        id: Set(id.to_string()),
        principal_id: Set(recipient.id.to_string()),
        account_id: Set(recipient.account_id.to_string()),
        key: Set(outgoing.key.to_owned()),
        params_json: Set(outgoing.params.clone()),
        level: Set(outgoing.level.as_str().to_owned()),
        source: Set(outgoing.source.to_owned()),
        problem_id: Set(outgoing.problem.map(|problem| problem.to_string())),
        read_at: Set(None),
        created_at: Set(now),
        unread: keel_sdk::sea_orm::ActiveValue::NotSet,
    }
    .insert(call.tx()?)
    .await?;
    call.emit(
        EVENT_MESSAGE_CREATED,
        &CreatedEvent {
            id,
            principal: recipient.id,
            account: recipient.account_id,
            key: outgoing.key,
            level: outgoing.level,
            source: outgoing.source,
            problem: outgoing.problem,
            channel: outgoing.channel,
        },
    )?;
    let frame = Frame {
        principal: recipient.id,
        account: recipient.account_id,
        kind: FRAME_KIND.to_owned(),
        data: keel_sdk::serde_json::to_value(NotificationFrame {
            id,
            key: outgoing.key.to_owned(),
            params: outgoing.params.clone(),
            level: outgoing.level,
            created_at: now,
        })?,
    };
    Ok((id, frame))
}

pub(super) fn publish_after_commit<P>(
    call: &Call<P>,
    live: Arc<dyn LiveEvents>,
    frames: Vec<Frame>,
) {
    call.after_commit(move |_| async move {
        for frame in frames {
            live.publish(frame);
        }
        Ok(())
    });
}

pub(super) async fn create(call: &Call<Message>) -> Result<Created> {
    let now = call.require::<dyn Clock>()?.now();
    let live = call.require::<dyn LiveEvents>()?;
    let message = call.params();
    let outgoing = Outgoing {
        key: &message.key,
        params: &message.params,
        level: message.level,
        source: &message.source,
        problem: message.problem,
        channel: None,
    };
    let mut ids = Vec::new();
    let mut frames = Vec::new();
    for recipient in resolve(call).await? {
        let (id, frame) = insert(call, &recipient, &outgoing, now).await?;
        frames.push(frame);
        ids.push(id);
    }
    publish_after_commit(call, live, frames);
    Ok(Created { ids })
}

#[cfg(test)]
mod tests {
    use keel_sdk::chrono::Utc;
    use keel_sdk::serde_json::json;

    use super::*;

    #[test]
    fn a_row_becomes_a_note_with_its_level_and_an_unknown_level_is_named() {
        let row = message::Model {
            id: "01J9Q3W6B1R8N5X2K4T7M0V9Z3".into(),
            principal_id: "01J9Q3W6B1R8N5X2K4T7M0V9A1".into(),
            account_id: "01J8A0000000000000000000A1".into(),
            key: "notify.problem.raised".into(),
            params_json: json!({ "key": "note.overdue" }),
            level: "warning".into(),
            source: "problem".into(),
            problem_id: Some("01J9Q3W6B1R8N5X2K4T7M0V9Z4".into()),
            read_at: None,
            created_at: Utc::now(),
            unread: true,
        };
        let note = note_of(row.clone()).expect("a valid row");
        assert_eq!(note.level, NotificationLevel::Warning);
        assert!(note.unread);
        assert_eq!(
            note.problem.map(|id| id.to_string()).as_deref(),
            Some("01J9Q3W6B1R8N5X2K4T7M0V9Z4")
        );
        let mut broken = row;
        broken.level = "loud".into();
        assert_eq!(
            note_of(broken).expect_err("loud is not a level").key(),
            super::super::api::ERROR_LEVEL
        );
    }
}
