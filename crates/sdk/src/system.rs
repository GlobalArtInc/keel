use std::net::IpAddr;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use keel_types::{CallError, Id, Mode, Principal};
use serde::Serialize;
use serde_json::Value;

use crate::call::Call;
use crate::db::Db;
use crate::error::Result;
use crate::secret::Secret;

#[derive(Clone, Debug)]
pub enum Credential {
    Bearer(Secret),
    Session(Secret),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Client {
    pub ip: Option<IpAddr>,
    pub user_agent: Option<String>,
    pub session: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Authenticated {
    pub principal: Principal,
    pub session: Option<String>,
}

#[derive(Clone, Debug)]
pub enum SessionChange {
    Set(Secret),
    Clear,
}

#[async_trait]
pub trait Authenticator: Send + Sync {
    async fn authenticate(
        &self,
        credential: &Credential,
        client: &Client,
        db: &Db,
    ) -> Result<Option<Authenticated>>;
}

#[async_trait]
pub trait PrincipalSource: Send + Sync {
    async fn by_id(&self, id: Id, db: &Db) -> Result<Option<Principal>>;
    async fn by_login(&self, login: &str, db: &Db) -> Result<Option<Principal>>;
}

pub trait SecretCipher: Send + Sync {
    fn seal(&self, plaintext: &[u8]) -> Result<String>;
    fn open(&self, sealed: &str) -> Result<Vec<u8>>;
}

#[async_trait]
pub trait SecondFactorSource: Send + Sync {
    async fn enrolled(&self, principal: Id, db: &Db) -> Result<bool>;
}

pub trait CallExt {
    fn client(&self) -> Option<&Client>;
    fn session(&self, change: SessionChange);
}

impl<P> CallExt for Call<P> {
    fn client(&self) -> Option<&Client> {
        self.client_slot()
    }

    fn session(&self, change: SessionChange) {
        self.record_session(change);
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEvent {
    pub request_id: String,
    pub function: String,
    pub module: String,
    pub mode: Mode,
    pub principal: Principal,
    pub params: Value,
    pub outcome: AuditOutcome,
    pub at: DateTime<Utc>,
    pub elapsed: Duration,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "result")]
pub enum AuditOutcome {
    Committed,
    Failed { error: CallError },
}

impl AuditOutcome {
    pub fn is_committed(&self) -> bool {
        matches!(self, AuditOutcome::Committed)
    }
}

pub trait AuditSink: Send + Sync {
    fn record(&self, event: AuditEvent);
}

#[cfg(test)]
mod tests {
    use keel_types::Level;

    use super::*;
    use crate::call::CallBuilder;

    #[test]
    fn a_call_carries_the_client_of_its_request_and_records_one_session_change() {
        let client = Client {
            ip: Some("127.0.0.1".parse().expect("an address")),
            user_agent: Some("probe/1".into()),
            session: Some("abc".into()),
        };
        let handle = CallBuilder::new(crate::tests::principal(Level::User), "req-1")
            .client(Some(client.clone()))
            .read(sea_orm::DatabaseConnection::Disconnected);
        let call = handle.call(());
        assert_eq!(call.client(), Some(&client));
        assert!(handle.take_session_change().is_none());
        call.session(SessionChange::Set(Secret::new("ks_1")));
        call.session(SessionChange::Clear);
        assert!(matches!(
            handle.take_session_change(),
            Some(SessionChange::Clear)
        ));
        assert!(handle.take_session_change().is_none());
        let bare = crate::tests::read_handle();
        assert!(bare.call(()).client().is_none());
        assert_eq!(
            format!("{:?}", SessionChange::Set(Secret::new("ks_1"))),
            "Set(Secret(***))"
        );
    }
}
