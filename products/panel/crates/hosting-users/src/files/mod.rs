mod agent;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use keel::prelude::*;
use keel::sdk::sea_orm::EntityTrait;
use keel::sdk::system::{TaskRunner, TaskSpec};

use crate::config::HostingConfig;
use crate::{FEATURE, user};
use agent::{Home, Listing, stage, staged_name, unstage};

pub(crate) const READ: &str = "hosting.file.read";
pub(crate) const MANAGE: &str = "hosting.file.manage";
pub(crate) const VIEW: &str = "hosting.file-manager";
const LIST: &str = "hosting.file.list";
const MKDIR: &str = "hosting.file.mkdir";
const RENAME: &str = "hosting.file.rename";
const DELETE: &str = "hosting.file.delete";
const CHMOD: &str = "hosting.file.chmod";
const UPLOAD: &str = "hosting.file.upload";
const DOWNLOAD: &str = "hosting.file.download";
const APPLY: &str = "hosting.file.apply";
const CLASS: &str = "hosting.file";
const KEY: &str = "user";
const PATH_MAX: u32 = 4096;
const UPLOAD_MAX: u64 = 32 * 1024 * 1024;
const MODE: &str = r"^0?[0-7]{3}$";
const HOMELESS: &str = "hosting.file.error.homeless";
const UNREADABLE: &str = "hosting.file.error.unreadable";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Folder {
    user: Id,
    #[serde(default)]
    #[schemars(length(max = PATH_MAX))]
    path: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Entry {
    user: Id,
    #[schemars(length(min = 1, max = PATH_MAX))]
    path: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Move {
    user: Id,
    #[schemars(length(min = 1, max = PATH_MAX))]
    from: String,
    #[schemars(length(min = 1, max = PATH_MAX))]
    to: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Permissions {
    user: Id,
    #[schemars(length(min = 1, max = PATH_MAX))]
    path: String,
    #[schemars(pattern(MODE))]
    mode: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Upload {
    #[schemars(extend("x-keel" = json!({ "options": "hosting.user.options" })))]
    user: Id,
    #[schemars(length(min = 1, max = PATH_MAX))]
    path: String,
    #[schemars(extend(
        "contentEncoding" = "base64",
        "x-keel" = json!({ "secret": true, "file": { "maxBytes": UPLOAD_MAX, "types": ["application/octet-stream"] } }),
        "x-ui" = json!({ "control": "file" })
    ))]
    content: String,
}

#[derive(Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", rename_all = "lowercase")]
#[schemars(crate = "keel::sdk::schemars")]
enum Op {
    Mkdir,
    Rename,
    Delete,
    Chmod,
    Write,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Apply {
    user: Id,
    op: Op,
    #[schemars(length(min = 1, max = PATH_MAX))]
    path: String,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    staged: Option<String>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars", rename = "HostingFileApplied")]
struct Applied {
    path: String,
}

impl Apply {
    fn new(user: Id, op: Op, path: &str) -> Self {
        Self {
            user,
            op,
            path: path.to_owned(),
            to: None,
            mode: None,
            staged: None,
        }
    }
}

pub(crate) fn list() -> FunctionDef {
    function::<Folder, Listing>(LIST)
        .kind(Kind::Get)
        .access(Level::User, READ)
        .feature(FEATURE)
        .key(KEY)
        .owned::<user::Entity>()
        .handler(|call: Call<Folder>| async move {
            let row = housed(&call, &call.params().user).await?;
            Home::of(&call, &row)?.list(&call.params().path).await
        })
}

pub(crate) fn download() -> FunctionDef {
    function::<Entry, DownloadFile>(DOWNLOAD)
        .access(Level::User, READ)
        .feature(FEATURE)
        .key(KEY)
        .owned::<user::Entity>()
        .download(|call: Call<Entry>| async move {
            let row = housed(&call, &call.params().user).await?;
            let config = call.config::<HostingConfig>()?;
            let download = Home::of(&call, &row)?
                .download(&config, &call.params().path)
                .await?;
            Ok(DownloadOutcome::Body(download))
        })
}

pub(crate) fn mkdir() -> FunctionDef {
    change::<Entry>(MKDIR, |params| {
        Apply::new(params.user, Op::Mkdir, &params.path)
    })
}

pub(crate) fn rename() -> FunctionDef {
    change::<Move>(RENAME, |params| Apply {
        to: Some(params.to.clone()),
        ..Apply::new(params.user, Op::Rename, &params.from)
    })
}

pub(crate) fn delete() -> FunctionDef {
    change::<Entry>(DELETE, |params| {
        Apply::new(params.user, Op::Delete, &params.path)
    })
}

pub(crate) fn chmod() -> FunctionDef {
    change::<Permissions>(CHMOD, |params| Apply {
        mode: Some(params.mode.clone()),
        ..Apply::new(params.user, Op::Chmod, &params.path)
    })
}

pub(crate) fn upload() -> FunctionDef {
    function::<Upload, TaskRef>(UPLOAD)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .key(KEY)
        .owned::<user::Entity>()
        .ui(Form::new("hosting.file.upload.title").page("main", ["user", "path", "content"]))
        .upload(|call: Call<Upload>| async move {
            let params = call.params();
            housed(&call, &params.user).await?;
            let bytes = STANDARD
                .decode(params.content.as_bytes())
                .map_err(|_| KeelError::validation(UNREADABLE).field("content"))?;
            let config = call.config::<HostingConfig>()?;
            let staged = staged_name("upload");
            stage(&config, &staged, &bytes).await?;
            let apply = Apply {
                staged: Some(staged.clone()),
                ..Apply::new(params.user, Op::Write, &params.path)
            };
            let spawned = spawn(&call, &apply).await;
            if spawned.is_err() {
                unstage(&config, &staged).await;
            }
            spawned
        })
}

pub(crate) fn apply() -> FunctionDef {
    function::<Apply, Applied>(APPLY)
        .kind(Kind::Action)
        .mode(Mode::Read)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .key(KEY)
        .owned::<user::Entity>()
        .internal()
        .class(CLASS)
        .handler(|call: Call<Apply>| async move {
            let params = call.params();
            let row = housed(&call, &params.user).await?;
            let home = Home::of(&call, &row)?;
            let path = params.path.as_str();
            match params.op {
                Op::Mkdir => home.mkdir(path).await?,
                Op::Rename => {
                    home.rename(path, required(params.to.as_deref(), "to")?)
                        .await?
                }
                Op::Delete => home.remove(path).await?,
                Op::Chmod => {
                    home.chmod(path, required(params.mode.as_deref(), "mode")?)
                        .await?
                }
                Op::Write => {
                    let config = call.config::<HostingConfig>()?;
                    home.write(&config, path, required(params.staged.as_deref(), "staged")?)
                        .await?;
                }
            }
            Ok(Applied {
                path: path.to_owned(),
            })
        })
}

fn change<P>(name: &str, apply: fn(&P) -> Apply) -> FunctionDef
where
    P: HasUser + for<'de> Deserialize<'de> + JsonSchema + Send + Sync + 'static,
{
    function::<P, TaskRef>(name)
        .kind(Kind::Action)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .key(KEY)
        .owned::<user::Entity>()
        .handler(move |call: Call<P>| async move {
            housed(&call, call.params().user()).await?;
            spawn(&call, &apply(call.params())).await
        })
}

trait HasUser {
    fn user(&self) -> &Id;
}

impl HasUser for Entry {
    fn user(&self) -> &Id {
        &self.user
    }
}

impl HasUser for Move {
    fn user(&self) -> &Id {
        &self.user
    }
}

impl HasUser for Permissions {
    fn user(&self) -> &Id {
        &self.user
    }
}

fn required<'a>(value: Option<&'a str>, field: &str) -> Result<&'a str> {
    value.ok_or_else(|| KeelError::validation("keel.error.validation").field(field))
}

async fn spawn<P>(call: &Call<P>, apply: &Apply) -> Result<TaskRef> {
    let runner = call.require::<dyn TaskRunner>()?;
    let id = runner
        .spawn(TaskSpec {
            function: APPLY.to_owned(),
            params: keel::sdk::serde_json::to_value(apply)?,
            principal: call.principal().clone(),
            class: CLASS.to_owned(),
            resumable: false,
            request_id: Some(call.request_id().to_owned()),
        })
        .await?;
    Ok(TaskRef::new(id))
}

async fn housed<P>(call: &Call<P>, id: &Id) -> Result<user::Model> {
    let row = user::Entity::find_by_id(id.to_string())
        .one(call.db())
        .await?
        .ok_or_else(|| KeelError::not_found("keel.error.not_found").field(KEY))?;
    if row.uid.is_none() || row.home.is_none() {
        return Err(KeelError::conflict(HOMELESS)
            .field(KEY)
            .param("user", row.name));
    }
    Ok(row)
}
