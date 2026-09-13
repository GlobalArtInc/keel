use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use axum::Router;
use chrono::{DateTime, Utc};
use keel_kernel::dispatcher::SchemaError;
use keel_kernel::loader::BootError;
use keel_kernel::secret::{Key, KeyError};
use keel_kernel::{
    CallOptions, Composition, Dispatcher, Http, HttpOptions, Kernel, Readiness, Registry, Reply,
    Storage,
};
use keel_sdk::{ModuleDef, ProductManifest};
use keel_types::{Level, Principal};
use sea_orm::DatabaseConnection;
use serde_json::Value;

use crate::auth;
use crate::clock::FixedClock;
use crate::database::{DatabaseError, TestDatabase};
use crate::fixtures::{Fixtures, SeedError, Who, seed};
use crate::http::{HttpError, TestHttp, with_test_principal};
use crate::snapshots;

#[derive(Default)]
pub struct Builder {
    modules: Vec<&'static ModuleDef>,
    products: Vec<&'static ProductManifest>,
    clock: Option<DateTime<Utc>>,
    sections: Vec<(String, Value)>,
}

pub struct Harness {
    clock: Arc<FixedClock>,
    kernel: Mutex<Kernel>,
    dispatcher: Arc<Dispatcher>,
    readiness: Arc<Readiness>,
    fixtures: Fixtures,
    storage: Storage,
    database: TestDatabase,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Database(#[from] DatabaseError),
    #[error("the composition did not boot: {0}")]
    Boot(#[from] BootError),
    #[error("the dispatcher could not compile a schema: {0}")]
    Schema(#[from] SchemaError),
    #[error(transparent)]
    Seed(#[from] SeedError),
    #[error(transparent)]
    Http(#[from] HttpError),
    #[error("the kernel did not stop cleanly: {0}")]
    Stop(#[from] keel_kernel::loader::StopError),
    #[error(transparent)]
    Storage(#[from] keel_kernel::StorageError),
    #[error("the data key of the test composition could not be generated: {0}")]
    Key(#[from] KeyError),
}

impl Harness {
    #[allow(clippy::new_ret_no_self)]
    pub fn new() -> Builder {
        Builder::default()
    }
}

impl Builder {
    pub fn module(mut self, def: &'static ModuleDef) -> Self {
        self.modules.push(def);
        self
    }

    pub fn product(mut self, bundle: &'static ProductManifest) -> Self {
        self.products.push(bundle);
        self
    }

    pub fn clock(mut self, at: DateTime<Utc>) -> Self {
        self.clock = Some(at);
        self
    }

    pub fn section(mut self, key: &str, value: Value) -> Self {
        self.sections.push((key.to_owned(), value));
        self
    }

    pub async fn boot(self) -> Harness {
        match self.try_boot().await {
            Ok(harness) => harness,
            Err(error) => panic!("{error}"),
        }
    }

    pub async fn try_boot(self) -> Result<Harness, Error> {
        let database = TestDatabase::try_create().await?;
        let storage = database.try_storage().await?;
        let clock = Arc::new(match self.clock {
            Some(at) => FixedClock::at(at),
            None => FixedClock::frozen_now(),
        });
        let mut composition = Composition::new()
            .clock(clock.clone())
            .key(Key::generate()?)
            .module(&auth::MODULE);
        for module in self.modules {
            composition = composition.module(module);
        }
        for bundle in self.products {
            composition = composition.product(bundle);
        }
        for (key, value) in self.sections {
            composition = composition.section(&key, value);
        }
        let kernel = Kernel::boot(composition, &storage).await?;
        let readiness = Readiness::new();
        readiness.migrated();
        let dispatcher = match Dispatcher::new(kernel.registry().clone(), storage.db().clone()) {
            Ok(dispatcher) => Arc::new(dispatcher),
            Err(error) => {
                stop_quietly(kernel).await;
                return Err(error.into());
            }
        };
        let fixtures = match seed(&dispatcher).await {
            Ok(fixtures) => fixtures,
            Err(error) => {
                stop_quietly(kernel).await;
                return Err(error.into());
            }
        };
        Ok(Harness {
            clock,
            kernel: Mutex::new(kernel),
            dispatcher,
            readiness,
            fixtures,
            storage,
            database,
        })
    }
}

async fn stop_quietly(kernel: Kernel) {
    if let Err(error) = kernel.stop().await {
        tracing::warn!(error = %error, "stopping the kernel after a failed boot");
    }
}

impl Harness {
    pub fn fixtures(&self) -> &Fixtures {
        &self.fixtures
    }

    pub fn principal(&self, who: Who) -> &Principal {
        self.fixtures.principal(who)
    }

    pub fn clock(&self) -> &FixedClock {
        &self.clock
    }

    pub fn registry(&self) -> &Arc<Registry> {
        self.dispatcher.registry()
    }

    pub fn dispatcher(&self) -> &Arc<Dispatcher> {
        &self.dispatcher
    }

    pub fn modules(&self) -> Vec<String> {
        self.registry()
            .modules()
            .iter()
            .map(|module| module.id.clone())
            .collect()
    }

    pub fn db(&self) -> &DatabaseConnection {
        self.storage.db()
    }

    pub fn database(&self) -> &TestDatabase {
        &self.database
    }

    pub async fn call_as(&self, level: Level, func: &str, params: Value) -> Reply {
        self.call(Who::at(level), func, params).await
    }

    pub async fn call(&self, who: Who, func: &str, params: Value) -> Reply {
        self.call_as_principal(self.fixtures.principal(who), func, params)
            .await
    }

    pub async fn call_as_principal(
        &self,
        principal: &Principal,
        func: &str,
        params: Value,
    ) -> Reply {
        self.call_with(principal, func, params, CallOptions::default())
            .await
    }

    pub async fn call_with(
        &self,
        principal: &Principal,
        func: &str,
        params: Value,
        options: CallOptions,
    ) -> Reply {
        self.dispatcher
            .call(principal.clone(), func, params, options)
            .await
    }

    pub fn snapshot_descriptors(&self, level: Level, dir: impl AsRef<Path>) -> Vec<String> {
        snapshots::snapshot_descriptors(
            self.registry(),
            self.fixtures.at(level),
            level,
            dir.as_ref(),
        )
    }

    pub fn router(&self) -> Router {
        let http = Http::new(
            self.db().clone(),
            self.readiness.clone(),
            HttpOptions::default(),
        );
        if let Err(error) = http.attach(self.dispatcher.clone()) {
            panic!("{error}");
        }
        with_test_principal(http.router())
    }

    pub async fn http(&self) -> TestHttp {
        match TestHttp::bind(self.router()).await {
            Ok(server) => server,
            Err(error) => panic!("{error}"),
        }
    }

    pub async fn finish(self) {
        if let Err(error) = self.try_finish().await {
            panic!("{error}");
        }
    }

    pub async fn try_finish(self) -> Result<(), Error> {
        let database = self.try_stop().await?;
        database.try_finish().await?;
        Ok(())
    }

    pub async fn stop(self) -> TestDatabase {
        match self.try_stop().await {
            Ok(database) => database,
            Err(error) => panic!("{error}"),
        }
    }

    pub async fn try_stop(self) -> Result<TestDatabase, Error> {
        let Harness {
            kernel,
            dispatcher,
            readiness,
            storage,
            database,
            ..
        } = self;
        readiness.draining();
        drop(dispatcher);
        let kernel = kernel.into_inner().unwrap_or_else(PoisonError::into_inner);
        let mut first: Option<Error> = None;
        if let Err(error) = kernel.stop().await {
            first.get_or_insert(error.into());
        }
        if let Err(error) = storage.close().await {
            first.get_or_insert(error.into());
        }
        match first {
            Some(error) => Err(error),
            None => Ok(database),
        }
    }
}

impl fmt::Debug for Harness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Harness")
            .field("database", &self.database.name())
            .field("modules", &self.modules())
            .field("products", &self.registry().product_ids())
            .finish()
    }
}
