use keel::prelude::*;
use keel::sdk::chrono::{DateTime, Duration, Utc};
use keel::sdk::sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use keel::sdk::serde_json;
use keel::sdk::system::{TaskRunner, TaskSpec};
use keel::types::ErrorCode;

use crate::FEATURE;
use crate::acme::{self, DIRECTORY, Directory, EMAIL};
use crate::certificate::{
    self, KIND_ACME, KIND_UPLOADED, STATE_ACTIVE, STATE_ERROR, STATE_PENDING,
};
use crate::install::{Installer, Store, detail, origin, parse_id};
use crate::pem::Material;

pub(crate) const REQUEST: &str = "hosting.certificate.request";
pub(crate) const RENEW: &str = "hosting.certificate.renew";

const READ: &str = "hosting.certificate.read";
const MANAGE: &str = "hosting.certificate.manage";
const LIST: &str = "hosting.certificate.list";
const UPLOAD: &str = "hosting.certificate.upload";
const DELETE: &str = "hosting.certificate.delete";
const SITES: &str = "hosting.certificate.sites";
const ISSUE: &str = "hosting.certificate.issue";
const CLASS: &str = "hosting.certificate";
const SITE_GET: &str = "hosting.site.get";
const SITE_LIST: &str = "hosting.site.list";
const SITE: &str = "hosting.certificate.error.site";
const SITE_ROW: &str = "hosting.certificate.error.site_row";
const RENEW_BEFORE_DAYS: i64 = 30;
const CHOICES: u32 = 20;
const CERTIFICATE_MAX: u32 = 65_536;
const KEY_MAX: u32 = 16_384;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Pick {
    id: Id,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Nothing {}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Requested {
    #[schemars(extend("x-keel" = json!({ "options": SITES })))]
    site: Id,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Uploaded {
    #[schemars(extend("x-keel" = json!({ "options": SITES })))]
    site: Id,
    #[schemars(length(min = 1, max = CERTIFICATE_MAX), extend("x-ui" = json!({ "control": "code" })))]
    certificate: String,
    #[schemars(extend("x-keel" = json!({ "secret": true }), "maxLength" = KEY_MAX))]
    key: Secret,
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

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel::sdk::schemars", rename_all = "camelCase")]
struct Issued {
    id: String,
    domain: String,
    not_after: Option<DateTime<Utc>>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Renewing {
    renewing: u64,
}

#[derive(Deserialize)]
#[serde(crate = "keel::sdk::serde", rename_all = "camelCase")]
struct SiteView {
    id: String,
    account_id: String,
    domain: String,
    #[serde(default)]
    aliases: Vec<String>,
}

impl SiteView {
    fn names(&self) -> Vec<String> {
        std::iter::once(self.domain.clone())
            .chain(self.aliases.iter().cloned())
            .collect()
    }
}

pub(crate) fn list() -> FunctionDef {
    function::<ListQuery, Page<certificate::Model>>(LIST)
        .kind(Kind::List)
        .access(Level::User, READ)
        .feature(FEATURE)
        .owned::<certificate::Entity>()
        .ui(List::new("hosting.certificate.list.title")
            .search(["domain"])
            .column(Column::new("domain", ColumnType::Text).sortable().primary())
            .column(
                Column::new("kind", ColumnType::Status)
                    .map([(KIND_ACME, Tone::Info), (KIND_UPLOADED, Tone::Neutral)]),
            )
            .column(
                Column::new("state", ColumnType::Status)
                    .map([
                        (STATE_PENDING, Tone::Neutral),
                        (STATE_ACTIVE, Tone::Success),
                        (STATE_ERROR, Tone::Danger),
                    ])
                    .filter_options([STATE_PENDING, STATE_ACTIVE, STATE_ERROR]),
            )
            .column(Column::new("notAfter", ColumnType::Datetime).sortable())
            .column(Column::new("error", ColumnType::Text))
            .column(
                Column::new("accountId", ColumnType::Text)
                    .level(Level::Reseller)
                    .filter_remote("keel.account.options"),
            )
            .action(
                Action::new("request", ActionRole::New)
                    .screen(REQUEST)
                    .icon("lock"),
            )
            .action(
                Action::new("upload", ActionRole::Button)
                    .screen(UPLOAD)
                    .icon("upload"),
            )
            .action(
                Action::new("renew", ActionRole::Button)
                    .function(REQUEST)
                    .params([("site", "$siteId")])
                    .icon("refresh-cw")
                    .confirm(),
            )
            .action(
                Action::new("delete", ActionRole::Delete)
                    .function(DELETE)
                    .confirm_with(ConfirmDetails {
                        destructive: true,
                        items: Some("domain".to_owned()),
                        input: None,
                    }),
            )
            .row_actions(["renew", "delete"])
            .empty("hosting.certificate.empty", Some("request"))
            .invalidate_on(["hosting.certificate.*"]))
        .default_sort("domain", SortDir::Asc)
        .source(entity_source::<certificate::Entity>())
}

pub(crate) fn request(store: Arc<Store>) -> FunctionDef {
    function::<Requested, certificate::Model>(REQUEST)
        .kind(Kind::Form)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .owned::<certificate::Entity>()
        .ui(Form::new("hosting.certificate.request.title").page("main", ["site"]))
        .handler(move |call: Call<Requested>| {
            let store = Arc::clone(&store);
            async move {
                let site = site_of(&call, &call.params().site.to_string()).await?;
                let row = claim(&call, &store, &site, KIND_ACME).await?;
                spawn_after_commit(&call, vec![row.id.clone()])?;
                Ok(row)
            }
        })
}

pub(crate) fn upload(store: Arc<Store>) -> FunctionDef {
    function::<Uploaded, certificate::Model>(UPLOAD)
        .kind(Kind::Form)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .owned::<certificate::Entity>()
        .ui(Form::new("hosting.certificate.upload.title")
            .page("main", ["site", "certificate", "key"]))
        .handler(move |call: Call<Uploaded>| {
            let store = Arc::clone(&store);
            async move {
                let params = call.params();
                let site = site_of(&call, &params.site.to_string()).await?;
                let material = Material::uploaded(
                    &params.certificate,
                    params.key.expose(),
                    &site.names(),
                    store.now()?,
                )?;
                let row = claim(&call, &store, &site, KIND_UPLOADED).await?;
                let installer = Installer::of(&call, &store)?;
                let id = row.id.clone();
                call.after_commit(move |ctx| async move {
                    let origin = origin(ctx.principal(), ctx.request_id());
                    installer
                        .install(ctx.db()?, origin, &id, KIND_UPLOADED, material)
                        .await
                });
                Ok(row)
            }
        })
}

pub(crate) fn delete(store: Arc<Store>) -> FunctionDef {
    function::<Batch, Deleted>(DELETE)
        .kind(Kind::Action)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .key("ids")
        .owned::<certificate::Entity>()
        .handler(move |call: Call<Batch>| {
            let store = Arc::clone(&store);
            async move {
                let ids: Vec<String> = call.params().ids.iter().map(ToString::to_string).collect();
                let gone = drop_rows(call.tx()?, certificate::Column::Id.is_in(ids)).await?;
                let deleted = u64::try_from(gone.len()).unwrap_or(u64::MAX);
                let installer = Installer::of(&call, &store)?;
                call.after_commit(move |ctx| async move {
                    let origin = origin(ctx.principal(), ctx.request_id());
                    let db = ctx.db()?;
                    for row in gone {
                        installer.render(db, origin.clone(), &row.site_id).await?;
                        if let Some(stem) = row.installed {
                            installer.drop_files(origin.clone(), &[stem]).await?;
                        }
                    }
                    Ok(())
                });
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

pub(crate) fn issue(store: Arc<Store>) -> FunctionDef {
    function::<Pick, Issued>(ISSUE)
        .kind(Kind::Action)
        .mode(Mode::Read)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .unowned()
        .internal()
        .class(CLASS)
        .handler(move |call: Call<Pick>| {
            let store = Arc::clone(&store);
            async move { issue_one(&call, &store).await }
        })
}

pub(crate) fn renew(store: Arc<Store>) -> FunctionDef {
    function::<Nothing, Renewing>(RENEW)
        .kind(Kind::Action)
        .level(Level::Admin)
        .feature(FEATURE)
        .unowned()
        .internal()
        .handler(move |call: Call<Nothing>| {
            let store = Arc::clone(&store);
            async move {
                let due = store.now()? + Duration::days(RENEW_BEFORE_DAYS);
                let ids: Vec<String> = certificate::Entity::find()
                    .filter(certificate::Column::Kind.eq(KIND_ACME))
                    .filter(certificate::Column::NotAfter.lt(due))
                    .all(call.tx()?)
                    .await?
                    .into_iter()
                    .map(|row| row.id)
                    .collect();
                let renewing = u64::try_from(ids.len()).unwrap_or(u64::MAX);
                spawn_after_commit(&call, ids)?;
                Ok(Renewing { renewing })
            }
        })
}

async fn issue_one(call: &Call<Pick>, store: &Arc<Store>) -> Result<Issued> {
    let id = call.params().id.to_string();
    let db = call.db();
    let row = certificate::Entity::find_by_id(id.as_str())
        .one(db)
        .await?
        .ok_or_else(|| KeelError::not_found("keel.error.not_found"))?;
    let site = site_of(call, &row.site_id).await?;
    let names = site.names();
    let directory = Directory {
        url: call.setting::<String>(DIRECTORY).await?,
        email: call.setting::<String>(EMAIL).await?,
    };
    let installer = Installer::of(call, store)?;
    note(
        call,
        &format!(
            "ordering a certificate for {} from {}",
            names.join(", "),
            directory.url
        ),
    )
    .await?;
    let outcome = async {
        installer
            .render(
                db,
                origin(call.principal(), call.request_id()),
                &row.site_id,
            )
            .await?;
        let material = acme::order(call, &installer, &directory, &names).await?;
        let not_after = material.not_after;
        installer
            .install(
                db,
                origin(call.principal(), call.request_id()),
                &id,
                KIND_ACME,
                material,
            )
            .await?;
        Ok::<_, KeelError>(not_after)
    }
    .await;
    match outcome {
        Ok(not_after) => {
            note(
                call,
                &format!(
                    "installed the certificate of {}, valid until {}",
                    site.domain,
                    not_after.to_rfc3339()
                ),
            )
            .await?;
            Ok(Issued {
                id,
                domain: site.domain,
                not_after: Some(not_after),
            })
        }
        Err(error) => {
            installer.fail(db, &id, &error).await?;
            note(
                call,
                &format!(
                    "the certificate of {} was not issued: {}",
                    site.domain,
                    detail(&error)
                ),
            )
            .await?;
            Err(error)
        }
    }
}

async fn note<P>(call: &Call<P>, line: &str) -> Result<()> {
    match call.task() {
        Some(task) => task.log(line).await,
        None => Ok(()),
    }
}

async fn site_of<P>(call: &Call<P>, site: &str) -> Result<SiteView> {
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
    serde_json::from_value(value)
        .map_err(|error| KeelError::internal(SITE_ROW).param("detail", error.to_string()))
}

async fn claim<P>(
    call: &Call<P>,
    store: &Store,
    site: &SiteView,
    kind: &str,
) -> Result<certificate::Model> {
    let tx = call.tx()?;
    let account = call
        .resolve_account(Some(parse_id(&site.account_id)?))
        .await?;
    let now = store.now()?;
    let existing = certificate::Entity::find()
        .filter(certificate::Column::SiteId.eq(site.id.as_str()))
        .one(tx)
        .await?;
    let row = match existing {
        Some(existing) => {
            certificate::ActiveModel {
                id: Set(existing.id),
                state: Set(STATE_PENDING.to_owned()),
                error: Set(None),
                updated_at: Set(now),
                ..Default::default()
            }
            .update(tx)
            .await?
        }
        None => {
            certificate::ActiveModel {
                id: Set(Id::generate().to_string()),
                account_id: Set(account.to_string()),
                site_id: Set(site.id.clone()),
                domain: Set(site.domain.clone()),
                kind: Set(kind.to_owned()),
                state: Set(STATE_PENDING.to_owned()),
                error: Set(None),
                installed: Set(None),
                not_after: Set(None),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(tx)
            .await?
        }
    };
    Ok(row)
}

fn spawn_after_commit<P>(call: &Call<P>, ids: Vec<String>) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let runner = call.require::<dyn TaskRunner>()?;
    call.after_commit(move |ctx| async move {
        for id in ids {
            runner
                .spawn(TaskSpec {
                    function: ISSUE.to_owned(),
                    params: json!({ "id": id }),
                    principal: ctx.principal().clone(),
                    class: CLASS.to_owned(),
                    resumable: false,
                    request_id: Some(ctx.request_id().to_owned()),
                })
                .await?;
        }
        Ok(())
    });
    Ok(())
}

pub(crate) async fn drop_rows<C: keel::sdk::sea_orm::ConnectionTrait>(
    db: &C,
    filter: impl keel::sdk::sea_orm::sea_query::IntoCondition,
) -> Result<Vec<certificate::Model>> {
    let condition = filter.into_condition();
    let rows = certificate::Entity::find()
        .filter(condition.clone())
        .all(db)
        .await?;
    certificate::Entity::delete_many()
        .filter(condition)
        .exec(db)
        .await?;
    Ok(rows)
}
