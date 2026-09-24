use std::fmt;
use std::future::Future;
use std::sync::Arc;

use futures::future::BoxFuture;
use keel_types::Id;

use crate::call::Call;
use crate::error::Result;

pub type LifecycleHandler = Arc<dyn Fn(Call<Id>) -> BoxFuture<'static, Result<()>> + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountChange {
    Suspend,
    Resume,
    Delete,
    Rename,
}

impl AccountChange {
    pub const fn as_str(self) -> &'static str {
        match self {
            AccountChange::Suspend => "suspend",
            AccountChange::Resume => "resume",
            AccountChange::Delete => "delete",
            AccountChange::Rename => "rename",
        }
    }
}

#[derive(Clone, Default)]
pub struct AccountLifecycle {
    module: String,
    suspend: Option<LifecycleHandler>,
    resume: Option<LifecycleHandler>,
    delete: Option<LifecycleHandler>,
    rename: Option<LifecycleHandler>,
}

fn boxed<F, Fut>(handler: F) -> LifecycleHandler
where
    F: Fn(Call<Id>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    Arc::new(move |call| Box::pin(handler(call)))
}

impl AccountLifecycle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_suspend<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(Call<Id>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.suspend = Some(boxed(handler));
        self
    }

    pub fn on_resume<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(Call<Id>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.resume = Some(boxed(handler));
        self
    }

    pub fn on_delete<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(Call<Id>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.delete = Some(boxed(handler));
        self
    }

    pub fn on_rename<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(Call<Id>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.rename = Some(boxed(handler));
        self
    }

    pub fn module(&self) -> &str {
        &self.module
    }

    pub fn handler(&self, change: AccountChange) -> Option<&LifecycleHandler> {
        match change {
            AccountChange::Suspend => self.suspend.as_ref(),
            AccountChange::Resume => self.resume.as_ref(),
            AccountChange::Delete => self.delete.as_ref(),
            AccountChange::Rename => self.rename.as_ref(),
        }
    }

    pub(crate) fn stamp(&mut self, module: &str) {
        module.clone_into(&mut self.module);
    }
}

impl fmt::Debug for AccountLifecycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let handles = [
            AccountChange::Suspend,
            AccountChange::Resume,
            AccountChange::Delete,
            AccountChange::Rename,
        ]
        .into_iter()
        .filter(|change| self.handler(*change).is_some())
        .map(AccountChange::as_str)
        .collect::<Vec<_>>();
        f.debug_struct("AccountLifecycle")
            .field("module", &self.module)
            .field("handles", &handles)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use keel_types::Level;

    use super::*;
    use crate::tests::read_handle;

    #[tokio::test]
    async fn a_lifecycle_answers_the_handlers_it_was_given_under_the_module_a_registrar_stamped() {
        let mut lifecycle = AccountLifecycle::new()
            .on_suspend(|_| async { Ok(()) })
            .on_delete(|call: Call<Id>| async move {
                assert_eq!(call.principal().level, Level::User);
                Err(crate::KeelError::conflict("probe.refused")
                    .param("account", call.params().to_string()))
            });
        assert_eq!(lifecycle.module(), "");
        lifecycle.stamp("probe");
        assert_eq!(lifecycle.module(), "probe");
        assert!(lifecycle.handler(AccountChange::Suspend).is_some());
        assert!(lifecycle.handler(AccountChange::Resume).is_none());
        assert!(lifecycle.handler(AccountChange::Rename).is_none());
        let delete = lifecycle
            .handler(AccountChange::Delete)
            .expect("the delete handler")
            .clone();
        let account = Id::ROOT_ACCOUNT;
        let error = delete(read_handle().call(account))
            .await
            .expect_err("the handler refuses");
        assert_eq!(error.key(), "probe.refused");
        assert_eq!(
            format!("{lifecycle:?}"),
            r#"AccountLifecycle { module: "probe", handles: ["suspend", "delete"] }"#
        );
        assert_eq!(AccountChange::Rename.as_str(), "rename");
    }
}
