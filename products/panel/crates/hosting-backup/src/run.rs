use std::ffi::OsStr;
use std::path::Path;

use keel::prelude::*;
use keel::sdk::sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, IntoActiveModel, QueryFilter,
    QueryOrder, Set,
};
use keel::sdk::serde_json;
use keel::types::ErrorCode;
use keel_host::api::{Dest, Host, HostExecutor, Origin};
use keel_hosting_db::api::Databases;
use keel_hosting_mail::api::{Mailbox, Mailboxes};
use keel_hosting_web::api::Sites;

use crate::backup::{self, Names};
use crate::ops::{Contents, DatabaseDump, MailboxDir, PACK, SiteArchive, UNPACK};
use crate::{KEEP_DEFAULT, PRODUCT, STATE_ACTIVE, STATE_ERROR, STATE_PENDING, names, schedule};

pub(crate) const MISSING: &str = "hosting.backup.error.id";
pub(crate) const NOT_READY: &str = "hosting.backup.error.not_ready";

const SITE_GET: &str = "hosting.site.get";
const MARIADB: &str = "mariadb.exec";
const DUMPS_ROOT: &str = "dumps";
const ARCHIVES_ROOT: &str = "backups";
const SITE: &str = "hosting.backup.error.site";
const SITE_ROW: &str = "hosting.backup.error.site_row";
const SITE_GONE: &str = "hosting.backup.error.site_gone";

#[derive(Deserialize)]
#[serde(crate = "keel::sdk::serde", rename_all = "camelCase")]
pub(crate) struct SiteView {
    pub(crate) id: String,
    pub(crate) account_id: String,
    pub(crate) user_id: String,
    pub(crate) domain: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
pub(crate) struct Restored {
    pub(crate) id: String,
    pub(crate) files: u64,
    pub(crate) databases: Vec<String>,
    pub(crate) mailboxes: Vec<String>,
}

struct Target {
    account: Id,
    user: Id,
    home: String,
    root: String,
}

pub(crate) async fn take<P>(call: &Call<P>, site: &Id, scheduled: bool) -> Result<backup::Model> {
    let view = site_of(call, site).await?;
    let target = target(call, &view).await?;
    let db = call.db();
    let databases = call
        .require::<dyn Databases>()?
        .of_user(db, &target.account, &target.user)
        .await?;
    let mailboxes = call
        .require::<dyn Mailboxes>()?
        .of_domain(db, &target.account, &view.domain)
        .await?;
    let clock = call.require::<dyn Clock>()?;
    let now = clock.now();
    let id = Id::generate().to_string();
    let row = backup::ActiveModel {
        id: Set(id.clone()),
        account_id: Set(view.account_id.clone()),
        site_id: Set(view.id.clone()),
        domain: Set(view.domain.clone()),
        file: Set(names::archive_of(&id)),
        bytes: Set(None),
        databases: Set(Names(databases.clone())),
        mailboxes: Set(Names(
            mailboxes
                .iter()
                .map(|mailbox| mailbox.address.clone())
                .collect(),
        )),
        scheduled: Set(scheduled),
        state: Set(STATE_PENDING.to_owned()),
        error: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await?;
    note(
        call,
        &format!("backing up {} into {}", row.domain, row.file),
    )
    .await?;
    let executor = call.require::<dyn HostExecutor>()?;
    let host = executor.on(origin(call));
    let dumps = dumps_of(&id, &databases);
    let request = SiteArchive {
        archive: row.file.clone(),
        domain: row.domain.clone(),
        home: target.home,
        root: target.root,
        databases: dumps.clone(),
        mailboxes: maildirs(mailboxes),
    };
    let outcome = pack(call, &host, &request).await;
    remove_dumps(call, &host, &dumps).await?;
    let mut active = row.into_active_model();
    active.updated_at = Set(clock.now());
    let contents = match outcome {
        Ok(contents) => contents,
        Err(error) => {
            active.state = Set(STATE_ERROR.to_owned());
            active.error = Set(Some(detail(&error)));
            active.update(db).await?;
            return Err(error);
        }
    };
    active.state = Set(STATE_ACTIVE.to_owned());
    active.bytes = Set(Some(i64::try_from(contents.bytes).unwrap_or(i64::MAX)));
    active.mailboxes = Set(Names(contents.mailboxes.clone()));
    let row = active.update(db).await?;
    note(
        call,
        &format!(
            "the archive holds {} file(s), {} database(s) and {} mailbox(es) in {} bytes",
            contents.files,
            contents.databases.len(),
            contents.mailboxes.len(),
            contents.bytes
        ),
    )
    .await?;
    let dropped = retain(db, &host, &row.site_id).await?;
    if dropped > 0 {
        note(
            call,
            &format!("removed {dropped} older archive(s) of the site"),
        )
        .await?;
    }
    Ok(row)
}

pub(crate) async fn restore<P>(call: &Call<P>, id: &Id) -> Result<Restored> {
    let db = call.db();
    let row = load(db, id).await?;
    if row.state != STATE_ACTIVE {
        return Err(KeelError::conflict(NOT_READY).param("id", id.to_string()));
    }
    let site: Id = row
        .site_id
        .parse()
        .map_err(|_| KeelError::internal(SITE_ROW).param("site", row.site_id.as_str()))?;
    let view = site_of(call, &site).await?;
    let target = target(call, &view).await?;
    let databases: Vec<String> = call
        .require::<dyn Databases>()?
        .of_user(db, &target.account, &target.user)
        .await?
        .into_iter()
        .filter(|name| row.databases.0.contains(name))
        .collect();
    let mailboxes: Vec<Mailbox> = call
        .require::<dyn Mailboxes>()?
        .of_domain(db, &target.account, &view.domain)
        .await?
        .into_iter()
        .filter(|mailbox| row.mailboxes.0.contains(&mailbox.address))
        .collect();
    let dumps = dumps_of(&Id::generate().to_string(), &databases);
    let request = SiteArchive {
        archive: row.file.clone(),
        domain: row.domain.clone(),
        home: target.home,
        root: target.root,
        databases: dumps.clone(),
        mailboxes: maildirs(mailboxes),
    };
    let executor = call.require::<dyn HostExecutor>()?;
    let host = executor.on(origin(call));
    note(call, &format!("restoring {} from {}", row.domain, row.file)).await?;
    let outcome = unpack(call, &host, &request).await;
    remove_dumps(call, &host, &dumps).await?;
    let contents = outcome?;
    note(
        call,
        &format!(
            "restored {} file(s), {} database(s) and {} mailbox(es)",
            contents.files,
            contents.databases.len(),
            contents.mailboxes.len()
        ),
    )
    .await?;
    Ok(Restored {
        id: row.id,
        files: contents.files,
        databases: contents.databases,
        mailboxes: contents.mailboxes,
    })
}

pub(crate) async fn site_of<P>(call: &Call<P>, site: &Id) -> Result<SiteView> {
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

pub(crate) async fn load<C: ConnectionTrait>(db: &C, id: &Id) -> Result<backup::Model> {
    backup::Entity::find_by_id(id.to_string())
        .one(db)
        .await?
        .ok_or_else(|| KeelError::not_found(MISSING).param("id", id.to_string()))
}

pub(crate) async fn drop_backups<C: ConnectionTrait>(
    db: &C,
    filter: impl keel::sdk::sea_orm::sea_query::IntoCondition,
) -> Result<Vec<String>> {
    let condition = filter.into_condition();
    let files: Vec<String> = backup::Entity::find()
        .filter(condition.clone())
        .all(db)
        .await?
        .into_iter()
        .map(|row| row.file)
        .collect();
    if !files.is_empty() {
        backup::Entity::delete_many()
            .filter(condition)
            .exec(db)
            .await?;
    }
    Ok(files)
}

pub(crate) fn forget_after_commit<P>(call: &Call<P>, files: Vec<String>) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }
    let executor = call.require::<dyn HostExecutor>()?;
    call.after_commit(move |ctx| async move {
        let host = executor.on(Origin::new(PRODUCT, ctx.principal(), ctx.request_id()));
        remove_archives(&host, &files).await
    });
    Ok(())
}

async fn pack<P>(call: &Call<P>, host: &Host<'_>, request: &SiteArchive) -> Result<Contents> {
    for dump in &request.databases {
        note(call, &format!("dumping the database {}", dump.database)).await?;
        host.op::<_, Value>(
            MARIADB,
            &json!({
                "statement": "dump",
                "database": dump.database,
                "dest": { "root": DUMPS_ROOT, "name": dump.dump },
            }),
        )
        .await?;
    }
    host.op(PACK, request).await
}

async fn unpack<P>(call: &Call<P>, host: &Host<'_>, request: &SiteArchive) -> Result<Contents> {
    let contents: Contents = host.op(UNPACK, request).await?;
    for dump in &request.databases {
        if !contents.databases.contains(&dump.database) {
            continue;
        }
        note(call, &format!("importing the database {}", dump.database)).await?;
        host.op::<_, Value>(
            MARIADB,
            &json!({
                "statement": "import",
                "database": dump.database,
                "source": { "root": DUMPS_ROOT, "name": dump.dump },
            }),
        )
        .await?;
    }
    Ok(contents)
}

async fn target<P>(call: &Call<P>, view: &SiteView) -> Result<Target> {
    let unreadable = || KeelError::internal(SITE_ROW).param("site", view.id.as_str());
    let site: Id = view.id.parse().map_err(|_| unreadable())?;
    let account: Id = view.account_id.parse().map_err(|_| unreadable())?;
    let user: Id = view.user_id.parse().map_err(|_| unreadable())?;
    let root = call
        .require::<dyn Sites>()?
        .document_root(call.db(), &site)
        .await?
        .ok_or_else(|| KeelError::not_found(SITE_GONE).param("site", view.id.as_str()))?;
    let home = Path::new(&root.home)
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(unreadable)?
        .to_owned();
    Ok(Target {
        account,
        user,
        home,
        root: root.root,
    })
}

async fn retain(db: &Db, host: &Host<'_>, site: &str) -> Result<usize> {
    let keep = schedule::Entity::find()
        .filter(schedule::Column::SiteId.eq(site))
        .one(db)
        .await?
        .and_then(|row| usize::try_from(row.keep).ok())
        .unwrap_or(KEEP_DEFAULT);
    let older: Vec<backup::Model> = backup::Entity::find()
        .filter(backup::Column::SiteId.eq(site))
        .filter(backup::Column::State.eq(STATE_ACTIVE))
        .order_by_desc(backup::Column::CreatedAt)
        .order_by_desc(backup::Column::Id)
        .all(db)
        .await?
        .into_iter()
        .skip(keep)
        .collect();
    if older.is_empty() {
        return Ok(0);
    }
    let ids: Vec<String> = older.iter().map(|row| row.id.clone()).collect();
    let files: Vec<String> = older.into_iter().map(|row| row.file).collect();
    drop_backups(db, backup::Column::Id.is_in(ids)).await?;
    remove_archives(host, &files).await?;
    Ok(files.len())
}

async fn remove_archives(host: &Host<'_>, files: &[String]) -> Result<()> {
    let mut outcome = Ok(());
    for file in files {
        if let Err(error) = host
            .fs()
            .remove(&Dest::new(ARCHIVES_ROOT, file), false)
            .await
        {
            outcome = outcome.and(Err(error));
        }
    }
    outcome
}

async fn remove_dumps<P>(call: &Call<P>, host: &Host<'_>, dumps: &[DatabaseDump]) -> Result<()> {
    for dump in dumps {
        if let Err(error) = host
            .fs()
            .remove(&Dest::new(DUMPS_ROOT, &dump.dump), false)
            .await
        {
            note(
                call,
                &format!("the dump {} stays behind: {}", dump.dump, detail(&error)),
            )
            .await?;
        }
    }
    Ok(())
}

fn dumps_of(run: &str, databases: &[String]) -> Vec<DatabaseDump> {
    databases
        .iter()
        .enumerate()
        .map(|(index, database)| DatabaseDump {
            database: database.clone(),
            dump: names::dump_of(run, index),
        })
        .collect()
}

fn maildirs(mailboxes: Vec<Mailbox>) -> Vec<MailboxDir> {
    mailboxes
        .into_iter()
        .map(|mailbox| MailboxDir {
            address: mailbox.address,
            maildir: mailbox.maildir,
        })
        .collect()
}

fn origin<P>(call: &Call<P>) -> Origin {
    Origin::new(PRODUCT, call.principal(), call.request_id())
}

async fn note<P>(call: &Call<P>, line: &str) -> Result<()> {
    match call.task() {
        Some(task) => task.log(line).await,
        None => Ok(()),
    }
}

fn detail(error: &KeelError) -> String {
    error
        .params()
        .get("detail")
        .and_then(Value::as_str)
        .map_or_else(|| error.key().to_owned(), str::to_owned)
}
