use keel::prelude::*;
use keel::sdk::escape_like;
use keel::sdk::sea_orm::sea_query::Expr;
use keel::sdk::sea_orm::sea_query::extension::postgres::PgExpr;
use keel::sdk::sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};

use crate::host::Effects;
use crate::{FEATURE, USER_PREFIX, user};

const READ: &str = "hosting.user.read";
const MANAGE: &str = "hosting.user.manage";
const LIST: &str = "hosting.user.list";
const GET: &str = "hosting.user.get";
const CREATE: &str = "hosting.user.create";
const EDIT: &str = "hosting.user.edit";
const DELETE: &str = "hosting.user.delete";
const OPTIONS: &str = "hosting.user.options";
const MAX_OPTIONS: u32 = 100;
const EXISTS: &str = "hosting.user.error.exists";
const NAME: &str = r"^[a-z][a-z0-9_-]{0,28}$";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Pick {
    id: Id,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct UserCreate {
    #[schemars(length(min = 1, max = 29), pattern(NAME))]
    name: String,
    #[serde(default)]
    shell: bool,
    #[schemars(extend("x-keel" = json!({ "level": "reseller", "options": "keel.account.options" })))]
    account: Option<Id>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct UserEdit {
    id: Id,
    shell: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Batch {
    #[schemars(length(min = 1))]
    ids: Vec<Id>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Deleted {
    deleted: u64,
}

pub(crate) fn list() -> FunctionDef {
    function::<ListQuery, Page<user::Model>>(LIST)
        .kind(Kind::List)
        .access(Level::User, READ)
        .feature(FEATURE)
        .owned::<user::Entity>()
        .ui(List::new("hosting.user.list.title")
            .search(["name"])
            .column(
                Column::new("name", ColumnType::Link)
                    .to(EDIT, [("id", "$id")])
                    .sortable()
                    .primary(),
            )
            .column(
                Column::new("state", ColumnType::Status)
                    .map([
                        (user::STATE_PENDING, Tone::Neutral),
                        (user::STATE_ACTIVE, Tone::Success),
                        (user::STATE_SUSPENDED, Tone::Warning),
                        (user::STATE_ERROR, Tone::Danger),
                    ])
                    .filter_options([
                        user::STATE_PENDING,
                        user::STATE_ACTIVE,
                        user::STATE_SUSPENDED,
                        user::STATE_ERROR,
                    ]),
            )
            .column(Column::new("uid", ColumnType::Number))
            .column(Column::new("home", ColumnType::Text))
            .column(Column::new("shell", ColumnType::Boolean))
            .column(Column::new("error", ColumnType::Text))
            .column(
                Column::new("accountId", ColumnType::Text)
                    .level(Level::Reseller)
                    .filter_remote("keel.account.options"),
            )
            .action(
                Action::new("create", ActionRole::New)
                    .screen(CREATE)
                    .icon("plus"),
            )
            .action(Action::new("edit", ActionRole::Edit).screen(EDIT).default())
            .action(
                Action::new("delete", ActionRole::Delete)
                    .function(DELETE)
                    .confirm_with(ConfirmDetails {
                        destructive: true,
                        items: Some("name".to_owned()),
                        input: Some(ConfirmInput::Count),
                    }),
            )
            .row_actions(["edit", "delete"])
            .empty("hosting.user.empty", Some("create"))
            .invalidate_on(["hosting.user.*"]))
        .default_sort("name", SortDir::Asc)
        .source(entity_source::<user::Entity>())
}

pub(crate) fn get() -> FunctionDef {
    function::<Pick, user::Model>(GET)
        .kind(Kind::Get)
        .access(Level::User, READ)
        .feature(FEATURE)
        .owned::<user::Entity>()
        .handler(|call: Call<Pick>| async move { load(call.db(), &call.params().id).await })
}

pub(crate) fn create() -> FunctionDef {
    function::<UserCreate, user::Model>(CREATE)
        .kind(Kind::Form)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .owned::<user::Entity>()
        .ui(Form::new("hosting.user.create.title")
            .page("main", ["name", "shell"])
            .page("owner", ["account"])
            .show_if("$principal.level != 'user'"))
        .handler(|call: Call<UserCreate>| async move {
            let params = call.params();
            let tx = call.tx()?;
            let account = call.resolve_account(params.account).await?;
            let name = format!("{USER_PREFIX}{}", params.name);
            refuse_taken(tx, &name).await?;
            let now = call.require::<dyn Clock>()?.now();
            let row = user::ActiveModel {
                id: Set(Id::generate().to_string()),
                account_id: Set(account.to_string()),
                name: Set(name),
                uid: Set(None),
                home: Set(None),
                shell: Set(params.shell),
                state: Set(user::STATE_PENDING.to_owned()),
                error: Set(None),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(tx)
            .await?;
            let effects = Effects::of(&call)?;
            let id = row.id.clone();
            call.after_commit(move |ctx| async move { effects.create(&ctx, &id).await });
            Ok(row)
        })
}

pub(crate) fn edit() -> FunctionDef {
    function::<UserEdit, user::Model>(EDIT)
        .kind(Kind::Form)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .owned::<user::Entity>()
        .ui(Form::new("hosting.user.edit.title")
            .read(GET)
            .page("main", ["shell"]))
        .handler(|call: Call<UserEdit>| async move {
            let params = call.params();
            let tx = call.tx()?;
            let current = load(tx, &params.id).await?;
            let now = call.require::<dyn Clock>()?.now();
            let row = user::ActiveModel {
                id: Set(current.id.clone()),
                shell: Set(params.shell),
                updated_at: Set(now),
                ..Default::default()
            }
            .update(tx)
            .await?;
            if current.shell != row.shell && current.uid.is_some() {
                let effects = Effects::of(&call)?;
                let name = row.name.clone();
                let shell = row.shell;
                call.after_commit(
                    move |ctx| async move { effects.shell(&ctx, &name, shell).await },
                );
            }
            Ok(row)
        })
}

pub(crate) fn delete() -> FunctionDef {
    function::<Batch, Deleted>(DELETE)
        .kind(Kind::Action)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .key("ids")
        .owned::<user::Entity>()
        .handler(|call: Call<Batch>| async move {
            let ids: Vec<String> = call.params().ids.iter().map(ToString::to_string).collect();
            let tx = call.tx()?;
            let names = drop_rows(tx, user::Column::Id.is_in(ids)).await?;
            let deleted = u64::try_from(names.len()).unwrap_or(u64::MAX);
            remove_after_commit(&call, names)?;
            Ok(Deleted { deleted })
        })
}

pub(crate) fn options() -> FunctionDef {
    function::<OptionsQuery, Choices>(OPTIONS)
        .kind(Kind::Options)
        .access(Level::User, READ)
        .feature(FEATURE)
        .unowned()
        .handler(|call: Call<OptionsQuery>| async move {
            let query = call.params();
            let limit = query.limit.clamp(1, MAX_OPTIONS);
            let mut select = call
                .account_scope()
                .apply(user::Entity::find())
                .filter(user::Column::State.eq(user::STATE_ACTIVE));
            if !query.search.is_empty() {
                let pattern = format!("%{}%", escape_like(&query.search));
                select = select.filter(Expr::col(user::Column::Name).ilike(pattern.as_str()));
            }
            if let Some(cursor) = &query.cursor {
                select = select.filter(user::Column::Id.gt(cursor.as_str()));
            }
            let mut rows = select
                .order_by_asc(user::Column::Id)
                .limit(u64::from(limit) + 1)
                .all(call.db())
                .await?;
            let more = rows.len() > limit as usize;
            rows.truncate(limit as usize);
            Ok(Choices {
                data: rows
                    .into_iter()
                    .map(|row| Choice::new(row.id, row.name))
                    .collect(),
                more,
            })
        })
}

pub(crate) async fn drop_rows<C: ConnectionTrait>(
    db: &C,
    filter: impl keel::sdk::sea_orm::sea_query::IntoCondition,
) -> Result<Vec<String>> {
    let condition = filter.into_condition();
    let rows = user::Entity::find()
        .filter(condition.clone())
        .all(db)
        .await?;
    user::Entity::delete_many()
        .filter(condition)
        .exec(db)
        .await?;
    Ok(rows.into_iter().map(|row| row.name).collect())
}

pub(crate) fn remove_after_commit<P>(call: &Call<P>, names: Vec<String>) -> Result<()> {
    if names.is_empty() {
        return Ok(());
    }
    let effects = Effects::of(call)?;
    call.after_commit(move |ctx| async move { effects.remove(&ctx, &names).await });
    Ok(())
}

async fn refuse_taken<C: ConnectionTrait>(db: &C, name: &str) -> Result<()> {
    let taken = user::Entity::find()
        .filter(user::Column::Name.eq(name))
        .one(db)
        .await?;
    match taken {
        Some(_) => Err(KeelError::conflict(EXISTS)
            .field("name")
            .param("name", name)),
        None => Ok(()),
    }
}

async fn load<C: ConnectionTrait>(db: &C, id: &Id) -> Result<user::Model> {
    user::Entity::find_by_id(id.to_string())
        .one(db)
        .await?
        .ok_or_else(|| KeelError::not_found("keel.error.not_found"))
}
