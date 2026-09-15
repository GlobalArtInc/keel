use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::Expr;
use keel_sdk::sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use keel_sdk::system::SecretCipher;
use rand::TryRng;
use rand::rngs::SysRng;

use super::super::entity::subscription;
use super::super::masks;
use super::super::started::private_targets;
use super::super::targets::{self, Resolver, resolver_or_system};
use super::deliveries::DELIVERY_LIST;
use super::{PERMISSION_MANAGE, Pick, Subscription, load_subscription, subscription_of};

pub const SUBSCRIPTION_LIST: &str = "webhook.subscription.list";
pub const SUBSCRIPTION_GET: &str = "webhook.subscription.get";
pub const SUBSCRIPTION_SET: &str = "webhook.subscription.set";
pub const SUBSCRIPTION_TOGGLE: &str = "webhook.subscription.toggle";
pub const SUBSCRIPTION_DELETE: &str = "webhook.subscription.delete";
pub const SECRET_MIN_BYTES: usize = 16;
pub const SECRET_GENERATED_BYTES: usize = 32;
pub const ERROR_SECRET_SHORT: &str = "webhook.error.secret_short";
pub const ERROR_MASK: &str = "webhook.error.mask";
pub const ERROR_ENTROPY: &str = "webhook.error.entropy";

const BASE64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct SubscriptionSet {
    pub id: Option<Id>,
    #[schemars(length(min = 1, max = 120))]
    pub name: String,
    #[schemars(url, length(min = 1, max = 2048))]
    pub url: String,
    #[serde(default)]
    #[schemars(extend("x-keel" = json!({ "secret": true }), "x-ui" = json!({ "control": "password" })))]
    pub secret: Option<Secret>,
    #[schemars(length(min = 1, max = 64), extend("x-ui" = json!({ "control": "tags" })))]
    pub events: Vec<String>,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    #[serde(default)]
    #[schemars(extend("x-keel" = json!({ "level": "reseller" })))]
    pub account: Option<Id>,
}

fn enabled_by_default() -> bool {
    true
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct SubscriptionToggle {
    pub id: Id,
    pub enabled: bool,
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct SubscriptionSaved {
    #[serde(flatten)]
    pub subscription: Subscription,
    #[schemars(extend("x-keel" = json!({ "once": true })))]
    pub secret: Option<String>,
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct SubscriptionDeleted {
    pub deleted: bool,
}

pub fn base64url(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let word = chunk.iter().enumerate().fold(0u32, |word, (i, byte)| {
            word | (u32::from(*byte) << (16 - 8 * i))
        });
        let symbols = chunk.len() + 1;
        for i in 0..symbols {
            let index = ((word >> (18 - 6 * i)) & 0x3f) as usize;
            out.push(char::from(BASE64URL[index]));
        }
    }
    out
}

pub fn generate_secret() -> Result<String> {
    let mut bytes = vec![0u8; SECRET_GENERATED_BYTES];
    SysRng.try_fill_bytes(&mut bytes).map_err(|error| {
        KeelError::internal(ERROR_ENTROPY).source(std::io::Error::other(error.to_string()))
    })?;
    Ok(base64url(&bytes))
}

pub fn check_masks(events: &[String]) -> Result<()> {
    for (index, mask) in events.iter().enumerate() {
        if !masks::is_valid(mask) {
            return Err(KeelError::validation(ERROR_MASK)
                .field(format!("events[{index}]"))
                .param("mask", mask.as_str()));
        }
    }
    Ok(())
}

struct SubscriptionSource;

#[async_trait]
impl ListSource for SubscriptionSource {
    type Row = Subscription;

    async fn fetch(&self, call: &Call<ListQuery>, spec: &ListSpec) -> Result<Page<Subscription>> {
        let page = fetch_page(
            call.account_scope().apply(subscription::Entity::find()),
            call,
            spec,
        )
        .await?;
        Ok(Page {
            data: page.data.into_iter().map(subscription_of).collect(),
            total: page.total,
            totals: page.totals,
        })
    }
}

pub fn subscription_list() -> FunctionDef {
    function::<ListQuery, Page<Subscription>>(SUBSCRIPTION_LIST)
        .kind(Kind::List)
        .access(Level::User, PERMISSION_MANAGE)
        .owned::<subscription::Entity>()
        .ui(List::new("webhook.subscription.list.title")
            .search(["name", "url"])
            .column(
                Column::new("name", ColumnType::Link)
                    .to(SUBSCRIPTION_SET, [("id", "$id")])
                    .sortable()
                    .primary(),
            )
            .column(Column::new("url", ColumnType::Text).sortable())
            .column(Column::new("events", ColumnType::Text))
            .column(
                Column::new("enabled", ColumnType::Toggle)
                    .sortable()
                    .edit(SUBSCRIPTION_TOGGLE),
            )
            .column(Column::new("createdAt", ColumnType::Datetime).sortable())
            .action(
                Action::new("create", ActionRole::New)
                    .screen(SUBSCRIPTION_SET)
                    .icon("plus"),
            )
            .action(
                Action::new("edit", ActionRole::Edit)
                    .screen(SUBSCRIPTION_SET)
                    .default(),
            )
            .action(
                Action::new("deliveries", ActionRole::Child)
                    .screen(DELIVERY_LIST)
                    .params([("subscription_id", "$id")])
                    .icon("list"),
            )
            .action(
                Action::new("delete", ActionRole::Delete)
                    .function(SUBSCRIPTION_DELETE)
                    .confirm_with(ConfirmDetails {
                        destructive: true,
                        items: Some("name".to_owned()),
                        input: None,
                    }),
            )
            .row_actions(["edit", "deliveries", "delete"])
            .empty("webhook.subscription.empty", Some("create"))
            .invalidate_on(["webhook.subscription.*"]))
        .default_sort("name", SortDir::Asc)
        .source(SubscriptionSource)
}

pub fn subscription_get() -> FunctionDef {
    function::<Pick, Subscription>(SUBSCRIPTION_GET)
        .kind(Kind::Get)
        .access(Level::User, PERMISSION_MANAGE)
        .owned::<subscription::Entity>()
        .handler(|call: Call<Pick>| async move {
            Ok(subscription_of(
                load_subscription(call.db(), call.params().id).await?,
            ))
        })
}

fn chosen_secret(secret: Option<&Secret>) -> Result<Option<&str>> {
    let Some(secret) = secret else {
        return Ok(None);
    };
    let text = secret.expose();
    if text.is_empty() {
        return Ok(None);
    }
    if text.len() < SECRET_MIN_BYTES {
        return Err(KeelError::validation(ERROR_SECRET_SHORT)
            .field("secret")
            .param("min", SECRET_MIN_BYTES));
    }
    Ok(Some(text))
}

pub fn subscription_set() -> FunctionDef {
    function::<SubscriptionSet, SubscriptionSaved>(SUBSCRIPTION_SET)
        .kind(Kind::Form)
        .access(Level::User, PERMISSION_MANAGE)
        .owned::<subscription::Entity>()
        .ui(Form::new("webhook.subscription.set.title")
            .read(SUBSCRIPTION_GET)
            .page("main", ["name", "url", "secret", "events", "enabled"])
            .page("owner", ["account"])
            .show_if("$principal.level != 'user'"))
        .handler(|call: Call<SubscriptionSet>| async move {
            let params = call.params();
            check_masks(&params.events)?;
            let chosen = chosen_secret(params.secret.as_ref())?;
            let settings = call.require::<dyn SettingsSource>()?;
            let allow_private = private_targets(settings.as_ref(), call.db()).await?;
            let resolver = resolver_or_system(call.require::<dyn Resolver>().ok());
            targets::check(&params.url, allow_private, resolver.as_ref()).await?;
            let cipher = call.require::<dyn SecretCipher>()?;
            let now = call.require::<dyn Clock>()?.now();
            let tx = call.tx()?;
            let previous = match params.id {
                Some(id) => Some(load_subscription(tx, id).await?),
                None => None,
            };
            let account = match (params.account, &previous) {
                (None, Some(row)) => row.account_id.clone(),
                (requested, _) => call.resolve_account(requested).await?.to_string(),
            };
            let (sealed, answered) = match (chosen, &previous) {
                (Some(chosen), _) => (cipher.seal(chosen.as_bytes())?, Some(chosen.to_owned())),
                (None, Some(row)) => (row.secret.clone(), None),
                (None, None) => {
                    let generated = generate_secret()?;
                    (cipher.seal(generated.as_bytes())?, Some(generated))
                }
            };
            let row = match previous {
                Some(row) => {
                    let mut active: subscription::ActiveModel = row.into();
                    active.account_id = Set(account);
                    active.name = Set(params.name.clone());
                    active.url = Set(params.url.clone());
                    active.secret = Set(sealed);
                    active.events = Set(params.events.clone());
                    active.enabled = Set(params.enabled);
                    active.updated_at = Set(now);
                    active.update(tx).await?
                }
                None => {
                    subscription::ActiveModel {
                        id: Set(Id::generate().to_string()),
                        account_id: Set(account),
                        principal_id: Set(call.principal().id.to_string()),
                        name: Set(params.name.clone()),
                        url: Set(params.url.clone()),
                        secret: Set(sealed),
                        events: Set(params.events.clone()),
                        enabled: Set(params.enabled),
                        created_at: Set(now),
                        created_by: Set(call.principal().login.clone()),
                        updated_at: Set(now),
                    }
                    .insert(tx)
                    .await?
                }
            };
            let subscription = subscription_of(row);
            call.emit_about(
                "webhook.subscription.saved",
                EventSubject {
                    kind: "webhook.subscription".to_owned(),
                    id: subscription.id.parse().map_err(|_| {
                        KeelError::internal("webhook.error.id")
                            .param("id", subscription.id.as_str())
                    })?,
                    account_id: subscription.account_id.parse().ok(),
                },
                &json!({ "id": subscription.id, "name": subscription.name }),
            )?;
            Ok(SubscriptionSaved {
                subscription,
                secret: answered,
            })
        })
}

pub fn subscription_toggle() -> FunctionDef {
    function::<SubscriptionToggle, Subscription>(SUBSCRIPTION_TOGGLE)
        .kind(Kind::Action)
        .access(Level::User, PERMISSION_MANAGE)
        .owned::<subscription::Entity>()
        .handler(|call: Call<SubscriptionToggle>| async move {
            let now = call.require::<dyn Clock>()?.now();
            let tx = call.tx()?;
            let row = load_subscription(tx, call.params().id).await?;
            subscription::Entity::update_many()
                .filter(subscription::Column::Id.eq(row.id.clone()))
                .col_expr(
                    subscription::Column::Enabled,
                    Expr::value(call.params().enabled),
                )
                .col_expr(subscription::Column::UpdatedAt, Expr::value(now))
                .exec(tx)
                .await?;
            let subscription = subscription_of(load_subscription(tx, call.params().id).await?);
            call.emit_about(
                "webhook.subscription.saved",
                EventSubject {
                    kind: "webhook.subscription".to_owned(),
                    id: call.params().id,
                    account_id: subscription.account_id.parse().ok(),
                },
                &json!({ "id": subscription.id, "enabled": subscription.enabled }),
            )?;
            Ok(subscription)
        })
}

pub fn subscription_delete() -> FunctionDef {
    function::<Pick, SubscriptionDeleted>(SUBSCRIPTION_DELETE)
        .kind(Kind::Action)
        .access(Level::User, PERMISSION_MANAGE)
        .owned::<subscription::Entity>()
        .handler(|call: Call<Pick>| async move {
            let tx = call.tx()?;
            let row = load_subscription(tx, call.params().id).await?;
            subscription::Entity::delete_by_id(row.id.clone())
                .exec(tx)
                .await?;
            call.emit_about(
                "webhook.subscription.deleted",
                EventSubject {
                    kind: "webhook.subscription".to_owned(),
                    id: call.params().id,
                    account_id: row.account_id.parse().ok(),
                },
                &json!({ "id": row.id }),
            )?;
            Ok(SubscriptionDeleted { deleted: true })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_secret_is_thirty_two_bytes_as_base64url_and_a_short_chosen_one_is_refused() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(b"foob"), "Zm9vYg");
        assert_eq!(base64url(&[0xfb, 0xff, 0xbf]), "-_-_");
        let generated = generate_secret().expect("entropy");
        assert_eq!(generated.len(), 43);
        assert!(
            generated.bytes().all(|byte| BASE64URL.contains(&byte)),
            "{generated}"
        );
        assert_ne!(generate_secret().expect("entropy"), generated);
        let short = Secret::new("fifteen-chars-x");
        let error = chosen_secret(Some(&short)).expect_err("fifteen bytes are short");
        assert_eq!(error.key(), ERROR_SECRET_SHORT);
        assert_eq!(error.field_path(), Some("secret"));
        let sixteen = Secret::new("sixteen-chars-xx");
        assert_eq!(
            chosen_secret(Some(&sixteen)).expect("sixteen bytes pass"),
            Some("sixteen-chars-xx")
        );
        assert_eq!(chosen_secret(Some(&Secret::new(""))).expect("empty"), None);
        assert_eq!(chosen_secret(None).expect("absent"), None);
        let error =
            check_masks(&["note.*".to_owned(), "nope".to_owned()]).expect_err("nope is no mask");
        assert_eq!(error.key(), ERROR_MASK);
        assert_eq!(error.field_path(), Some("events[1]"));
        assert!(check_masks(&["note.*".to_owned(), "dns.zone.created".to_owned()]).is_ok());
    }
}
