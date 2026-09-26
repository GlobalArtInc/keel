use keel::prelude::*;
use keel::sdk::sea_orm::sea_query::Expr;
use keel::sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use keel_host::api::{Dest, HostExecutor, Origin};

use crate::{PRODUCT, USER_PREFIX, user};

const HOME_ROOT: &str = "www";
const HOME_PREFIX: &str = "keel-panel-";
const LOGIN_SHELL: &str = "/bin/bash";
const NO_LOGIN: &str = "/usr/sbin/nologin";

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

    pub(crate) async fn create(&self, ctx: &AfterContext, id: &str) -> Result<()> {
        let db = ctx.db()?;
        let Some(row) = user::Entity::find_by_id(id).one(db).await? else {
            return Ok(());
        };
        if row.state != user::STATE_PENDING {
            return Ok(());
        }
        let home = Dest::new(HOME_ROOT, &home_of(&row.name));
        let made = self
            .host
            .on(origin(ctx))
            .users()
            .create(&row.name, Some(shell_of(row.shell)), Some(&home))
            .await;
        let update = user::Entity::update_many()
            .col_expr(user::Column::UpdatedAt, Expr::value(self.clock.now()))
            .filter(user::Column::Id.eq(id))
            .filter(user::Column::State.eq(user::STATE_PENDING));
        match made {
            Ok(created) => {
                update
                    .col_expr(user::Column::State, Expr::value(user::STATE_ACTIVE))
                    .col_expr(user::Column::Uid, Expr::value(i64::from(created.item.uid)))
                    .col_expr(user::Column::Home, Expr::value(created.item.home))
                    .col_expr(user::Column::Error, Expr::value(Option::<String>::None))
                    .exec(db)
                    .await?;
                Ok(())
            }
            Err(error) => {
                update
                    .col_expr(user::Column::State, Expr::value(user::STATE_ERROR))
                    .col_expr(user::Column::Error, Expr::value(detail(&error)))
                    .exec(db)
                    .await?;
                Err(error)
            }
        }
    }

    pub(crate) async fn shell(&self, ctx: &AfterContext, name: &str, shell: bool) -> Result<()> {
        self.host
            .on(origin(ctx))
            .users()
            .set_shell(name, shell_of(shell))
            .await
            .map(drop)
    }

    pub(crate) async fn lock(
        &self,
        ctx: &AfterContext,
        names: &[String],
        locked: bool,
    ) -> Result<()> {
        let host = self.host.on(origin(ctx));
        let users = host.users();
        for name in names {
            if locked {
                users.lock(name).await?;
            } else {
                users.unlock(name).await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn remove(&self, ctx: &AfterContext, names: &[String]) -> Result<()> {
        let host = self.host.on(origin(ctx));
        let users = host.users();
        for name in names {
            users.delete(name, true).await?;
        }
        Ok(())
    }
}

fn home_of(name: &str) -> String {
    format!(
        "{HOME_PREFIX}{}",
        name.strip_prefix(USER_PREFIX).unwrap_or(name)
    )
}

fn shell_of(shell: bool) -> &'static str {
    if shell { LOGIN_SHELL } else { NO_LOGIN }
}

fn origin(ctx: &AfterContext) -> Origin {
    Origin::new(PRODUCT, ctx.principal(), ctx.request_id())
}

fn detail(error: &KeelError) -> String {
    error
        .params()
        .get("detail")
        .and_then(Value::as_str)
        .map_or_else(|| error.key().to_owned(), str::to_owned)
}
