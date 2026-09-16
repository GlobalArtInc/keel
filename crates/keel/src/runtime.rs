use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use keel_kernel::http::Frontend;
use keel_kernel::loader::{Migrated, Sections};
use keel_kernel::socket::Serving;
use keel_kernel::tls;
use keel_kernel::{
    Config, Dispatcher, Http, HttpOptions, Instance, Kernel, Readiness, Served, Socket, Storage,
    TlsMaterial,
};
use keel_sdk::{ModuleDef, ProductManifest};

use crate::VERSION;
use crate::error::Error;

#[derive(Clone, Debug, Default)]
pub struct Runtime {
    pub(crate) modules: Vec<&'static ModuleDef>,
    pub(crate) products: Vec<&'static ProductManifest>,
    pub(crate) frontend: Option<Frontend>,
}

pub struct Running {
    storage: Storage,
    instance: Instance,
    readiness: Arc<Readiness>,
    kernel: Kernel,
    http: Served,
    tls: TlsMaterial,
    socket: Serving,
    products: Vec<String>,
}

impl Runtime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn module(mut self, def: &'static ModuleDef) -> Self {
        self.modules.push(def);
        self
    }

    pub fn product(mut self, bundle: &'static ProductManifest) -> Self {
        self.products.push(bundle);
        self
    }

    pub fn frontend(mut self, frontend: Frontend) -> Self {
        self.frontend = Some(frontend);
        self
    }

    pub async fn start(&self, config: &Config) -> Result<Running, Error> {
        if tls::install_provider() {
            tracing::debug!("the TLS provider is installed");
        }
        let storage = Storage::connect(config).await?;
        let outcome = self.start_with(config, &storage).await;
        match outcome {
            Ok(running) => Ok(running),
            Err(error) => {
                if let Err(close) = storage.close().await {
                    tracing::warn!(error = %close, "closing the pool after a failed start");
                }
                Err(error)
            }
        }
    }

    async fn start_with(&self, config: &Config, storage: &Storage) -> Result<Running, Error> {
        let enabled = self.enabled(storage).await?.ids;
        let instance = Instance::establish(&config.data_dir, storage.db()).await?;
        let tls = TlsMaterial::load(&config.tls, &config.data_dir, config.listen)?;
        tracing::info!(
            fingerprint = %tls.fingerprint(),
            cert = %tls.source().map(|source| source.cert_path().display().to_string()).unwrap_or_default(),
            "tls certificate"
        );
        let readiness = Readiness::new();
        let http = Http::new(
            storage.db().clone(),
            readiness.clone(),
            HttpOptions {
                frontend: self.frontend.clone(),
                trusted_proxies: config.trusted_proxies.clone(),
                ..HttpOptions::default()
            },
        );
        let served = http.clone().bind_tls(config.listen, &tls).await?;
        let booted = self.boot(config, storage, &enabled, &instance, &http).await;
        match booted {
            Ok((kernel, socket)) => {
                readiness.migrated();
                tracing::info!(
                    version = VERSION,
                    products = ?enabled,
                    https = %served.addr(),
                    socket = %socket.path().display(),
                    "keel is ready"
                );
                Ok(Running {
                    storage: storage.clone(),
                    instance,
                    readiness,
                    kernel,
                    http: served,
                    tls,
                    socket,
                    products: enabled,
                })
            }
            Err(error) => {
                if let Err(stop) = served.stop().await {
                    tracing::warn!(error = %stop, "stopping the listener after a failed start");
                }
                Err(error)
            }
        }
    }

    async fn boot(
        &self,
        config: &Config,
        storage: &Storage,
        enabled: &[String],
        instance: &Instance,
        http: &Http,
    ) -> Result<(Kernel, Serving), Error> {
        let composition = self
            .composition(enabled)?
            .key(instance.key().clone())
            .sections(Sections::new(&config.file, config.sections.clone()))
            .data_dir(config.data_dir.clone());
        let kernel = Kernel::boot(composition, storage).await?;
        let dispatcher = Arc::new(Dispatcher::new(
            kernel.registry().clone(),
            storage.db().clone(),
        )?);
        dispatcher.fill_caller();
        dispatcher.fill_outbox(instance.id());
        http.attach(dispatcher.clone())?;
        let socket = match Socket::new(dispatcher).start(&config.socket).await {
            Ok(socket) => socket,
            Err(error) => {
                if let Err(stop) = kernel.stop().await {
                    tracing::warn!(error = %stop, "stopping the modules after a failed start");
                }
                return Err(error.into());
            }
        };
        Ok((kernel, socket))
    }

    pub async fn serve(
        &self,
        config: &Config,
        shutdown: impl Future<Output = ()>,
    ) -> Result<(), Error> {
        let running = self.start(config).await?;
        shutdown.await;
        tracing::info!("shutdown requested");
        running.stop().await
    }

    pub async fn migrate(&self, config: &Config) -> Result<Migrated, Error> {
        let storage = Storage::connect(config).await?;
        let outcome = async {
            let enabled = self.enabled(&storage).await?;
            let graph = self.composition(&enabled.ids)?.resolve()?;
            let mut migrated = graph.migrate(&storage).await?;
            migrated.kernel.splice(0..0, enabled.kernel);
            Ok(migrated)
        }
        .await;
        storage.close().await?;
        outcome
    }
}

impl fmt::Debug for Running {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Running")
            .field("instance", &self.instance.id())
            .field("http", &self.http.addr())
            .field("socket", &self.socket.path())
            .field("products", &self.products)
            .field("modules", &self.kernel.graph().ids())
            .finish()
    }
}

impl Running {
    pub fn http_addr(&self) -> SocketAddr {
        self.http.addr()
    }

    pub fn url(&self, path: &str) -> String {
        self.http.url(path)
    }

    pub fn tls(&self) -> &TlsMaterial {
        &self.tls
    }

    pub fn socket(&self) -> &Path {
        self.socket.path()
    }

    pub fn products(&self) -> &[String] {
        &self.products
    }

    pub fn modules(&self) -> Vec<&'static str> {
        self.kernel.graph().ids()
    }

    pub async fn stop(self) -> Result<(), Error> {
        self.readiness.draining();
        let mut first: Option<Error> = None;
        if let Err(error) = self.socket.stop().await {
            first.get_or_insert(error.into());
        }
        if let Err(error) = self.http.stop().await {
            first.get_or_insert(error.into());
        }
        if let Err(error) = self.kernel.stop().await {
            first.get_or_insert(error.into());
        }
        if let Err(error) = self.storage.close().await {
            first.get_or_insert(error.into());
        }
        match first {
            Some(error) => Err(error),
            None => {
                tracing::info!("keel stopped");
                Ok(())
            }
        }
    }
}
