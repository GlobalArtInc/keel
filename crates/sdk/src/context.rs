use std::any::{Any, type_name};
use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::future::BoxFuture;

use crate::db::Db;
use crate::error::{KeelError, Result};
use crate::registrar::Contributions;
use crate::services::Services;

pub trait Spawner: Send + Sync {
    fn spawn(&self, task: BoxFuture<'static, ()>);
}

#[derive(Clone)]
pub struct Context {
    module: Arc<str>,
    contributions: Arc<Contributions>,
    services: Arc<Services>,
    spawner: Arc<dyn Spawner>,
    db: Db,
    data_dir: Arc<Path>,
}

impl Context {
    #[doc(hidden)]
    pub fn new(
        module: &str,
        contributions: Arc<Contributions>,
        services: Arc<Services>,
        spawner: Arc<dyn Spawner>,
        db: Db,
        data_dir: PathBuf,
    ) -> Self {
        Self {
            module: Arc::from(module),
            contributions,
            services,
            spawner,
            db,
            data_dir: Arc::from(data_dir),
        }
    }

    pub fn module(&self) -> &str {
        &self.module
    }

    pub fn contributions<T: Any + Send + Sync>(&self) -> impl Iterator<Item = &T> {
        self.contributions.of::<T>()
    }

    pub fn require<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Arc<T>> {
        self.services.get::<T>().ok_or_else(|| {
            KeelError::internal("keel.error.service_missing").param("service", type_name::<T>())
        })
    }

    pub fn config<T: Send + Sync + 'static>(&self) -> Result<Arc<T>> {
        self.services.get::<T>().ok_or_else(|| {
            KeelError::internal("keel.error.config_missing").param("section", type_name::<T>())
        })
    }

    pub fn spawn<F>(&self, task: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.spawner.spawn(Box::pin(task));
    }

    pub(crate) fn db_slot(&self) -> &Db {
        &self.db
    }

    pub(crate) fn data_dir_slot(&self) -> &Path {
        &self.data_dir
    }
}

impl fmt::Debug for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Context")
            .field("module", &self.module)
            .field("contributions", &self.contributions)
            .field("services", &self.services)
            .field("data_dir", &self.data_dir)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct Inline(Mutex<Vec<BoxFuture<'static, ()>>>);

    impl Spawner for Inline {
        fn spawn(&self, task: BoxFuture<'static, ()>) {
            self.0.lock().unwrap().push(task);
        }
    }

    trait Clock: Send + Sync {
        fn now(&self) -> u64;
    }

    struct Fixed;

    impl Clock for Fixed {
        fn now(&self) -> u64 {
            7
        }
    }

    struct Widget(&'static str);

    #[tokio::test]
    async fn a_context_reads_the_bag_resolves_services_and_hands_tasks_to_the_spawner() {
        let mut contributions = Contributions::default();
        contributions.add(Widget("a"));
        contributions.add(Widget("b"));
        let mut services = Services::new();
        services.provide::<dyn Clock>(Arc::new(Fixed));
        let spawner = Arc::new(Inline(Mutex::new(Vec::new())));
        let ctx = Context::new(
            "dns",
            Arc::new(contributions),
            Arc::new(services),
            spawner.clone(),
            Db::from_connection(sea_orm::DatabaseConnection::Disconnected),
            PathBuf::from("/var/lib/keel"),
        );
        assert_eq!(ctx.module(), "dns");
        assert!(!crate::system::ContextExt::db(&ctx).is_transaction());
        assert_eq!(
            crate::system::ContextExt::data_dir(&ctx),
            Path::new("/var/lib/keel")
        );
        let widgets: Vec<&str> = ctx.contributions::<Widget>().map(|w| w.0).collect();
        assert_eq!(widgets, ["a", "b"]);
        assert_eq!(ctx.contributions::<u8>().count(), 0);
        assert_eq!(ctx.require::<dyn Clock>().unwrap().now(), 7);
        let missing = ctx.require::<String>().unwrap_err();
        assert_eq!(missing.key(), "keel.error.service_missing");
        let unconfigured = ctx.config::<String>().unwrap_err();
        assert_eq!(unconfigured.key(), "keel.error.config_missing");
        assert!(
            unconfigured.params()["section"]
                .as_str()
                .is_some_and(|section| section.ends_with("String"))
        );

        let seen = Arc::new(Mutex::new(false));
        let flag = seen.clone();
        ctx.spawn(async move { *flag.lock().unwrap() = true });
        let tasks = std::mem::take(&mut *spawner.0.lock().unwrap());
        assert_eq!(tasks.len(), 1);
        for task in tasks {
            task.await;
        }
        assert!(*seen.lock().unwrap());
        assert!(format!("{ctx:?}").starts_with("Context { module: \"dns\""));
    }
}
