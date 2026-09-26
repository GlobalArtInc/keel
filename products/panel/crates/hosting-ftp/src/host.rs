use std::collections::BTreeMap;
use std::sync::OnceLock;

use keel::prelude::*;
use keel::sdk::chrono::{DateTime, Utc};
use keel::sdk::sea_orm::sea_query::Expr;
use keel::sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use keel::sdk::tokio::sync::Mutex;
use keel_host::api::{Dest, Host, HostExecutor, Origin};
use keel_hosting_web::api::Sites;

use crate::{PRODUCT, STATE_ACTIVE, STATE_ERROR, account, names};

pub(crate) const PASSWD_TEMPLATE: &str = "panel/vsftpd-passwd.j2";
pub(crate) const USER_TEMPLATE: &str = "panel/vsftpd-user.conf.j2";
pub(crate) const VSFTPD_CHECK: &str = "vsftpd.check";

const PASSWD_ROOT: &str = "ftp";
const USERS_ROOT: &str = "ftp-users";
const PASSWD_FILE: &str = "keel-panel-passwd";
const MODE: &str = "0600";
const DETACHED: &str = "hosting.ftp.error.detached";
const SITE_GONE: &str = "hosting.ftp.error.site_gone";

#[derive(Default)]
pub(crate) struct Changed {
    pub(crate) accounts: Vec<String>,
    pub(crate) gone: Vec<String>,
}

#[derive(Serialize)]
#[serde(crate = "keel::sdk::serde")]
struct Login {
    login: String,
    hash: String,
}

struct Attached {
    host: Arc<dyn HostExecutor>,
    sites: Arc<dyn Sites>,
    clock: Arc<dyn Clock>,
}

#[derive(Default)]
pub(crate) struct Ftp {
    turn: Mutex<()>,
    attached: OnceLock<Attached>,
}

impl Ftp {
    pub(crate) fn attach(&self, ctx: &Context) -> Result<()> {
        let _ = self.attached.set(Attached {
            host: ctx.require::<dyn HostExecutor>()?,
            sites: ctx.require::<dyn Sites>()?,
            clock: ctx.require::<dyn Clock>()?,
        });
        Ok(())
    }

    pub(crate) async fn apply(&self, ctx: &AfterContext, changed: Changed) -> Result<()> {
        let attached = self
            .attached
            .get()
            .ok_or_else(|| KeelError::unavailable(DETACHED))?;
        let _turn = self.turn.lock().await;
        let db = ctx.db()?;
        let host = attached
            .host
            .on(Origin::new(PRODUCT, ctx.principal(), ctx.request_id()));
        let rows = account::Entity::find()
            .order_by_asc(account::Column::Login)
            .all(db)
            .await?;
        let placed: Vec<&account::Model> = rows
            .iter()
            .filter(|row| row.state != STATE_ACTIVE || changed.accounts.contains(&row.id))
            .collect();
        let mut failed = BTreeMap::new();
        for row in &placed {
            if let Err(error) = place(&host, db, attached.sites.as_ref(), row).await {
                failed.insert(row.id.clone(), error);
            }
        }
        let logins: Vec<Login> = rows
            .iter()
            .filter(|row| !row.suspended && !failed.contains_key(&row.id))
            .map(|row| Login {
                login: row.login.clone(),
                hash: row.password_hash.clone(),
            })
            .collect();
        let written = host
            .template(PASSWD_TEMPLATE)
            .vars(&json!({ "accounts": logins }))
            .dest(PASSWD_ROOT, PASSWD_FILE)
            .mode(MODE)
            .validate(VSFTPD_CHECK)
            .apply()
            .await
            .map(drop);
        let mut removed = Ok(());
        for login in &changed.gone {
            if let Err(error) = host.fs().remove(&Dest::new(USERS_ROOT, login), false).await {
                removed = removed.and(Err(error));
            }
        }
        settle(
            db,
            attached.clock.now(),
            &placed,
            &failed,
            written.as_ref().err(),
        )
        .await?;
        let first = failed.into_values().next().map_or(Ok(()), Err);
        written.and(removed).and(first)
    }
}

async fn place(host: &Host<'_>, db: &Db, sites: &dyn Sites, row: &account::Model) -> Result<()> {
    let gone = || KeelError::internal(SITE_GONE).param("site", row.site_id.as_str());
    let site: Id = row.site_id.parse().map_err(|_| gone())?;
    let root = sites.document_root(db, &site).await?.ok_or_else(gone)?;
    let directory = names::directory(&row.directory)?;
    host.template(USER_TEMPLATE)
        .vars(&json!({
            "user": root.user,
            "root": names::chroot(&root.path, directory),
        }))
        .dest(USERS_ROOT, &row.login)
        .mode(MODE)
        .validate(VSFTPD_CHECK)
        .apply()
        .await
        .map(drop)
}

async fn settle(
    db: &Db,
    now: DateTime<Utc>,
    placed: &[&account::Model],
    failed: &BTreeMap<String, KeelError>,
    passwd: Option<&KeelError>,
) -> Result<()> {
    for row in placed {
        let error = failed.get(&row.id).or(passwd).map(detail);
        let state = if error.is_some() {
            STATE_ERROR
        } else {
            STATE_ACTIVE
        };
        account::Entity::update_many()
            .col_expr(account::Column::State, Expr::value(state))
            .col_expr(account::Column::Error, Expr::value(error))
            .col_expr(account::Column::UpdatedAt, Expr::value(now))
            .filter(account::Column::Id.eq(row.id.as_str()))
            .exec(db)
            .await?;
    }
    Ok(())
}

fn detail(error: &KeelError) -> String {
    error
        .params()
        .get("detail")
        .and_then(Value::as_str)
        .map_or_else(|| error.key().to_owned(), str::to_owned)
}
