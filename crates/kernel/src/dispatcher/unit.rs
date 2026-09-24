use std::future::Future;

use keel_sdk::runtime::{CallBuilder, CallHandle};
use keel_sdk::{Call, Db, KeelError, TaskHandle};
use keel_types::Principal;
use sea_orm::TransactionTrait;

use super::{Dispatcher, Shared, outbox, spawn_after_commit, take};

pub(crate) struct Unit<'a> {
    pub(crate) module: &'a str,
    pub(crate) principal: &'a Principal,
    pub(crate) request_id: &'a str,
    pub(crate) task: Option<TaskHandle>,
}

impl Dispatcher {
    pub(crate) async fn unit<P, F, Fut>(
        &self,
        unit: Unit<'_>,
        params: P,
        work: F,
    ) -> Result<(), KeelError>
    where
        F: FnOnce(Call<P>) -> Fut,
        Fut: Future<Output = Result<(), KeelError>>,
    {
        let inner = &self.inner;
        let after = Shared::default();
        let handle = CallBuilder::new(unit.principal.clone(), unit.request_id)
            .services(inner.registry.services().clone())
            .invoker(inner.invoker(after.clone()))
            .task(unit.task)
            .write(inner.db.begin().await?);
        let outcome = match work(handle.call(params)).await {
            Ok(()) => self.record(&handle, unit.module, &after).await,
            Err(error) => Err(error),
        };
        if let Err(error) = outcome {
            if let Ok(Some(transaction)) = handle.finish() {
                transaction.rollback().await?;
            }
            return Err(error);
        }
        let mut hooks = take(&after);
        hooks.extend(handle.take_after_commit());
        let context = handle
            .after_context()
            .with_db(Db::from_connection(inner.db.clone()));
        let transaction = handle
            .finish()
            .map_err(|shared| {
                KeelError::internal("keel.error.call_still_shared").param("holders", shared.holders)
            })?
            .ok_or_else(|| KeelError::internal("keel.error.write_call_without_transaction"))?;
        transaction.commit().await?;
        if let (Some(feed), Some(seq)) = (&inner.outbox, after.written()) {
            feed.publish(seq);
        }
        if !hooks.is_empty() {
            spawn_after_commit(hooks, context);
        }
        Ok(())
    }

    async fn record(
        &self,
        handle: &CallHandle,
        module: &str,
        after: &Shared,
    ) -> Result<(), KeelError> {
        if let Some(seq) = outbox::write(
            handle.db(),
            module,
            handle.principal(),
            handle.take_events(),
            self.inner.clock.now(),
        )
        .await?
        {
            after.wrote(seq);
        }
        Ok(())
    }
}
