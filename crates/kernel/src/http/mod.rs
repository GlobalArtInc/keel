mod assets;
mod auth;
mod boot;
mod brand;
mod call;
mod cookie;
mod descriptors;
mod events;
mod i18n;
mod live;
mod proxy;
mod raw;
mod reply;

use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use axum::{Router, middleware};
use axum_server::tls_rustls::RustlsConfig;
use ipnet::IpNet;
use keel_sdk::KeelError;
use keel_types::Principal;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;

use crate::dispatcher::Dispatcher;
use crate::registry::Registry;
use crate::tls::TlsMaterial;

use events::Streams;
use live::Live;

pub use assets::Frontend;
pub use auth::{GUEST_LOGIN, SESSION_COOKIE, guest};
pub use descriptors::Projection;
pub use events::{DATA_INVALIDATE, LAST_EVENT_ID, META_INVALIDATE, SSE_REPLAY_WINDOW, SSE_RETRY};
pub use i18n::Catalogue;
pub use raw::{BODY_NOT_MULTIPART_KEY, PART_REPEATED_KEY, PART_UNKNOWN_KEY};
pub use reply::{IDEMPOTENCY_KEY, IDEMPOTENT_REPLAY, META_VERSION, REQUEST_ID};

pub(crate) use i18n::catalogue;
pub(crate) use reply::acceptable_request_id;

pub(crate) const API_PREFIX: &str = "/api/v1";
pub const BODY_LIMIT: usize = 1024 * 1024;
pub const TLS_SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
pub struct HttpOptions {
    pub body_limit: usize,
    pub frontend: Option<Frontend>,
    pub sse_replay_window: u64,
    pub trusted_proxies: Vec<IpNet>,
}

impl Default for HttpOptions {
    fn default() -> Self {
        Self {
            body_limit: BODY_LIMIT,
            frontend: None,
            sse_replay_window: SSE_REPLAY_WINDOW,
            trusted_proxies: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    Http,
    Https,
}

impl Scheme {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }
}

#[derive(Debug)]
pub struct Readiness {
    migrated: AtomicBool,
    draining: watch::Sender<bool>,
}

impl Default for Readiness {
    fn default() -> Self {
        Self {
            migrated: AtomicBool::new(false),
            draining: watch::Sender::new(false),
        }
    }
}

impl Readiness {
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    pub fn migrated(&self) {
        self.migrated.store(true, Ordering::SeqCst);
    }

    pub fn draining(&self) {
        self.draining.send_replace(true);
    }

    pub(crate) fn is_migrated(&self) -> bool {
        self.migrated.load(Ordering::SeqCst)
    }

    pub(crate) fn is_draining(&self) -> bool {
        *self.draining.borrow()
    }

    pub(crate) fn drained(&self) -> watch::Receiver<bool> {
        self.draining.subscribe()
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
    #[error(transparent)]
    Tls(crate::tls::TlsError),
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
    streams: Arc<Streams>,
}

pub struct Served {
    addr: SocketAddr,
    scheme: Scheme,
    stop: Option<Stop>,
    task: JoinHandle<Result<(), HttpError>>,
}

enum Stop {
    Plain(oneshot::Sender<()>),
    Tls(axum_server::Handle<SocketAddr>),
}

impl Http {
    pub fn new(db: DatabaseConnection, readiness: Arc<Readiness>, options: HttpOptions) -> Self {
        let streams = Arc::new(Streams::new(readiness.clone()));
        Self {
            state: Arc::new(Shared {
                db,
                readiness,
                options,
                live: OnceLock::new(),
                streams,
            }),
        }
    }

    pub fn open_streams(&self) -> usize {
        self.state.streams.open()
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
            .route("/events", get(events::serve))
            .route(
                "/x/{module}/{name}",
                get(raw::get)
                    .post(raw::post)
                    .layer(DefaultBodyLimit::disable()),
            )
            .layer(middleware::from_fn_with_state(
                self.state.clone(),
                auth::authenticate,
            ));
        Router::new()
            .route("/healthz", get(healthz))
            .route("/readyz", get(readyz))
            .route("/brand/logo.svg", get(brand::effective))
            .route("/brand/{id}/logo.svg", get(brand::product))
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
        axum::serve(
            listener,
            self.router()
                .into_make_service_with_connect_info::<SocketAddr>(),
        )
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
            scheme: Scheme::Http,
            stop: Some(Stop::Plain(stop)),
            task,
        })
    }

    pub async fn bind_local(self) -> Result<Served, HttpError> {
        self.bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .await
    }

    pub async fn bind_tls(self, addr: SocketAddr, tls: &TlsMaterial) -> Result<Served, HttpError> {
        let config = tls.server_config().map_err(HttpError::Tls)?;
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|source| HttpError::Bind { addr, source })?;
        let addr = listener.local_addr().map_err(HttpError::LocalAddr)?;
        let listener = listener.into_std().map_err(HttpError::LocalAddr)?;
        let handle = axum_server::Handle::new();
        let server = axum_server::from_tcp_rustls(listener, RustlsConfig::from_config(config))
            .map_err(HttpError::Serve)?
            .handle(handle.clone());
        let service = self
            .router()
            .into_make_service_with_connect_info::<SocketAddr>();
        let task = tokio::spawn(async move {
            tracing::info!(%addr, "https listening");
            server.serve(service).await.map_err(HttpError::Serve)
        });
        Ok(Served {
            addr,
            scheme: Scheme::Https,
            stop: Some(Stop::Tls(handle)),
            task,
        })
    }
}

impl Served {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn scheme(&self) -> Scheme {
        self.scheme
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}://{}{path}", self.scheme.as_str(), self.addr)
    }

    pub async fn stop(mut self) -> Result<(), HttpError> {
        match self.stop.take() {
            Some(Stop::Plain(stop)) => {
                let _ = stop.send(());
            }
            Some(Stop::Tls(handle)) => handle.graceful_shutdown(Some(TLS_SHUTDOWN_GRACE)),
            None => {}
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

async fn fallback(State(state): State<Arc<Shared>>, request: Request) -> Response {
    if let Some(frontend) = &state.options.frontend
        && let Some(response) =
            frontend.answer(request.method(), request.uri().path(), request.headers())
    {
        return response;
    }
    let request_id = reply::request_id_in(request.extensions());
    reply::error(
        KeelError::not_found("keel.error.route_not_found").param("path", request.uri().path()),
        &request_id,
    )
}
