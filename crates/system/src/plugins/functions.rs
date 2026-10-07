use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ActiveModelTrait, EntityTrait, Set};
use keel_sdk::system::{Extension, NewPrincipal, PrincipalSource, TaskRunner, TaskSpec};

use super::discover::{read, root};
use super::entity::state;
use super::manifest::{LEVELS, Manifest, Rights, Storage};

pub const LIST: &str = "plugin.list";
pub const GET: &str = "plugin.get";
pub const ENABLE: &str = "plugin.enable";
pub const DISABLE: &str = "plugin.disable";
pub const ERROR_UNKNOWN: &str = "plugin.error.unknown";
const ERROR_MANIFEST: &str = "plugin.error.manifest";
const ERROR_LEVEL: &str = "plugin.error.level";
const ERROR_ID: &str = "plugin.error.id";
pub const LOGIN_PREFIX: &str = "plugin-";
const ROLE: &str = "plugin";
const LANG: &str = "en";
const PRINCIPAL_UPDATE: &str = "keel.principal.update";
const RESTART: &str = "keel.restart";
const RESTART_CLASS: &str = "restart";
const ID_PATTERN: &str = "^[a-z][a-z0-9_]{0,63}$";

pub(super) struct Started {
    pub(super) data_dir: PathBuf,
    pub(super) extensions: Vec<Extension>,
}

pub(super) type Shared = Arc<OnceLock<Started>>;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct PluginPick {
    #[schemars(regex(pattern = ID_PATTERN))]
    id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "lowercase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "lowercase")]
enum Status {
    Loaded,
    Refused,
    Unloaded,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
struct PluginRow {
    id: String,
    version: Option<String>,
    level: Option<Level>,
    enabled: bool,
    status: Status,
    reason: Option<String>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
struct PluginDetail {
    id: String,
    version: Option<String>,
    level: Option<Level>,
    enabled: bool,
    status: Status,
    reason: Option<String>,
    mode: Option<String>,
    storage: bool,
    functions: Vec<String>,
    observe: Vec<String>,
    hook: Vec<String>,
    patch: Vec<String>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct PluginChange {
    id: String,
    enabled: bool,
    changed: bool,
    restart: bool,
}

pub(super) fn list(shared: Shared) -> FunctionDef {
    function::<ListQuery, Page<PluginRow>>(LIST)
        .kind(Kind::List)
        .level(Level::Admin)
        .unowned()
        .key("id")
        .ui(List::new("plugin.list.title")
            .column(Column::new("id", ColumnType::Text).primary())
            .column(Column::new("version", ColumnType::Text))
            .column(Column::new("level", ColumnType::Text))
            .column(Column::new("enabled", ColumnType::Boolean))
            .column(Column::new("status", ColumnType::Status).map([
                ("loaded", Tone::Success),
                ("refused", Tone::Danger),
                ("unloaded", Tone::Neutral),
            ]))
            .column(Column::new("reason", ColumnType::Text))
            .action(
                Action::new("enable", ActionRole::Button)
                    .function(ENABLE)
                    .params([("id", "$id")])
                    .confirm(),
            )
            .action(
                Action::new("disable", ActionRole::Button)
                    .function(DISABLE)
                    .params([("id", "$id")])
                    .confirm(),
            )
            .row_actions(["enable", "disable"])
            .empty("plugin.empty", None))
        .handler(move |call: Call<ListQuery>| {
            let shared = shared.clone();
            async move {
                let started = started(&shared)?;
                let mut rows = Vec::new();
                for id in known(started, call.db()).await? {
                    rows.push(describe(started, call.db(), &id).await?.0);
                }
                let total = rows.len() as u64;
                Ok(Page::new(rows, total))
            }
        })
}

pub(super) fn get(shared: Shared) -> FunctionDef {
    function::<PluginPick, PluginDetail>(GET)
        .kind(Kind::Get)
        .level(Level::Admin)
        .unowned()
        .key("id")
        .handler(move |call: Call<PluginPick>| {
            let shared = shared.clone();
            async move {
                let started = started(&shared)?;
                let id = checked(&call.params().id)?;
                if !known(started, call.db()).await?.contains(id) {
                    return Err(unknown(id));
                }
                let (row, manifest) = describe(started, call.db(), id).await?;
                let mode = manifest
                    .as_ref()
                    .map(|manifest| manifest.run.mode.as_str().to_owned());
                let storage = manifest
                    .as_ref()
                    .is_some_and(|manifest| manifest.storage == Some(Storage::Kv));
                let (functions, rights) = match manifest {
                    Some(manifest) => (
                        manifest
                            .functions
                            .into_iter()
                            .map(|function| function.name)
                            .collect(),
                        manifest.rights,
                    ),
                    None => (Vec::new(), Rights::default()),
                };
                Ok(PluginDetail {
                    id: row.id,
                    version: row.version,
                    level: row.level,
                    enabled: row.enabled,
                    status: row.status,
                    reason: row.reason,
                    mode,
                    storage,
                    functions,
                    observe: rights.observe,
                    hook: rights.hook,
                    patch: rights.patch,
                })
            }
        })
}

pub(super) fn enable(shared: Shared) -> FunctionDef {
    function::<PluginPick, PluginChange>(ENABLE)
        .kind(Kind::Action)
        .level(Level::Admin)
        .unowned()
        .handler(move |call: Call<PluginPick>| {
            let shared = shared.clone();
            async move {
                let started = started(&shared)?;
                let id = checked(&call.params().id)?.to_owned();
                let directory = root(&started.data_dir).join(&id);
                if !tokio::fs::try_exists(&directory).await.unwrap_or(false) {
                    return Err(unknown(&id));
                }
                let manifest = read(&directory)
                    .await
                    .map_err(|reason| manifest_error(&id, &reason))?
                    .manifest;
                if manifest.id != id {
                    return Err(manifest_error(
                        &id,
                        &format!("the id {} differs from its directory {id}", manifest.id),
                    ));
                }
                if !LEVELS.contains(&manifest.level) {
                    return Err(KeelError::validation(ERROR_LEVEL)
                        .field("id")
                        .param("id", id.as_str())
                        .param("level", manifest.level.as_str()));
                }
                let changed = grant(&call, &id, manifest.level).await?;
                finish(&call, &id, true, changed)
            }
        })
}

pub(super) fn disable() -> FunctionDef {
    function::<PluginPick, PluginChange>(DISABLE)
        .kind(Kind::Action)
        .level(Level::Admin)
        .unowned()
        .handler(|call: Call<PluginPick>| async move {
            let id = checked(&call.params().id)?.to_owned();
            let tx = call.tx()?;
            let Some(row) = state::Entity::find_by_id(id.clone()).one(tx).await? else {
                return Err(unknown(&id));
            };
            let changed = row.enabled;
            if changed {
                let mut row: state::ActiveModel = row.into();
                row.enabled = Set(false);
                row.changed_at = Set(call.require::<dyn Clock>()?.now());
                row.changed_by = Set(call.principal().login.clone());
                row.update(tx).await?;
            }
            finish(&call, &id, false, changed)
        })
}

async fn grant(call: &Call<PluginPick>, id: &str, level: Level) -> Result<bool> {
    let tx = call.tx()?;
    let now = call.require::<dyn Clock>()?.now();
    let by = call.principal().login.clone();
    let Some(row) = state::Entity::find_by_id(id.to_owned()).one(tx).await? else {
        let principal = call
            .require::<dyn PrincipalSource>()?
            .create(
                NewPrincipal {
                    account: Id::ROOT_ACCOUNT,
                    login: format!("{LOGIN_PREFIX}{id}"),
                    level,
                    role: ROLE.to_owned(),
                    lang: LANG.to_owned(),
                },
                now,
                tx,
            )
            .await?;
        state::ActiveModel {
            id: Set(id.to_owned()),
            enabled: Set(true),
            level: Set(level.as_str().to_owned()),
            principal_id: Set(principal.id.to_string()),
            changed_at: Set(now),
            changed_by: Set(by),
        }
        .insert(tx)
        .await?;
        return Ok(true);
    };
    let regranted = row.level != level.as_str();
    if regranted {
        call.invoke(
            PRINCIPAL_UPDATE,
            json!({ "id": row.principal_id, "level": level }),
        )
        .await?;
    }
    if row.enabled && !regranted {
        return Ok(false);
    }
    let mut row: state::ActiveModel = row.into();
    row.enabled = Set(true);
    row.level = Set(level.as_str().to_owned());
    row.changed_at = Set(now);
    row.changed_by = Set(by);
    row.update(tx).await?;
    Ok(true)
}

fn finish(call: &Call<PluginPick>, id: &str, enabled: bool, changed: bool) -> Result<PluginChange> {
    if changed {
        tracing::info!(
            plugin = id,
            enabled,
            by = call.principal().login,
            "the plugin set changed; a restart applies it"
        );
        request_restart(call, id, enabled)?;
    }
    Ok(PluginChange {
        id: id.to_owned(),
        enabled,
        changed,
        restart: changed,
    })
}

fn request_restart(call: &Call<PluginPick>, id: &str, enabled: bool) -> Result<()> {
    let Ok(runner) = call.require::<dyn TaskRunner>() else {
        tracing::warn!(
            plugin = id,
            "no task runner is composed; the change takes effect at the next start"
        );
        return Ok(());
    };
    let verb = if enabled { "enable" } else { "disable" };
    let reason = format!("plugin {verb} {id}");
    let requested_at: DateTime<Utc> = call.require::<dyn Clock>()?.now();
    call.after_commit(move |context| async move {
        let spawned = runner
            .spawn(TaskSpec {
                function: RESTART.to_owned(),
                params: json!({ "reason": reason, "requestedAt": requested_at }),
                principal: context.principal().clone(),
                class: RESTART_CLASS.to_owned(),
                resumable: false,
                request_id: Some(context.request_id().to_owned()),
            })
            .await?;
        tracing::info!(task = %spawned, reason, "the restart task is queued");
        Ok(())
    });
    Ok(())
}

fn started(shared: &Shared) -> Result<&Started> {
    shared
        .get()
        .ok_or_else(|| KeelError::unavailable("plugin.error.not_started"))
}

fn checked(id: &str) -> Result<&str> {
    let mut chars = id.chars();
    let valid = chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && id.len() <= 64;
    if valid {
        Ok(id)
    } else {
        Err(KeelError::validation(ERROR_ID).field("id").param("id", id))
    }
}

async fn known(started: &Started, db: &Db) -> Result<BTreeSet<String>> {
    let mut ids: BTreeSet<String> = state::Entity::find()
        .all(db)
        .await?
        .into_iter()
        .map(|row| row.id)
        .collect();
    let root = root(&started.data_dir);
    if let Ok(mut entries) = tokio::fs::read_dir(&root).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let is_dir = entry
                .file_type()
                .await
                .map(|kind| kind.is_dir())
                .unwrap_or(false);
            if let Some(name) = entry.file_name().to_str()
                && is_dir
                && checked(name).is_ok()
            {
                ids.insert(name.to_owned());
            }
        }
    }
    Ok(ids)
}

async fn describe(started: &Started, db: &Db, id: &str) -> Result<(PluginRow, Option<Manifest>)> {
    let row = state::Entity::find_by_id(id.to_owned()).one(db).await?;
    let directory = root(&started.data_dir).join(id);
    let (manifest, unread) = match read(&directory).await {
        Ok(read) => (Some(read.manifest), None),
        Err(reason) => (None, Some(reason)),
    };
    let outcome = started
        .extensions
        .iter()
        .find(|extension| extension.id == id);
    let (status, reason) = match outcome {
        Some(Extension {
            refused: Some(reason),
            ..
        }) => (Status::Refused, Some(reason.clone())),
        Some(_) => (Status::Loaded, None),
        None => (Status::Unloaded, unread),
    };
    Ok((
        PluginRow {
            id: id.to_owned(),
            version: manifest.as_ref().map(|manifest| manifest.version.clone()),
            level: manifest.as_ref().map(|manifest| manifest.level),
            enabled: row.is_some_and(|row| row.enabled),
            status,
            reason,
        },
        manifest,
    ))
}

fn unknown(id: &str) -> KeelError {
    KeelError::validation(ERROR_UNKNOWN)
        .field("id")
        .param("id", id)
}

fn manifest_error(id: &str, reason: &str) -> KeelError {
    KeelError::validation(ERROR_MANIFEST)
        .field("id")
        .param("id", id)
        .param("reason", reason)
}
