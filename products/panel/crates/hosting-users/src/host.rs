use std::future::Future;
use std::time::Duration;

use keel::prelude::*;
use keel::sdk::sea_orm::sea_query::Expr;
use keel::sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use keel::sdk::tokio;
use keel_host::api::{Dest, HostExecutor, Origin};

use crate::{PRODUCT, USER_PREFIX, user};

const HOME_ROOT: &str = "www";
const HOME_PREFIX: &str = "keel-panel-";
const LOGIN_SHELL: &str = "/bin/bash";
const NO_LOGIN: &str = "/usr/sbin/nologin";
const ERROR_EXEC_FAILED: &str = "host.error.exec_failed";
const DELETE_ATTEMPTS: usize = 6;
const DELETE_PAUSE: Duration = Duration::from_secs(2);

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
            delete_with_retry(
                || users.delete(name, true),
                name,
                DELETE_ATTEMPTS,
                DELETE_PAUSE,
            )
            .await?;
        }
        Ok(())
    }
}

async fn delete_with_retry<F, Fut>(
    mut delete: F,
    name: &str,
    attempts: usize,
    pause: Duration,
) -> Result<bool>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<bool>>,
{
    let mut attempt = 1;
    loop {
        match delete().await {
            Err(error) if attempt < attempts && error.key() == ERROR_EXEC_FAILED => {
                tracing::warn!(
                    user = name,
                    attempt,
                    attempts,
                    error = error.key(),
                    "the host agent could not delete the user, retrying"
                );
                tokio::time::sleep(pause).await;
                attempt += 1;
            }
            outcome => return outcome,
        }
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use keel::sdk::types::ErrorCode;

    use super::*;

    fn exec_failed() -> KeelError {
        KeelError::with_key(ErrorCode::Internal, ERROR_EXEC_FAILED)
    }

    async fn run(
        calls: &AtomicUsize,
        attempts: usize,
        mut outcomes: Vec<Result<bool>>,
    ) -> Result<bool> {
        outcomes.reverse();
        let outcomes = std::sync::Mutex::new(outcomes);
        delete_with_retry(
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                let next = outcomes.lock().unwrap().pop().expect("an outcome per call");
                async move { next }
            },
            "hp_web",
            attempts,
            Duration::ZERO,
        )
        .await
    }

    #[tokio::test(crate = "keel::sdk::tokio")]
    async fn a_busy_user_deletion_is_retried_until_the_agent_lets_it_go() {
        let calls = AtomicUsize::new(0);
        let outcome = run(
            &calls,
            DELETE_ATTEMPTS,
            vec![Err(exec_failed()), Err(exec_failed()), Ok(true)],
        )
        .await;
        assert!(outcome.expect("the third attempt succeeds"));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(crate = "keel::sdk::tokio")]
    async fn a_user_deletion_that_keeps_failing_gives_up_after_six_attempts_with_the_original_error()
     {
        let calls = AtomicUsize::new(0);
        let outcomes = (0..DELETE_ATTEMPTS).map(|_| Err(exec_failed())).collect();
        let error = run(&calls, DELETE_ATTEMPTS, outcomes)
            .await
            .expect_err("every attempt fails");
        assert_eq!(error.key(), ERROR_EXEC_FAILED);
        assert_eq!(error.code(), ErrorCode::Internal);
        assert_eq!(calls.load(Ordering::SeqCst), 6);
    }

    #[tokio::test(crate = "keel::sdk::tokio")]
    async fn a_user_deletion_failure_that_is_not_a_busy_user_is_not_retried() {
        for error in [
            KeelError::with_key(ErrorCode::Forbidden, "host.error.policy"),
            KeelError::with_key(ErrorCode::NotFound, "host.error.not_found"),
            KeelError::with_key(ErrorCode::Unavailable, "host.error.unreachable"),
        ] {
            let key = error.key().to_owned();
            let calls = AtomicUsize::new(0);
            let failed = run(&calls, DELETE_ATTEMPTS, vec![Err(error)])
                .await
                .expect_err("the failure is final");
            assert_eq!(failed.key(), key);
            assert_eq!(calls.load(Ordering::SeqCst), 1, "{key}");
        }
    }
}
