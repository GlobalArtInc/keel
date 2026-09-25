mod agent;
pub mod auth;
pub mod clock;
pub mod database;
pub mod events;
pub mod fixtures;
pub mod harness;
pub mod http;
pub mod invariants;
pub mod license;
pub mod snapshots;
pub mod socket;

pub use keel_kernel::http::SESSION_COOKIE;
pub use keel_kernel::secret::Key;
pub use keel_kernel::{CallOptions, Clock, Dispatcher, HttpOptions, Registry, Reply};
pub use keel_types::{CallError, ErrorCode};
pub use tokio;

pub use auth::PRINCIPAL_HEADER;
pub use clock::FixedClock;
pub use database::{DatabaseError, DatabaseHandle, Server, TestDatabase};
pub use events::{EventStream, SseFrame};
pub use fixtures::{Accounts, Fixtures, Principals, Who};
pub use harness::{AttachedDatabase, Builder, Error, Harness};
pub use http::{Answer, EVENTS_PATH, TestHttp, UPLOAD_BOUNDARY, UploadPart, multipart_body};
pub use invariants::Fixture;
pub use socket::{CtlOutput, SocketTestError, TestSocket};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
