use keel::prelude::*;
use keel::sdk::sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, IntoActiveModel,
    PaginatorTrait, QueryFilter, QueryOrder, Set, Statement,
};
use keel::sdk::serde_json;
use keel::types::ErrorCode;

use crate::host::{Changed, Ftp};
use crate::names::{self, DIRECTORY, DIRECTORY_MAX, NAME, NAME_MAX, PASSWORD_MAX, PASSWORD_MIN};
use crate::{FEATURE, LIMIT, MANAGE, READ, STATE_ACTIVE, STATE_ERROR, STATE_PENDING, account};

const LIST: &str = "hosting.ftp.list";
const GET: &str = "hosting.ftp.get";
const CREATE: &str = "hosting.ftp.create";
const EDIT: &str = "hosting.ftp.edit";
const DELETE: &str = "hosting.ftp.delete";
const SITES: &str = "hosting.ftp.sites";
const SITE_GET: &str = "hosting.site.get";
const SITE_LIST: &str = "hosting.site.list";
const EXISTS: &str = "hosting.ftp.error.exists";
const SITE: &str = "hosting.ftp.error.site";
const SITE_ROW: &str = "hosting.ftp.error.site_row";
const MISSING: &str = "hosting.ftp.error.id";
const CHOICES: u32 = 20;
const SERIALISE_ACCOUNT: &str =
    "SELECT pg_advisory_xact_lock(hashtextextended('hosting_ftp:account:' || $1, 0))";
const SERIALISE_LOGIN: &str =
    "SELECT pg_advisory_xact_lock(hashtextextended('hosting_ftp:login:' || $1, 0))";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Pick {
    id: Id,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct FtpCreate {
    #[schemars(extend("x-keel" = json!({ "options": SITES })))]
    site: Id,
    #[schemars(length(min = 1, max = NAME_MAX), pattern(NAME))]
    name: String,
    #[schemars(extend("x-keel" = json!({ "secret": true }), "x-ui" = json!({ "control": "password" }), "minLength" = PASSWORD_MIN, "maxLength" = PASSWORD_MAX))]
    password: Secret,
    #[serde(default)]
    #[schemars(length(max = DIRECTORY_MAX), pattern(DIRECTORY))]
    directory: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct FtpEdit {
    id: Id,
    #[serde(default)]
    #[schemars(extend("x-keel" = json!({ "secret": true }), "x-ui" = json!({ "control": "password" }), "minLength" = PASSWORD_MIN, "maxLength" = PASSWORD_MAX))]
    password: Option<Secret>,
    #[serde(default)]
    #[schemars(length(max = DIRECTORY_MAX), pattern(DIRECTORY))]
    directory: String,
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

#[derive(Deserialize)]
#[serde(crate = "keel::sdk::serde", rename_all = "camelCase")]
struct SiteView {
    id: String,
    account_id: String,
    domain: String,
}

pub(crate) fn list() -> FunctionDef {
    function::<ListQuery, Page<account::Model>>(LIST)
        .kind(Kind::List)
        .access(Level::User, READ)
        .feature(FEATURE)
        .owned::<account::Entity>()
        .ui(List::new("hosting.ftp.list.title")
            .search(["login", "domain"])
            .column(
                Column::new("login", ColumnType::Link)
                    .to(EDIT, [("id", "$id")])
                    .sortable()
                    .primary(),
            )
            .column(Column::new("domain", ColumnType::Text).sortable())
            .column(Column::new("directory", ColumnType::Text))
            .column(Column::new("suspended", ColumnType::Boolean))
            .column(
                Column::new("state", ColumnType::Status)
                    .map([
                        (STATE_PENDING, Tone::Neutral),
                        (STATE_ACTIVE, Tone::Success),
                        (STATE_ERROR, Tone::Danger),
                    ])
                    .filter_options([STATE_PENDING, STATE_ACTIVE, STATE_ERROR]),
            )
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
                        items: Some("login".to_owned()),
                        input: Some(ConfirmInput::Count),
                    }),
            )
            .row_actions(["edit", "delete"])
            .empty("hosting.ftp.empty", Some("create"))
            .invalidate_on(["hosting.ftp.*"]))
        .default_sort("login", SortDir::Asc)
        .source(entity_source::<account::Entity>())
}

pub(crate) fn get() -> FunctionDef {
    function::<Pick, account::Model>(GET)
        .kind(Kind::Get)
        .access(Level::User, READ)
        .feature(FEATURE)
        .owned::<account::Entity>()
        .handler(|call: Call<Pick>| async move { load(call.db(), &call.params().id).await })
}

pub(crate) fn create(ftp: Arc<Ftp>) -> FunctionDef {
    function::<FtpCreate, account::Model>(CREATE)
        .kind(Kind::Form)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .owned::<account::Entity>()
        .ui(Form::new("hosting.ftp.create.title")
            .page("main", ["site", "name", "password", "directory"]))
        .handler(move |call: Call<FtpCreate>| {
            let ftp = Arc::clone(&ftp);
            async move {
                let params = call.params();
                let login = names::login(&params.name)?;
                let directory = names::directory(&params.directory)?.to_owned();
                names::check_password(&params.password)?;
                let site = site_of(&call, &params.site).await?;
                let owner: Id = site
                    .account_id
                    .parse()
                    .map_err(|_| KeelError::internal(SITE_ROW).param("site", site.id.as_str()))?;
                let owner = call.resolve_account(Some(owner)).await?.to_string();
                let tx = call.tx()?;
                serialise(tx, SERIALISE_ACCOUNT, &owner).await?;
                let kept = account::Entity::find()
                    .filter(account::Column::AccountId.eq(owner.as_str()))
                    .count(tx)
                    .await?;
                call.check_limit(LIMIT, kept).await?;
                refuse_taken(tx, &login).await?;
                let password_hash = names::hash(&params.password).await?;
                let now = call.require::<dyn Clock>()?.now();
                let row = account::ActiveModel {
                    id: Set(Id::generate().to_string()),
                    account_id: Set(owner),
                    site_id: Set(site.id),
                    domain: Set(site.domain),
                    login: Set(login),
                    password_hash: Set(password_hash),
                    directory: Set(directory),
                    suspended: Set(false),
                    state: Set(STATE_PENDING.to_owned()),
                    error: Set(None),
                    created_at: Set(now),
                    updated_at: Set(now),
                }
                .insert(tx)
                .await?;
                apply_after_commit(
                    &call,
                    ftp,
                    Changed {
                        accounts: vec![row.id.clone()],
                        ..Changed::default()
                    },
                );
                Ok(row)
            }
        })
}

pub(crate) fn edit(ftp: Arc<Ftp>) -> FunctionDef {
    function::<FtpEdit, account::Model>(EDIT)
        .kind(Kind::Form)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .owned::<account::Entity>()
        .ui(Form::new("hosting.ftp.edit.title")
            .read(GET)
            .page("main", ["password", "directory"]))
        .handler(move |call: Call<FtpEdit>| {
            let ftp = Arc::clone(&ftp);
            async move {
                let params = call.params();
                let directory = names::directory(&params.directory)?.to_owned();
                let tx = call.tx()?;
                let current = load(tx, &params.id).await?;
                let now = call.require::<dyn Clock>()?.now();
                let mut row = current.into_active_model();
                if let Some(password) = &params.password {
                    names::check_password(password)?;
                    row.password_hash = Set(names::hash(password).await?);
                }
                row.directory = Set(directory);
                row.state = Set(STATE_PENDING.to_owned());
                row.updated_at = Set(now);
                let row = row.update(tx).await?;
                apply_after_commit(
                    &call,
                    ftp,
                    Changed {
                        accounts: vec![row.id.clone()],
                        ..Changed::default()
                    },
                );
                Ok(row)
            }
        })
}

pub(crate) fn delete(ftp: Arc<Ftp>) -> FunctionDef {
    function::<Batch, Deleted>(DELETE)
        .kind(Kind::Action)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .key("ids")
        .owned::<account::Entity>()
        .handler(move |call: Call<Batch>| {
            let ftp = Arc::clone(&ftp);
            async move {
                let ids: Vec<String> = call.params().ids.iter().map(ToString::to_string).collect();
                let gone = drop_rows(call.tx()?, account::Column::Id.is_in(ids)).await?;
                let deleted = u64::try_from(gone.len()).unwrap_or(u64::MAX);
                apply_after_commit(
                    &call,
                    ftp,
                    Changed {
                        gone,
                        ..Changed::default()
                    },
                );
                Ok(Deleted { deleted })
            }
        })
}

pub(crate) fn sites() -> FunctionDef {
    function::<OptionsQuery, Choices>(SITES)
        .kind(Kind::Options)
        .access(Level::User, READ)
        .feature(FEATURE)
        .unowned()
        .handler(|call: Call<OptionsQuery>| async move {
            let query = call.params();
            let limit = if query.limit == 0 {
                CHOICES
            } else {
                query.limit.min(CHOICES)
            };
            let page = call
                .invoke(
                    SITE_LIST,
                    json!({ "page": 1, "pageSize": limit, "search": query.search }),
                )
                .await?;
            let rows: Vec<SiteView> = serde_json::from_value(page["data"].clone())?;
            Ok(Choices {
                data: rows
                    .into_iter()
                    .map(|site| Choice::new(site.id, site.domain))
                    .collect(),
                more: false,
            })
        })
}

pub(crate) fn apply_after_commit<P>(call: &Call<P>, ftp: Arc<Ftp>, changed: Changed) {
    call.after_commit(move |ctx| async move { ftp.apply(&ctx, changed).await });
}

pub(crate) async fn drop_rows<C: ConnectionTrait>(
    db: &C,
    filter: impl keel::sdk::sea_orm::sea_query::IntoCondition,
) -> Result<Vec<String>> {
    let condition = filter.into_condition();
    let gone: Vec<String> = account::Entity::find()
        .filter(condition.clone())
        .order_by_asc(account::Column::Login)
        .all(db)
        .await?
        .into_iter()
        .map(|row| row.login)
        .collect();
    if !gone.is_empty() {
        account::Entity::delete_many()
            .filter(condition)
            .exec(db)
            .await?;
    }
    Ok(gone)
}

async fn site_of<P>(call: &Call<P>, site: &Id) -> Result<SiteView> {
    let value = call
        .invoke(SITE_GET, json!({ "id": site }))
        .await
        .map_err(|error| {
            if error.code() == ErrorCode::NotFound {
                KeelError::not_found(SITE).field("site")
            } else {
                error
            }
        })?;
    serde_json::from_value(value).map_err(|error| {
        KeelError::internal(SITE_ROW)
            .param("site", site.to_string())
            .param("detail", error.to_string())
    })
}

async fn serialise<C: ConnectionTrait>(db: &C, sql: &str, key: &str) -> Result<()> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        [key.into()],
    ))
    .await?;
    Ok(())
}

async fn refuse_taken<C: ConnectionTrait>(db: &C, login: &str) -> Result<()> {
    serialise(db, SERIALISE_LOGIN, login).await?;
    let taken = account::Entity::find()
        .filter(account::Column::Login.eq(login))
        .count(db)
        .await?;
    if taken == 0 {
        Ok(())
    } else {
        Err(KeelError::conflict(EXISTS)
            .field("name")
            .param("name", login))
    }
}

async fn load<C: ConnectionTrait>(db: &C, id: &Id) -> Result<account::Model> {
    account::Entity::find_by_id(id.to_string())
        .one(db)
        .await?
        .ok_or_else(|| KeelError::not_found(MISSING).param("id", id.to_string()))
}
