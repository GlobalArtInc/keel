use keel::prelude::*;
use keel::sdk::sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, QueryFilter, Set,
    Statement,
};
use keel::sdk::serde_json;
use keel_hosting_users::api::{HostingUser, HostingUsers};

use crate::api::{DnsProvider, SiteZone};
use crate::host::{Effects, Gone, Turn};
use crate::names::{self, ALIASES_MAX, PHP_PATTERN, ROOT_MAX, ROOT_PATTERN};
use crate::{FEATURE, site};

const READ: &str = "hosting.site.read";
const MANAGE: &str = "hosting.site.manage";
const LIST: &str = "hosting.site.list";
const GET: &str = "hosting.site.get";
const CREATE: &str = "hosting.site.create";
const EDIT: &str = "hosting.site.edit";
const DELETE: &str = "hosting.site.delete";
const PHP: &str = "hosting.site.php";
const EXISTS: &str = "hosting.site.error.exists";
const USER: &str = "hosting.site.error.user";
const USER_PENDING: &str = "hosting.site.error.user_pending";
const VERSIONS: [&str; 3] = ["8.2", "8.3", "8.4"];
const SERIALISE: &str = "SELECT pg_advisory_xact_lock(hashtextextended('hosting_site:names', 0))";
const TAKEN: &str = "SELECT n FROM jsonb_array_elements_text($2::jsonb) AS n WHERE EXISTS (SELECT 1 FROM hosting_site s WHERE s.id <> $1 AND (s.domain = n OR s.aliases @> to_jsonb(n))) LIMIT 1";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Pick {
    id: Id,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel::sdk::schemars", rename_all = "camelCase")]
struct SiteCreate {
    #[schemars(extend("x-keel" = json!({ "options": "hosting.user.options" })))]
    user: Id,
    #[schemars(length(min = 3, max = 253), extend("x-keel" = json!({ "check": "domain" })))]
    domain: String,
    #[serde(default)]
    #[schemars(length(max = ALIASES_MAX), extend("x-keel" = json!({ "check": "domain" }), "x-ui" = json!({ "control": "tags" })))]
    aliases: Vec<String>,
    #[serde(default)]
    #[schemars(length(min = 1, max = ROOT_MAX), pattern(ROOT_PATTERN))]
    root: Option<String>,
    #[serde(default)]
    #[schemars(pattern(PHP_PATTERN), extend("x-keel" = json!({ "options": PHP })))]
    php: Option<String>,
    #[serde(default)]
    dns_zone: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct SiteEdit {
    id: Id,
    #[serde(default)]
    #[schemars(length(max = ALIASES_MAX), extend("x-keel" = json!({ "check": "domain" }), "x-ui" = json!({ "control": "tags" })))]
    aliases: Vec<String>,
    #[schemars(length(min = 1, max = ROOT_MAX), pattern(ROOT_PATTERN))]
    root: String,
    #[serde(default)]
    #[schemars(pattern(PHP_PATTERN), extend("x-keel" = json!({ "options": PHP })))]
    php: Option<String>,
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
    function::<ListQuery, Page<site::Model>>(LIST)
        .kind(Kind::List)
        .access(Level::User, READ)
        .feature(FEATURE)
        .owned::<site::Entity>()
        .ui(List::new("hosting.site.list.title")
            .search(["domain"])
            .column(
                Column::new("domain", ColumnType::Link)
                    .to(EDIT, [("id", "$id")])
                    .sortable()
                    .primary(),
            )
            .column(
                Column::new("state", ColumnType::Status)
                    .map([
                        (site::STATE_PENDING, Tone::Neutral),
                        (site::STATE_ACTIVE, Tone::Success),
                        (site::STATE_ERROR, Tone::Danger),
                    ])
                    .filter_options([site::STATE_PENDING, site::STATE_ACTIVE, site::STATE_ERROR]),
            )
            .column(Column::new("suspended", ColumnType::Boolean))
            .column(Column::new("php", ColumnType::Text))
            .column(Column::new("root", ColumnType::Text))
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
                        items: Some("domain".to_owned()),
                        input: Some(ConfirmInput::Count),
                    }),
            )
            .row_actions(["edit", "delete"])
            .empty("hosting.site.empty", Some("create"))
            .invalidate_on(["hosting.site.*"]))
        .default_sort("domain", SortDir::Asc)
        .source(entity_source::<site::Entity>())
}

pub(crate) fn get() -> FunctionDef {
    function::<Pick, site::Model>(GET)
        .kind(Kind::Get)
        .access(Level::User, READ)
        .feature(FEATURE)
        .owned::<site::Entity>()
        .handler(|call: Call<Pick>| async move { load(call.db(), &call.params().id).await })
}

pub(crate) fn create(turn: Turn) -> FunctionDef {
    function::<SiteCreate, site::Model>(CREATE)
        .kind(Kind::Wizard)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .owned::<site::Entity>()
        .ui(Form::new("hosting.site.create.title")
            .page("site", ["user", "domain", "aliases", "root"])
            .page("php", ["php"])
            .page("dns", ["dnsZone"]))
        .handler(move |call: Call<SiteCreate>| {
            let turn = Arc::clone(&turn);
            async move {
                let params = call.params();
                let tx = call.tx()?;
                let owner = owner_of(&call, &params.user).await?;
                let account = call.resolve_account(Some(owner.account)).await?;
                let domain = names::host_name(&params.domain, "domain")?;
                let aliases = names::aliases(&domain, &params.aliases)?;
                let root = names::root(params.root.as_deref().unwrap_or(&domain))?;
                let php = names::php(params.php.as_deref())?;
                refuse_taken(tx, None, &domain, &aliases).await?;
                let now = call.require::<dyn Clock>()?.now();
                let row = site::ActiveModel {
                    id: Set(Id::generate().to_string()),
                    account_id: Set(account.to_string()),
                    user_id: Set(owner.id.to_string()),
                    domain: Set(domain.clone()),
                    aliases: Set(site::Aliases(aliases)),
                    root: Set(root),
                    php: Set(php),
                    php_applied: Set(None),
                    suspended: Set(false),
                    state: Set(site::STATE_PENDING.to_owned()),
                    error: Set(None),
                    created_at: Set(now),
                    updated_at: Set(now),
                }
                .insert(tx)
                .await?;
                if params.dns_zone {
                    let dns = call.require::<dyn DnsProvider>()?;
                    let invoke = |func: &str, params: Value| call.invoke(func, params);
                    dns.add_zone(
                        &invoke,
                        &SiteZone {
                            name: domain,
                            account,
                            own_account: account == call.principal().account_id,
                        },
                    )
                    .await?;
                }
                apply_after_commit(&call, &turn, vec![row.id.clone()])?;
                Ok(row)
            }
        })
}

pub(crate) fn edit(turn: Turn) -> FunctionDef {
    function::<SiteEdit, site::Model>(EDIT)
        .kind(Kind::Form)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .owned::<site::Entity>()
        .ui(Form::new("hosting.site.edit.title")
            .read(GET)
            .page("main", ["aliases", "root", "php"]))
        .handler(move |call: Call<SiteEdit>| {
            let turn = Arc::clone(&turn);
            async move {
                let params = call.params();
                let tx = call.tx()?;
                let current = load(tx, &params.id).await?;
                let aliases = names::aliases(&current.domain, &params.aliases)?;
                let root = names::root(&params.root)?;
                let php = names::php(params.php.as_deref())?;
                refuse_taken(tx, Some(&current.id), &current.domain, &aliases).await?;
                let now = call.require::<dyn Clock>()?.now();
                let row = site::ActiveModel {
                    id: Set(current.id.clone()),
                    aliases: Set(site::Aliases(aliases)),
                    root: Set(root),
                    php: Set(php),
                    state: Set(site::STATE_PENDING.to_owned()),
                    error: Set(None),
                    updated_at: Set(now),
                    ..Default::default()
                }
                .update(tx)
                .await?;
                apply_after_commit(&call, &turn, vec![row.id.clone()])?;
                Ok(row)
            }
        })
}

pub(crate) fn delete(turn: Turn) -> FunctionDef {
    function::<Batch, Deleted>(DELETE)
        .kind(Kind::Action)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .key("ids")
        .owned::<site::Entity>()
        .handler(move |call: Call<Batch>| {
            let turn = Arc::clone(&turn);
            async move {
                let ids: Vec<String> = call.params().ids.iter().map(ToString::to_string).collect();
                let gone = drop_rows(call.tx()?, site::Column::Id.is_in(ids)).await?;
                let deleted = u64::try_from(gone.len()).unwrap_or(u64::MAX);
                remove_after_commit(&call, &turn, gone)?;
                Ok(Deleted { deleted })
            }
        })
}

pub(crate) fn php() -> FunctionDef {
    function::<OptionsQuery, Choices>(PHP)
        .kind(Kind::Options)
        .access(Level::User, READ)
        .feature(FEATURE)
        .unowned()
        .handler(|call: Call<OptionsQuery>| async move {
            let search = call.params().search.to_ascii_lowercase();
            Ok(Choices {
                data: VERSIONS
                    .iter()
                    .filter(|version| version.contains(search.as_str()))
                    .map(|version| Choice::new(*version, format!("PHP {version}")))
                    .collect(),
                more: false,
            })
        })
}

pub(crate) async fn drop_rows<C: ConnectionTrait>(
    db: &C,
    filter: impl keel::sdk::sea_orm::sea_query::IntoCondition,
) -> Result<Vec<Gone>> {
    let condition = filter.into_condition();
    let rows = site::Entity::find()
        .filter(condition.clone())
        .all(db)
        .await?;
    site::Entity::delete_many()
        .filter(condition)
        .exec(db)
        .await?;
    Ok(rows.into_iter().map(Gone::from).collect())
}

pub(crate) fn remove_after_commit<P>(call: &Call<P>, turn: &Turn, gone: Vec<Gone>) -> Result<()> {
    if gone.is_empty() {
        return Ok(());
    }
    let effects = Effects::of(call, turn)?;
    call.after_commit(move |ctx| async move { effects.remove(&ctx, &gone).await });
    Ok(())
}

fn apply_after_commit<P>(call: &Call<P>, turn: &Turn, ids: Vec<String>) -> Result<()> {
    let effects = Effects::of(call, turn)?;
    call.after_commit(move |ctx| async move { effects.apply(&ctx, &ids).await });
    Ok(())
}

async fn owner_of<P>(call: &Call<P>, user: &Id) -> Result<HostingUser> {
    let tx = call.tx()?;
    let missing = || KeelError::not_found(USER).field("user");
    let owner = call
        .require::<dyn HostingUsers>()?
        .get(tx, user)
        .await?
        .ok_or_else(missing)?;
    if !call.account_scope().includes(tx, owner.account).await? {
        return Err(missing());
    }
    if owner.home.is_none() {
        return Err(KeelError::conflict(USER_PENDING)
            .field("user")
            .param("user", owner.name.as_str()));
    }
    Ok(owner)
}

async fn refuse_taken<C: ConnectionTrait>(
    db: &C,
    except: Option<&str>,
    domain: &str,
    aliases: &[String],
) -> Result<()> {
    db.execute(Statement::from_string(DbBackend::Postgres, SERIALISE))
        .await?;
    let mut names = vec![domain.to_owned()];
    names.extend(aliases.iter().cloned());
    let taken = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            TAKEN,
            [
                except.unwrap_or_default().into(),
                serde_json::to_string(&names)?.into(),
            ],
        ))
        .await?;
    let Some(taken) = taken else {
        return Ok(());
    };
    let name: String = taken.try_get("", "n")?;
    let field = if name == domain { "domain" } else { "aliases" };
    Err(KeelError::conflict(EXISTS).field(field).param("name", name))
}

async fn load<C: ConnectionTrait>(db: &C, id: &Id) -> Result<site::Model> {
    site::Entity::find_by_id(id.to_string())
        .one(db)
        .await?
        .ok_or_else(|| KeelError::not_found("keel.error.not_found"))
}
