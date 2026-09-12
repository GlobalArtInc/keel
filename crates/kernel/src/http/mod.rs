mod auth;
mod boot;
mod call;
mod descriptors;
mod i18n;
mod live;
mod reply;

use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use axum::{Router, middleware};
use keel_sdk::KeelError;
use keel_types::Principal;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::dispatcher::Dispatcher;
use crate::registry::Registry;

use live::Live;

pub use auth::SESSION_COOKIE;
pub use i18n::{Catalogue, catalogue};
pub use reply::{IDEMPOTENCY_KEY, IDEMPOTENT_REPLAY, META_VERSION, REQUEST_ID};

pub(crate) use descriptors::Projection;
pub(crate) use reply::acceptable_request_id;

pub const API_PREFIX: &str = "/api/v1";
pub const BODY_LIMIT: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpOptions {
    pub body_limit: usize,
}

impl Default for HttpOptions {
    fn default() -> Self {
        Self {
            body_limit: BODY_LIMIT,
        }
    }
}

#[derive(Debug, Default)]
pub struct Readiness {
    migrated: AtomicBool,
    draining: AtomicBool,
}

impl Readiness {
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    pub fn migrated(&self) {
        self.migrated.store(true, Ordering::SeqCst);
    }

    pub fn draining(&self) {
        self.draining.store(true, Ordering::SeqCst);
    }

    pub(crate) fn is_migrated(&self) -> bool {
        self.migrated.load(Ordering::SeqCst)
    }

    pub(crate) fn is_draining(&self) -> bool {
        self.draining.load(Ordering::SeqCst)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("could not bind {addr}: {source}")]
    Bind {
        addr: SocketAddr,
        #[source]
        source: io::Error,
    },
    #[error("the listener has no local address: {0}")]
    LocalAddr(#[source] io::Error),
    #[error("serving HTTP failed: {0}")]
    Serve(#[source] io::Error),
    #[error("the kernel is already attached to the transport")]
    Attached,
}

#[derive(Clone)]
pub struct Http {
    state: Arc<Shared>,
}

pub(crate) struct Shared {
    db: DatabaseConnection,
    readiness: Arc<Readiness>,
    options: HttpOptions,
    live: OnceLock<Live>,
}

pub struct Served {
    addr: SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<Result<(), HttpError>>,
}

impl Http {
    pub fn new(db: DatabaseConnection, readiness: Arc<Readiness>, options: HttpOptions) -> Self {
        Self {
            state: Arc::new(Shared {
                db,
                readiness,
                options,
                live: OnceLock::new(),
            }),
        }
    }

    pub fn attach(&self, dispatcher: Arc<Dispatcher>) -> Result<(), HttpError> {
        self.state
            .live
            .set(Live::new(dispatcher))
            .map_err(|_| HttpError::Attached)
    }

    pub fn router(&self) -> Router {
        let api = Router::new()
            .route("/boot", get(boot::serve))
            .route("/functions", get(descriptors::list))
            .route("/functions/{name}", get(descriptors::one))
            .route("/call/{func}", get(call::get).post(call::post))
            .route("/i18n", get(i18n::serve))
            .layer(middleware::from_fn_with_state(
                self.state.clone(),
                auth::authenticate,
            ));
        Router::new()
            .route("/healthz", get(healthz))
            .route("/readyz", get(readyz))
            .nest(API_PREFIX, api)
            .fallback(fallback)
            .layer(middleware::from_fn(reply::request_id))
            .with_state(self.state.clone())
    }

    pub async fn serve(
        self,
        listener: TcpListener,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), HttpError> {
        let addr = listener.local_addr().map_err(HttpError::LocalAddr)?;
        tracing::info!(%addr, "http listening");
        axum::serve(listener, self.router())
            .with_graceful_shutdown(shutdown)
            .await
            .map_err(HttpError::Serve)
    }

    pub async fn bind(self, addr: SocketAddr) -> Result<Served, HttpError> {
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|source| HttpError::Bind { addr, source })?;
        let addr = listener.local_addr().map_err(HttpError::LocalAddr)?;
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(self.serve(listener, async move {
            let _ = stopped.await;
        }));
        Ok(Served {
            addr,
            stop: Some(stop),
            task,
        })
    }

    pub async fn bind_local(self) -> Result<Served, HttpError> {
        self.bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .await
    }
}

impl Served {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    pub async fn stop(mut self) -> Result<(), HttpError> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        match self.task.await {
            Ok(outcome) => outcome,
            Err(error) => Err(HttpError::Serve(io::Error::other(error))),
        }
    }
}

impl Shared {
    async fn meta_version(
        &self,
        registry: &Registry,
        principal: &Principal,
    ) -> Result<String, KeelError> {
        crate::meta::version(registry, &self.db, principal).await
    }
}

async fn healthz() -> Response {
    Json(json!({ "status": "ok" })).into_response()
}

async fn readyz(State(state): State<Arc<Shared>>) -> Response {
    let status = if !state.readiness.is_migrated() {
        "migrating"
    } else if state.readiness.is_draining() {
        "draining"
    } else {
        match state
            .db
            .execute(Statement::from_string(DbBackend::Postgres, "SELECT 1"))
            .await
        {
            Ok(_) => return Json(json!({ "status": "ready" })).into_response(),
            Err(error) => {
                tracing::warn!(error = %error, "readiness: the database did not answer");
                "database"
            }
        }
    };
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "status": status })),
    )
        .into_response()
}

async fn fallback(request: Request) -> Response {
    let request_id = reply::request_id_in(request.extensions());
    reply::error(
        KeelError::not_found("keel.error.route_not_found").param("path", request.uri().path()),
        &request_id,
    )
}
