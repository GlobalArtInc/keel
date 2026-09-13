use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ActiveModelTrait, EntityTrait, Set};

use super::super::entity::ip_rule;
use super::{IpRules, RuleAction};

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct IpRuleCreate {
    pub action: RuleAction,
    #[schemars(extend("x-keel" = json!({ "check": "cidr" })))]
    pub cidr: String,
    pub level: Level,
    #[schemars(length(max = 200))]
    pub comment: Option<String>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Pick {
    pub id: Id,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct IpRuleDeleted {
    pub deleted: bool,
}

struct IpRuleSource;

#[async_trait]
impl ListSource for IpRuleSource {
    type Row = ip_rule::Model;

    async fn fetch(&self, call: &Call<ListQuery>, spec: &ListSpec) -> Result<Page<ip_rule::Model>> {
        fetch_page(ip_rule::Entity::find(), call, spec).await
    }
}

pub fn list() -> FunctionDef {
    function::<ListQuery, Page<ip_rule::Model>>("auth.ip_rule.list")
        .kind(Kind::List)
        .level(Level::Admin)
        .unowned()
        .ui(List::new("auth.ip_rule.list.title")
            .search(["cidr", "comment"])
            .column(
                Column::new("action", ColumnType::Status)
                    .map([("allow", Tone::Success), ("deny", Tone::Danger)]),
            )
            .column(Column::new("cidr", ColumnType::Text).sortable().primary())
            .column(
                Column::new("level", ColumnType::Text)
                    .sortable()
                    .filter_options(["guest", "service", "user", "reseller", "admin", "owner"]),
            )
            .column(Column::new("comment", ColumnType::Text))
            .column(Column::new("createdAt", ColumnType::Datetime).sortable())
            .action(
                Action::new("create", ActionRole::New)
                    .screen("auth.ip_rule.create")
                    .icon("plus"),
            )
            .action(
                Action::new("delete", ActionRole::Button)
                    .function("auth.ip_rule.delete")
                    .params([("id", "$id")])
                    .confirm(),
            )
            .row_actions(["delete"])
            .empty("auth.ip_rule.empty", Some("create"))
            .invalidate_on(["auth.ip_rule.*"]))
        .default_sort("createdAt", SortDir::Desc)
        .source(IpRuleSource)
}

pub fn create(rules: Arc<IpRules>) -> FunctionDef {
    function::<IpRuleCreate, ip_rule::Model>("auth.ip_rule.create")
        .kind(Kind::Form)
        .level(Level::Admin)
        .unowned()
        .ui(Form::new("auth.ip_rule.create.title")
            .page("main", ["action", "cidr", "level", "comment"]))
        .handler(move |call: Call<IpRuleCreate>| {
            let rules = rules.clone();
            async move {
                let params = call.params();
                let now = call.require::<dyn Clock>()?.now();
                let row = ip_rule::ActiveModel {
                    id: Set(Id::generate().to_string()),
                    action: Set(params.action.as_str().to_owned()),
                    cidr: Set(params.cidr.clone()),
                    level: Set(params.level.as_str().to_owned()),
                    comment: Set(params.comment.clone()),
                    created_at: Set(now),
                }
                .insert(call.tx()?)
                .await?;
                call.after_commit(move |_| async move {
                    rules.invalidate();
                    Ok(())
                });
                Ok(row)
            }
        })
}

pub fn delete(rules: Arc<IpRules>) -> FunctionDef {
    function::<Pick, IpRuleDeleted>("auth.ip_rule.delete")
        .kind(Kind::Action)
        .level(Level::Admin)
        .unowned()
        .handler(move |call: Call<Pick>| {
            let rules = rules.clone();
            async move {
                let deleted = ip_rule::Entity::delete_by_id(call.params().id.to_string())
                    .exec(call.tx()?)
                    .await?;
                if deleted.rows_affected == 0 {
                    return Err(KeelError::not_found("keel.error.not_found"));
                }
                call.after_commit(move |_| async move {
                    rules.invalidate();
                    Ok(())
                });
                Ok(IpRuleDeleted { deleted: true })
            }
        })
}
