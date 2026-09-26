use keel::prelude::*;
use keel::sdk::sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, IntoActiveModel,
    QueryFilter, QueryOrder, Set, Statement,
};
use keel::sdk::serde_json;

use crate::domains::{Batch, Deleted, OPTIONS, apply_after_commit, state_column};
use crate::host::{Changed, Mail};
use crate::mailbox::Names;
use crate::names::{
    self, LOCAL_PATTERN, NAMES_MAX, PASSWORD_MAX, PASSWORD_MIN, QUOTA_DEFAULT, QUOTA_MAX,
};
use crate::{FEATURE, MANAGE, READ, STATE_PENDING, domain, mailbox};

const LIST: &str = "hosting.mail.mailbox.list";
const GET: &str = "hosting.mail.mailbox.get";
pub(crate) const CREATE: &str = "hosting.mail.mailbox.create";
const EDIT: &str = "hosting.mail.mailbox.edit";
const DELETE: &str = "hosting.mail.mailbox.delete";
const EXISTS: &str = "hosting.mail.mailbox.error.exists";
const DOMAIN: &str = "hosting.mail.mailbox.error.domain";
const MISSING: &str = "hosting.mail.mailbox.error.id";
const SERIALISE: &str = "SELECT pg_advisory_xact_lock(hashtextextended('hosting_mail:' || $1, 0))";
const TAKEN: &str = "SELECT n FROM jsonb_array_elements_text($3::jsonb) AS n WHERE EXISTS (SELECT 1 FROM hosting_mailbox m WHERE m.domain_id = $2 AND m.id <> $1 AND (m.local = n OR m.aliases @> to_jsonb(n))) LIMIT 1";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Pick {
    id: Id,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct MailboxCreate {
    #[schemars(extend("x-keel" = json!({ "options": OPTIONS })))]
    domain: Id,
    #[schemars(length(min = 1, max = 64), pattern(LOCAL_PATTERN))]
    local: String,
    #[schemars(extend("x-keel" = json!({ "secret": true }), "x-ui" = json!({ "control": "password" }), "minLength" = PASSWORD_MIN, "maxLength" = PASSWORD_MAX))]
    password: Secret,
    #[serde(default = "quota")]
    #[schemars(range(max = QUOTA_MAX))]
    quota: u32,
    #[serde(default)]
    #[schemars(length(max = NAMES_MAX), extend("x-ui" = json!({ "control": "tags" })))]
    aliases: Vec<String>,
    #[serde(default)]
    #[schemars(length(max = NAMES_MAX), extend("x-keel" = json!({ "check": "email" }), "x-ui" = json!({ "control": "tags" })))]
    forwards: Vec<String>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct MailboxEdit {
    id: Id,
    #[serde(default)]
    #[schemars(extend("x-keel" = json!({ "secret": true }), "x-ui" = json!({ "control": "password" }), "minLength" = PASSWORD_MIN, "maxLength" = PASSWORD_MAX))]
    password: Option<Secret>,
    #[schemars(range(max = QUOTA_MAX))]
    quota: u32,
    #[serde(default)]
    #[schemars(length(max = NAMES_MAX), extend("x-ui" = json!({ "control": "tags" })))]
    aliases: Vec<String>,
    #[serde(default)]
    #[schemars(length(max = NAMES_MAX), extend("x-keel" = json!({ "check": "email" }), "x-ui" = json!({ "control": "tags" })))]
    forwards: Vec<String>,
}

fn quota() -> u32 {
    QUOTA_DEFAULT
}

pub(crate) fn list() -> FunctionDef {
    function::<ListQuery, Page<mailbox::Model>>(LIST)
        .kind(Kind::List)
        .access(Level::User, READ)
        .feature(FEATURE)
        .owned::<mailbox::Entity>()
        .ui(List::new("hosting.mail.mailbox.list.title")
            .search(["address"])
            .column(
                Column::new("address", ColumnType::Link)
                    .to(EDIT, [("id", "$id")])
                    .sortable()
                    .primary(),
            )
            .column(Column::new("quota", ColumnType::Number).sortable())
            .column(Column::new("suspended", ColumnType::Boolean))
            .column(state_column())
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
                        items: Some("address".to_owned()),
                        input: Some(ConfirmInput::Count),
                    }),
            )
            .row_actions(["edit", "delete"])
            .empty("hosting.mail.mailbox.empty", Some("create"))
            .invalidate_on(["hosting.mail.*"]))
        .default_sort("address", SortDir::Asc)
        .source(entity_source::<mailbox::Entity>())
}

pub(crate) fn get() -> FunctionDef {
    function::<Pick, mailbox::Model>(GET)
        .kind(Kind::Get)
        .access(Level::User, READ)
        .feature(FEATURE)
        .owned::<mailbox::Entity>()
        .handler(|call: Call<Pick>| async move { load(call.db(), &call.params().id).await })
}

pub(crate) fn create(mail: Arc<Mail>) -> FunctionDef {
    function::<MailboxCreate, mailbox::Model>(CREATE)
        .kind(Kind::Form)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .owned::<mailbox::Entity>()
        .ui(Form::new("hosting.mail.mailbox.create.title")
            .page("main", ["domain", "local", "password", "quota"])
            .page("routing", ["aliases", "forwards"]))
        .handler(move |call: Call<MailboxCreate>| {
            let mail = Arc::clone(&mail);
            async move {
                let params = call.params();
                let tx = call.tx()?;
                let owner = domain_of(&call, &params.domain).await?;
                let account: Id = owner
                    .account_id
                    .parse()
                    .map_err(|_| KeelError::not_found(DOMAIN).field("domain"))?;
                let account = call.resolve_account(Some(account)).await?;
                let local = names::local(&params.local, "local")?;
                let address = format!("{local}@{}", owner.name);
                let aliases = names::aliases(&local, &params.aliases)?;
                let forwards = names::forwards(&address, &params.forwards)?;
                names::check_password(&params.password)?;
                refuse_taken(tx, None, &owner.id, &local, &aliases).await?;
                let password_hash = names::hash(&params.password).await?;
                let now = call.require::<dyn Clock>()?.now();
                let row = mailbox::ActiveModel {
                    id: Set(Id::generate().to_string()),
                    account_id: Set(account.to_string()),
                    domain_id: Set(owner.id.clone()),
                    local: Set(local),
                    address: Set(address),
                    password_hash: Set(password_hash),
                    quota: Set(quota_of(params.quota)),
                    aliases: Set(Names(aliases)),
                    forwards: Set(Names(forwards)),
                    suspended: Set(false),
                    state: Set(STATE_PENDING.to_owned()),
                    error: Set(None),
                    created_at: Set(now),
                    updated_at: Set(now),
                }
                .insert(tx)
                .await?;
                apply_to(&call, mail, &row.id);
                Ok(row)
            }
        })
}

pub(crate) fn edit(mail: Arc<Mail>) -> FunctionDef {
    function::<MailboxEdit, mailbox::Model>(EDIT)
        .kind(Kind::Form)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .owned::<mailbox::Entity>()
        .ui(Form::new("hosting.mail.mailbox.edit.title")
            .read(GET)
            .page("main", ["password", "quota"])
            .page("routing", ["aliases", "forwards"]))
        .handler(move |call: Call<MailboxEdit>| {
            let mail = Arc::clone(&mail);
            async move {
                let params = call.params();
                let tx = call.tx()?;
                let current = load(tx, &params.id).await?;
                let aliases = names::aliases(&current.local, &params.aliases)?;
                let forwards = names::forwards(&current.address, &params.forwards)?;
                refuse_taken(
                    tx,
                    Some(&current.id),
                    &current.domain_id,
                    &current.local,
                    &aliases,
                )
                .await?;
                let now = call.require::<dyn Clock>()?.now();
                let mut row = current.into_active_model();
                if let Some(password) = &params.password {
                    names::check_password(password)?;
                    row.password_hash = Set(names::hash(password).await?);
                }
                row.quota = Set(quota_of(params.quota));
                row.aliases = Set(Names(aliases));
                row.forwards = Set(Names(forwards));
                row.state = Set(STATE_PENDING.to_owned());
                row.updated_at = Set(now);
                let row = row.update(tx).await?;
                apply_to(&call, mail, &row.id);
                Ok(row)
            }
        })
}

pub(crate) fn delete(mail: Arc<Mail>) -> FunctionDef {
    function::<Batch, Deleted>(DELETE)
        .kind(Kind::Action)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .key("ids")
        .owned::<mailbox::Entity>()
        .handler(move |call: Call<Batch>| {
            let mail = Arc::clone(&mail);
            async move {
                let ids: Vec<String> = call.params().ids.iter().map(ToString::to_string).collect();
                let gone = drop_rows(call.tx()?, mailbox::Column::Id.is_in(ids)).await?;
                let deleted = u64::try_from(gone.len()).unwrap_or(u64::MAX);
                apply_after_commit(&call, mail, gone);
                Ok(Deleted { deleted })
            }
        })
}

pub(crate) async fn drop_rows<C: ConnectionTrait>(
    db: &C,
    filter: impl keel::sdk::sea_orm::sea_query::IntoCondition,
) -> Result<Vec<String>> {
    let condition = filter.into_condition();
    let gone: Vec<String> = mailbox::Entity::find()
        .filter(condition.clone())
        .order_by_asc(mailbox::Column::Address)
        .all(db)
        .await?
        .into_iter()
        .map(|row| row.id)
        .collect();
    if !gone.is_empty() {
        mailbox::Entity::delete_many()
            .filter(condition)
            .exec(db)
            .await?;
    }
    Ok(gone)
}

pub(crate) async fn domain_of<P>(call: &Call<P>, id: &Id) -> Result<domain::Model> {
    let tx = call.tx()?;
    let missing = || KeelError::not_found(DOMAIN).field("domain");
    let row = domain::Entity::find_by_id(id.to_string())
        .one(tx)
        .await?
        .ok_or_else(missing)?;
    let account: Id = row.account_id.parse().map_err(|_| missing())?;
    if call.account_scope().includes(tx, account).await? {
        Ok(row)
    } else {
        Err(missing())
    }
}

fn apply_to<P>(call: &Call<P>, mail: Arc<Mail>, id: &str) {
    let changed = Changed {
        mailboxes: vec![id.to_owned()],
        ..Changed::default()
    };
    call.after_commit(move |ctx| async move { mail.apply(&ctx, changed).await });
}

fn quota_of(quota: u32) -> i32 {
    i32::try_from(quota.min(QUOTA_MAX)).unwrap_or(i32::MAX)
}

async fn load<C: ConnectionTrait>(db: &C, id: &Id) -> Result<mailbox::Model> {
    mailbox::Entity::find_by_id(id.to_string())
        .one(db)
        .await?
        .ok_or_else(|| KeelError::not_found(MISSING).param("id", id.to_string()))
}

async fn refuse_taken<C: ConnectionTrait>(
    db: &C,
    except: Option<&str>,
    domain: &str,
    local: &str,
    aliases: &[String],
) -> Result<()> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        SERIALISE,
        [domain.into()],
    ))
    .await?;
    let names: Vec<&str> = std::iter::once(local)
        .chain(aliases.iter().map(String::as_str))
        .collect();
    let taken = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            TAKEN,
            [
                except.unwrap_or_default().into(),
                domain.into(),
                serde_json::to_string(&names)?.into(),
            ],
        ))
        .await?;
    let Some(row) = taken else {
        return Ok(());
    };
    let name: String = row.try_get_by_index(0)?;
    let field = if name == local { "local" } else { "aliases" };
    Err(KeelError::conflict(EXISTS).field(field).param("name", name))
}
