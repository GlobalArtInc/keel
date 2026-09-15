use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use axum::Router;
use chrono::{DateTime, Utc};
use keel_kernel::dispatcher::{LiveBus, SchemaError};
use keel_kernel::loader::BootError;
use keel_kernel::secret::{Key, KeyError};
use keel_kernel::{
    CallOptions, Composition, Dispatcher, Http, HttpOptions, Kernel, Readiness, Registry, Reply,
    Storage,
};
use keel_sdk::system::Frame;
use keel_sdk::{ModuleDef, ProductManifest};
use keel_types::{Level, Principal};
use sea_orm::DatabaseConnection;
use serde_json::Value;
use tempfile::TempDir;
use tokio::sync::broadcast;

use crate::auth;
use crate::clock::FixedClock;
use crate::database::{DatabaseError, DatabaseHandle, TestDatabase};
use crate::fixtures::{Fixtures, SeedError, Who, recover, seed};
use crate::http::{HttpError, TestHttp, with_test_principal};
use crate::snapshots;

#[derive(Default)]
pub struct Builder {
    modules: Vec<&'static ModuleDef>,
    products: Vec<&'static ProductManifest>,
    clock: Option<DateTime<Utc>>,
    sections: Vec<(String, Value)>,
    data_dir: Option<PathBuf>,
    database: Option<AttachedDatabase>,
}

pub enum AttachedDatabase {
    Owned(TestDatabase),
    Shared(DatabaseHandle),
}

impl From<TestDatabase> for AttachedDatabase {
    fn from(database: TestDatabase) -> Self {
        AttachedDatabase::Owned(database)
    }
}

impl From<DatabaseHandle> for AttachedDatabase {
    fn from(handle: DatabaseHandle) -> Self {
        AttachedDatabase::Shared(handle)
    }
}

impl AttachedDatabase {
    fn handle(&self) -> &DatabaseHandle {
        match self {
            AttachedDatabase::Owned(database) => database,
            AttachedDatabase::Shared(handle) => handle,
        }
    }

    fn into_owned(self) -> Option<TestDatabase> {
        match self {
            AttachedDatabase::Owned(database) => Some(database),
            AttachedDatabase::Shared(_) => None,
        }
    }
}

enum DataDir {
    Temporary(TempDir),
    Given(PathBuf),
}

impl DataDir {
    fn path(&self) -> &Path {
        match self {
            DataDir::Temporary(dir) => dir.path(),
            DataDir::Given(path) => path,
        }
    }
}

pub struct Harness {
    clock: Arc<FixedClock>,
    kernel: Mutex<Kernel>,
    dispatcher: Arc<Dispatcher>,
    readiness: Arc<Readiness>,
    fixtures: Fixtures,
    storage: Storage,
    database: AttachedDatabase,
    data_dir: DataDir,
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
    #[error("the temporary data directory of the harness could not be created: {0}")]
    DataDir(#[source] std::io::Error),
    #[error("the harness shares the database {0} with the harness that created it")]
    Shared(String),
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

    pub fn data_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.data_dir = Some(path.into());
        self
    }

    pub fn database(mut self, database: impl Into<AttachedDatabase>) -> Self {
        self.database = Some(database.into());
        self
    }

    pub async fn boot(self) -> Harness {
        match self.try_boot().await {
            Ok(harness) => harness,
            Err(error) => panic!("{error}"),
        }
    }

    pub async fn try_boot(self) -> Result<Harness, Error> {
        let (database, seeded) = match self.database {
            Some(database) => (database, true),
            None => (
                AttachedDatabase::Owned(TestDatabase::try_create().await?),
                false,
            ),
        };
        let data_dir = match self.data_dir {
            Some(path) => DataDir::Given(path),
            None => DataDir::Temporary(tempfile::tempdir().map_err(Error::DataDir)?),
        };
        let storage = database.handle().try_storage().await?;
        let clock = Arc::new(match self.clock {
            Some(at) => FixedClock::at(at),
            None => FixedClock::frozen_now(),
        });
        let mut composition = Composition::new()
            .clock(clock.clone())
            .key(Key::generate()?)
            .data_dir(data_dir.path())
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
        dispatcher.fill_caller();
        let fixtures = if seeded {
            recover(&dispatcher).await
        } else {
            seed(&dispatcher).await
        };
        let fixtures = match fixtures {
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
            data_dir,
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

    pub fn database(&self) -> &DatabaseHandle {
        self.database.handle()
    }

    pub fn database_handle(&self) -> DatabaseHandle {
        self.database.handle().clone()
    }

    pub fn data_dir(&self) -> &Path {
        self.data_dir.path()
    }

    pub fn live_events(&self) -> broadcast::Receiver<Frame> {
        match self.registry().services().get::<LiveBus>() {
            Some(bus) => bus.subscribe(),
            None => panic!("the kernel provides no live events bus"),
        }
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
        if let Some(database) = self.try_release().await? {
            database.try_finish().await?;
        }
        Ok(())
    }

    pub async fn stop(self) -> TestDatabase {
        match self.try_stop().await {
            Ok(database) => database,
            Err(error) => panic!("{error}"),
        }
    }

    pub async fn try_stop(self) -> Result<TestDatabase, Error> {
        let name = self.database().name().to_owned();
        self.try_release().await?.ok_or(Error::Shared(name))
    }

    pub async fn release(self) {
        if let Err(error) = self.try_release().await {
            panic!("{error}");
        }
    }

    pub async fn try_release(self) -> Result<Option<TestDatabase>, Error> {
        let Harness {
            kernel,
            dispatcher,
            readiness,
            storage,
            database,
            data_dir,
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
        drop(data_dir);
        match first {
            Some(error) => Err(error),
            None => Ok(database.into_owned()),
        }
    }
}

impl fmt::Debug for Harness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Harness")
            .field("database", &self.database().name())
            .field("data_dir", &self.data_dir())
            .field("modules", &self.modules())
            .field("products", &self.registry().product_ids())
            .finish()
    }
}
