use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ActiveModelTrait, EntityTrait, Set};
use keel_sdk::system::{LiveEvents, SecretCipher};
use keel_sdk::types::NotificationLevel;

use super::super::api::{KEY_TEST, SOURCE_TEST};
use super::super::channels::{
    Channel, ChannelKind, ERROR_EMAIL_MISSING, ERROR_KIND_UNAVAILABLE, EmailInput, StoredEmail,
    channel_of, email_of,
};
use super::super::entity::channel;
use super::super::messages::{Outgoing, insert, publish_after_commit};
use super::Pick;

pub const CHANNEL_LIST: &str = "notify.channel.list";
pub const CHANNEL_GET: &str = "notify.channel.get";
pub const CHANNEL_SET: &str = "notify.channel.set";
pub const CHANNEL_DELETE: &str = "notify.channel.delete";
pub const CHANNEL_TEST: &str = "notify.channel.test";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct ChannelSet {
    pub id: Option<Id>,
    pub kind: ChannelKind,
    #[schemars(length(min = 1, max = 120))]
    pub name: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    #[serde(default)]
    pub email: Option<EmailInput>,
}

fn enabled_by_default() -> bool {
    true
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct ChannelDeleted {
    pub deleted: bool,
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Tested {
    pub message: Id,
}

struct ChannelSource;

#[async_trait]
impl ListSource for ChannelSource {
    type Row = Channel;

    async fn fetch(&self, call: &Call<ListQuery>, spec: &ListSpec) -> Result<Page<Channel>> {
        let page = fetch_page(channel::Entity::find(), call, spec).await?;
        Ok(Page {
            data: page
                .data
                .into_iter()
                .map(channel_of)
                .collect::<Result<Vec<_>>>()?,
            total: page.total,
            totals: page.totals,
        })
    }
}

pub fn channel_list() -> FunctionDef {
    let kinds: Vec<&str> = ChannelKind::ALL.iter().map(|kind| kind.as_str()).collect();
    function::<ListQuery, Page<Channel>>(CHANNEL_LIST)
        .kind(Kind::List)
        .level(Level::Admin)
        .unowned()
        .key("id")
        .ui(List::new("notify.channel.list.title")
            .search(["name"])
            .column(
                Column::new("name", ColumnType::Link)
                    .to(CHANNEL_SET, [("id", "$id")])
                    .sortable()
                    .primary(),
            )
            .column(
                Column::new("kind", ColumnType::Status)
                    .map([
                        (ChannelKind::Email.as_str(), Tone::Info),
                        (ChannelKind::Telegram.as_str(), Tone::Neutral),
                        (ChannelKind::Webhook.as_str(), Tone::Neutral),
                    ])
                    .filter_options(kinds.iter().copied()),
            )
            .column(Column::new("enabled", ColumnType::Boolean).sortable())
            .column(Column::new("lastSentAt", ColumnType::Datetime).sortable())
            .column(Column::new("lastError", ColumnType::Text))
            .column(Column::new("createdAt", ColumnType::Datetime).sortable())
            .action(
                Action::new("create", ActionRole::New)
                    .screen(CHANNEL_SET)
                    .icon("plus"),
            )
            .action(
                Action::new("edit", ActionRole::Edit)
                    .screen(CHANNEL_SET)
                    .default(),
            )
            .action(
                Action::new("test", ActionRole::Button)
                    .function(CHANNEL_TEST)
                    .params([("id", "$id")])
                    .icon("send"),
            )
            .action(
                Action::new("delete", ActionRole::Delete)
                    .function(CHANNEL_DELETE)
                    .confirm_with(ConfirmDetails {
                        destructive: true,
                        items: Some("name".to_owned()),
                        input: None,
                    }),
            )
            .row_actions(["edit", "test", "delete"])
            .empty("notify.channel.empty", Some("create"))
            .invalidate_on(["notify.channel.*"]))
        .default_sort("name", SortDir::Asc)
        .source(ChannelSource)
}

async fn load(db: &Db, id: Id) -> Result<channel::Model> {
    channel::Entity::find_by_id(id.to_string())
        .one(db)
        .await?
        .ok_or_else(|| KeelError::not_found("keel.error.not_found"))
}

pub fn channel_get() -> FunctionDef {
    function::<Pick, Channel>(CHANNEL_GET)
        .kind(Kind::Get)
        .level(Level::Admin)
        .unowned()
        .key("id")
        .handler(
            |call: Call<Pick>| async move { channel_of(load(call.db(), call.params().id).await?) },
        )
}

pub fn channel_set() -> FunctionDef {
    function::<ChannelSet, Channel>(CHANNEL_SET)
        .kind(Kind::Form)
        .level(Level::Admin)
        .unowned()
        .key("id")
        .ui(Form::new("notify.channel.set.title")
            .read(CHANNEL_GET)
            .page("main", ["kind", "name", "enabled"])
            .page("email", ["email"])
            .show_if("kind == 'email'"))
        .handler(|call: Call<ChannelSet>| async move {
            let params = call.params();
            if !params.kind.is_available() {
                return Err(KeelError::validation(ERROR_KIND_UNAVAILABLE)
                    .field("kind")
                    .param("kind", params.kind.as_str()));
            }
            let Some(email) = &params.email else {
                return Err(KeelError::validation(ERROR_EMAIL_MISSING).field("email"));
            };
            let cipher = call.require::<dyn SecretCipher>()?;
            let now = call.require::<dyn Clock>()?.now();
            let tx = call.tx()?;
            let previous = match params.id {
                Some(id) => Some(load(tx, id).await?),
                None => None,
            };
            let stored_email = match &previous {
                Some(row) if row.kind == ChannelKind::Email.as_str() => Some(email_of(row)?),
                _ => None,
            };
            let sealed = StoredEmail::seal(email, stored_email.as_ref(), cipher.as_ref())?;
            let config = keel_sdk::serde_json::to_value(&sealed)?;
            let row = match previous {
                Some(row) => {
                    let mut active: channel::ActiveModel = row.into();
                    active.kind = Set(params.kind.as_str().to_owned());
                    active.name = Set(params.name.clone());
                    active.enabled = Set(params.enabled);
                    active.config_json = Set(config);
                    active.update(tx).await?
                }
                None => {
                    channel::ActiveModel {
                        id: Set(Id::generate().to_string()),
                        kind: Set(params.kind.as_str().to_owned()),
                        name: Set(params.name.clone()),
                        enabled: Set(params.enabled),
                        config_json: Set(config),
                        last_sent_at: Set(None),
                        last_error: Set(None),
                        created_at: Set(now),
                        created_by: Set(call.principal().login.clone()),
                    }
                    .insert(tx)
                    .await?
                }
            };
            channel_of(row)
        })
}

pub fn channel_delete() -> FunctionDef {
    function::<Pick, ChannelDeleted>(CHANNEL_DELETE)
        .kind(Kind::Action)
        .level(Level::Admin)
        .unowned()
        .key("id")
        .handler(|call: Call<Pick>| async move {
            let deleted = channel::Entity::delete_by_id(call.params().id.to_string())
                .exec(call.tx()?)
                .await?;
            if deleted.rows_affected == 0 {
                return Err(KeelError::not_found("keel.error.not_found"));
            }
            Ok(ChannelDeleted { deleted: true })
        })
}

pub fn channel_test() -> FunctionDef {
    function::<Pick, Tested>(CHANNEL_TEST)
        .kind(Kind::Action)
        .level(Level::Admin)
        .unowned()
        .key("id")
        .handler(|call: Call<Pick>| async move {
            let now = call.require::<dyn Clock>()?.now();
            let live = call.require::<dyn LiveEvents>()?;
            let channel = load(call.tx()?, call.params().id).await?;
            let params = json!({ "channel": channel.name, "channelId": channel.id });
            let outgoing = Outgoing {
                key: KEY_TEST,
                params: &params,
                level: NotificationLevel::Info,
                source: SOURCE_TEST,
                problem: None,
                channel: Some(call.params().id),
            };
            let (message, frame) = insert(&call, call.principal(), &outgoing, now).await?;
            publish_after_commit(&call, live, vec![frame]);
            Ok(Tested { message })
        })
}
