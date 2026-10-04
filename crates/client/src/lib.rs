mod call;
mod client;
mod credential;
mod error;
mod feed;
mod tls;

pub use call::{Answer, Call};
pub use client::{Builder, Client, Request, Response, Streaming};
pub use credential::{Credential, OnBehalf};
pub use error::{Error, Refusal};
pub use feed::{Feed, FeedPage};
pub use http;
pub use keel_types as types;
pub use tls::{Pin, Trust};

use std::time::Duration;

use http::HeaderName;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const API_PREFIX: &str = "/api/v1";
pub const CALL_PATH: &str = "/api/v1/call";
pub const INSTANCE_PATH: &str = "/api/v1/instance";
pub const FEED_PATH: &str = "/api/v1/x/peer/feed";
pub const IDEMPOTENCY_KEY: HeaderName = HeaderName::from_static("idempotency-key");
pub const IDEMPOTENT_REPLAY: HeaderName = HeaderName::from_static("x-keel-idempotent-replay");
pub const ON_BEHALF_OF: HeaderName = HeaderName::from_static("x-keel-on-behalf-of");
pub const REQUEST_ID: HeaderName = HeaderName::from_static("x-keel-request-id");
pub const META_VERSION: HeaderName = HeaderName::from_static("x-keel-meta-version");
pub const SESSION_COOKIE: &str = "__Host-keel_session";
pub const TOKEN_PREFIX: &str = "kat_";
pub const PEER_PREFIX: &str = "kpt_";
pub const SESSION_PREFIX: &str = "ks_";
pub const TIMEOUT: Duration = Duration::from_secs(30);
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const ATTEMPTS: u32 = 3;
pub const BACKOFF: Duration = Duration::from_millis(250);
pub const BACKOFF_MAX: Duration = Duration::from_secs(5);
pub const ANSWER_LIMIT: usize = 8 << 20;
