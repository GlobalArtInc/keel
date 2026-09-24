use keel_sdk::TaskHandle;
use keel_sdk::lifecycle::{AccountChange, AccountLifecycle};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{EntityTrait, QuerySelect};
use keel_sdk::types::AccountState;

use super::accounts::state_name;
use super::entity::account;
use crate::dispatcher::{CallerHandle, Dispatcher, Unit};

pub(super) const STATE_CHANGED: &str = "keel.account.error.state_changed";
const PROGRESS_MODULES: &str = "keel.account.delete.progress.modules";
const PROGRESS_ROW: &str = "keel.account.delete.progress.row";

#[derive(Default, Deserialize, Serialize)]
struct Checkpoint {
    done: Vec<String>,
}

pub(super) fn dispatcher<P>(call: &Call<P>, function: &str) -> Result<Dispatcher> {
    call.require::<CallerHandle>()?.dispatcher().ok_or_else(|| {
        KeelError::unavailable("keel.error.caller_unavailable").param("function", function)
    })
}

fn contributed(dispatcher: &Dispatcher, change: AccountChange) -> Vec<AccountLifecycle> {
    dispatcher
        .registry()
        .contributions()
        .of::<AccountLifecycle>()
        .filter(|lifecycle| lifecycle.handler(change).is_some())
        .cloned()
        .collect()
}

pub(super) async fn after_change(
    dispatcher: Dispatcher,
    context: AfterContext,
    account: Id,
    change: AccountChange,
    state: AccountState,
) -> Result<()> {
    for lifecycle in contributed(&dispatcher, change) {
        let unit = Unit {
            module: lifecycle.module(),
            principal: context.principal(),
            request_id: context.request_id(),
            task: None,
        };
        match run(&dispatcher, unit, &lifecycle, change, account, state).await {
            Ok(()) => {}
            Err(error) if error.key() == STATE_CHANGED => {
                tracing::debug!(
                    module = lifecycle.module(),
                    change = change.as_str(),
                    account = %account,
                    "the account changed again before its lifecycle ran; the later change walks the modules"
                );
            }
            Err(error) => {
                tracing::error!(
                    module = lifecycle.module(),
                    change = change.as_str(),
                    account = %account,
                    error = %error,
                    "an account lifecycle handler failed"
                );
            }
        }
    }
    Ok(())
}

pub(super) async fn delete_modules<P>(
    call: &Call<P>,
    task: &TaskHandle,
    function: &str,
    account: Id,
) -> Result<(Vec<String>, u64)> {
    let dispatcher = dispatcher(call, function)?;
    let mut done = task.resume_from::<Checkpoint>()?.unwrap_or_default().done;
    let mut lifecycles = contributed(&dispatcher, AccountChange::Delete);
    lifecycles.reverse();
    let total = u64::try_from(lifecycles.len())
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut step = 0;
    for lifecycle in &lifecycles {
        let module = lifecycle.module();
        if done.iter().any(|finished| finished == module) {
            step += 1;
            continue;
        }
        if task.is_cancelled() {
            return Err(task.cancelled());
        }
        task.progress(step, Some(total), Some(PROGRESS_MODULES))
            .await?;
        let unit = Unit {
            module,
            principal: call.principal(),
            request_id: call.request_id(),
            task: Some(task.clone()),
        };
        run(
            &dispatcher,
            unit,
            lifecycle,
            AccountChange::Delete,
            account,
            AccountState::Deleting,
        )
        .await
        .map_err(|error| error.param("module", module))?;
        done.push(module.to_owned());
        task.checkpoint(&Checkpoint { done: done.clone() }).await?;
        task.log(&format!("deleted {module}")).await?;
        step += 1;
    }
    task.progress(step, Some(total), Some(PROGRESS_ROW)).await?;
    Ok((done, total))
}

async fn run(
    dispatcher: &Dispatcher,
    unit: Unit<'_>,
    lifecycle: &AccountLifecycle,
    change: AccountChange,
    account: Id,
    state: AccountState,
) -> Result<()> {
    let Some(handler) = lifecycle.handler(change).cloned() else {
        return Ok(());
    };
    dispatcher
        .unit(unit, account, |call: Call<Id>| async move {
            holds(&call, account, state).await?;
            handler(call).await
        })
        .await
}

async fn holds(call: &Call<Id>, account: Id, state: AccountState) -> Result<()> {
    let row = account::Entity::find_by_id(account.to_string())
        .lock_shared()
        .one(call.tx()?)
        .await?
        .ok_or_else(|| KeelError::not_found("keel.error.account_not_found"))?;
    let expected = state_name(state);
    if row.state == expected {
        return Ok(());
    }
    Err(KeelError::conflict(STATE_CHANGED)
        .param("state", row.state)
        .param("expected", expected))
}
