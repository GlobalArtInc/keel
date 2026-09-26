use std::io;
use std::path::Path;
use std::sync::OnceLock;

use keel::prelude::*;
use keel::sdk::sea_orm::sea_query::Expr;
use keel::sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use keel::sdk::tokio;
use keel_host::api::{Dest, HostExecutor, Origin};

use crate::config::DatabaseConfig;
use crate::{PRODUCT, database, grant, login};

const OP: &str = "mariadb.exec";
const DUMP_ROOT: &str = "dumps";
const DUMP_PREFIX: &str = "keel-panel-";
const NOT_STARTED: &str = "hosting.database.error.detached";
const BACKUP_FAILED: &str = "hosting.database.error.backup";

#[derive(Serialize)]
#[serde(crate = "keel::sdk::serde", tag = "statement")]
enum Statement<'a> {
    #[serde(rename = "database.create")]
    DatabaseCreate { database: &'a str },
    #[serde(rename = "database.drop")]
    DatabaseDrop { database: &'a str },
    #[serde(rename = "user.create")]
    UserCreate { user: &'a str, password: &'a str },
    #[serde(rename = "user.password")]
    UserPassword { user: &'a str, password: &'a str },
    #[serde(rename = "user.drop")]
    UserDrop { user: &'a str },
    #[serde(rename = "user.lock")]
    UserLock { user: &'a str },
    #[serde(rename = "user.unlock")]
    UserUnlock { user: &'a str },
    #[serde(rename = "grant")]
    Grant { user: &'a str, database: &'a str },
    #[serde(rename = "revoke")]
    Revoke { user: &'a str, database: &'a str },
    #[serde(rename = "dump")]
    Dump { database: &'a str, dest: &'a Dest },
}

pub(crate) struct Gone {
    pub(crate) name: String,
    pub(crate) grantees: Vec<String>,
}

pub(crate) struct Change {
    pub(crate) password: Option<Secret>,
    pub(crate) granted: Vec<String>,
    pub(crate) revoked: Vec<String>,
}

pub(crate) struct Effects {
    host: Arc<dyn HostExecutor>,
    clock: Arc<dyn Clock>,
}

impl Effects {
    pub(crate) fn of<P>(call: &Call<P>) -> Result<Self> {
        Ok(Self {
            host: call.require::<dyn HostExecutor>()?,
            clock: call.require::<dyn Clock>()?,
        })
    }

    async fn run(&self, ctx: &AfterContext, statement: &Statement<'_>) -> Result<()> {
        let origin = Origin::new(PRODUCT, ctx.principal(), ctx.request_id());
        let _: Value = self.host.on(origin).op(OP, statement).await?;
        Ok(())
    }

    pub(crate) async fn create_database(&self, ctx: &AfterContext, id: &str) -> Result<()> {
        let db = ctx.db()?;
        let Some(row) = database::Entity::find_by_id(id).one(db).await? else {
            return Ok(());
        };
        if row.state != database::STATE_PENDING {
            return Ok(());
        }
        let made = self
            .run(
                ctx,
                &Statement::DatabaseCreate {
                    database: &row.name,
                },
            )
            .await;
        let (state, error) = outcome(&made, database::STATE_ACTIVE, database::STATE_ERROR);
        database::Entity::update_many()
            .col_expr(database::Column::State, Expr::value(state))
            .col_expr(database::Column::Error, Expr::value(error))
            .col_expr(database::Column::UpdatedAt, Expr::value(self.clock.now()))
            .filter(database::Column::Id.eq(id))
            .filter(database::Column::State.eq(database::STATE_PENDING))
            .exec(db)
            .await?;
        made
    }

    pub(crate) async fn drop_databases(&self, ctx: &AfterContext, gone: &[Gone]) -> Result<()> {
        for database in gone {
            for user in &database.grantees {
                self.run(
                    ctx,
                    &Statement::Revoke {
                        user,
                        database: &database.name,
                    },
                )
                .await?;
            }
            self.run(
                ctx,
                &Statement::DatabaseDrop {
                    database: &database.name,
                },
            )
            .await?;
        }
        Ok(())
    }

    pub(crate) async fn create_login(
        &self,
        ctx: &AfterContext,
        id: &str,
        password: &Secret,
    ) -> Result<()> {
        let db = ctx.db()?;
        let Some(row) = login::Entity::find_by_id(id).one(db).await? else {
            return Ok(());
        };
        if row.state != login::STATE_PENDING {
            return Ok(());
        }
        let databases = granted(db, id).await?;
        let made = async {
            self.run(
                ctx,
                &Statement::UserCreate {
                    user: &row.name,
                    password: password.expose(),
                },
            )
            .await?;
            for database in &databases {
                self.run(
                    ctx,
                    &Statement::Grant {
                        user: &row.name,
                        database,
                    },
                )
                .await?;
            }
            Ok(())
        }
        .await;
        let (state, error) = outcome(&made, login::STATE_ACTIVE, login::STATE_ERROR);
        login::Entity::update_many()
            .col_expr(login::Column::State, Expr::value(state))
            .col_expr(login::Column::Error, Expr::value(error))
            .col_expr(login::Column::UpdatedAt, Expr::value(self.clock.now()))
            .filter(login::Column::Id.eq(id))
            .filter(login::Column::State.eq(login::STATE_PENDING))
            .exec(db)
            .await?;
        made
    }

    pub(crate) async fn change_login(
        &self,
        ctx: &AfterContext,
        id: &str,
        change: &Change,
    ) -> Result<()> {
        let db = ctx.db()?;
        let Some(row) = login::Entity::find_by_id(id).one(db).await? else {
            return Ok(());
        };
        if !login::exists(&row.state) {
            return Ok(());
        }
        let user = row.name.as_str();
        let changed = async {
            if let Some(password) = &change.password {
                self.run(
                    ctx,
                    &Statement::UserPassword {
                        user,
                        password: password.expose(),
                    },
                )
                .await?;
            }
            for database in &change.granted {
                self.run(ctx, &Statement::Grant { user, database }).await?;
            }
            for database in &change.revoked {
                self.run(ctx, &Statement::Revoke { user, database }).await?;
            }
            Ok(())
        }
        .await;
        if let Err(error) = &changed {
            login::Entity::update_many()
                .col_expr(login::Column::State, Expr::value(login::STATE_ERROR))
                .col_expr(login::Column::Error, Expr::value(detail(error)))
                .col_expr(login::Column::UpdatedAt, Expr::value(self.clock.now()))
                .filter(login::Column::Id.eq(id))
                .exec(db)
                .await?;
        }
        changed
    }

    pub(crate) async fn lock(
        &self,
        ctx: &AfterContext,
        names: &[String],
        locked: bool,
    ) -> Result<()> {
        for user in names {
            let statement = if locked {
                Statement::UserLock { user }
            } else {
                Statement::UserUnlock { user }
            };
            self.run(ctx, &statement).await?;
        }
        Ok(())
    }

    pub(crate) async fn drop_logins(&self, ctx: &AfterContext, names: &[String]) -> Result<()> {
        for user in names {
            self.run(ctx, &Statement::UserDrop { user }).await?;
        }
        Ok(())
    }
}

struct Attached {
    host: Arc<dyn HostExecutor>,
    config: Arc<DatabaseConfig>,
}

#[derive(Default)]
pub(crate) struct Dumps {
    attached: OnceLock<Attached>,
}

impl Dumps {
    pub(crate) fn attach(&self, ctx: &Context) -> Result<()> {
        let _ = self.attached.set(Attached {
            host: ctx.require::<dyn HostExecutor>()?,
            config: ctx.config::<DatabaseConfig>()?,
        });
        Ok(())
    }

    pub(crate) async fn write(&self, dir: &BackupDir) -> Result<()> {
        let attached = self
            .attached
            .get()
            .ok_or_else(|| KeelError::unavailable(NOT_STARTED))?;
        let rows = database::Entity::find()
            .filter(database::Column::State.eq(database::STATE_ACTIVE))
            .order_by_asc(database::Column::Name)
            .all(dir.db())
            .await?;
        let request = Id::generate().to_string();
        let host = attached.host.on(Origin::installation(PRODUCT, &request));
        for row in rows {
            let file = format!("{DUMP_PREFIX}{}.sql", row.id);
            let dest = Dest::new(DUMP_ROOT, &file);
            let _: Value = host
                .op(
                    OP,
                    &Statement::Dump {
                        database: &row.name,
                        dest: &dest,
                    },
                )
                .await?;
            let source = attached.config.dump_path(&file);
            let copied = copy(
                Path::new(&source),
                &dir.path().join(format!("{}.sql", row.name)),
            )
            .await;
            host.fs().remove(&dest, false).await?;
            copied.map_err(|error| {
                KeelError::internal(BACKUP_FAILED)
                    .param("file", source.as_str())
                    .source(error)
            })?;
        }
        Ok(())
    }
}

async fn copy(source: &Path, target: &Path) -> io::Result<()> {
    let meta = tokio::fs::symlink_metadata(source).await?;
    if !meta.is_file() {
        return Err(io::Error::other("not a regular file"));
    }
    tokio::fs::copy(source, target).await.map(drop)
}

async fn granted(db: &Db, login: &str) -> Result<Vec<String>> {
    let ids: Vec<String> = grant::Entity::find()
        .filter(grant::Column::DatabaseUserId.eq(login))
        .all(db)
        .await?
        .into_iter()
        .map(|row| row.database_id)
        .collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(database::Entity::find()
        .filter(database::Column::Id.is_in(ids))
        .order_by_asc(database::Column::Name)
        .all(db)
        .await?
        .into_iter()
        .map(|row| row.name)
        .collect())
}

fn outcome(
    result: &Result<()>,
    active: &'static str,
    failed: &'static str,
) -> (&'static str, Option<String>) {
    match result {
        Ok(()) => (active, None),
        Err(error) => (failed, Some(detail(error))),
    }
}

fn detail(error: &KeelError) -> String {
    error
        .params()
        .get("detail")
        .and_then(Value::as_str)
        .map_or_else(|| error.key().to_owned(), str::to_owned)
}
