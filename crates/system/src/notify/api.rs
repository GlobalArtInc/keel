use std::fmt;
use std::future::Future;
use std::str::FromStr;
use std::sync::Arc;

use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::schemars::JsonSchema;
use keel_sdk::serde::{Deserialize, Serialize};
use keel_sdk::serde_json::{self, Value};
use keel_sdk::system::{Caller, CallerOptions};
use keel_sdk::types::NotificationLevel;
use keel_sdk::{Db, Id, KeelError, Result};

use super::super::cron::api::owner;
use super::functions::internal::{MESSAGE_CREATE, PROBLEM_CLEAR, PROBLEM_RAISE};

pub use super::channels::{Channel, ChannelKind, EmailInput, EmailView, Security};
pub use super::templates::Rendered;
pub use keel_sdk::{MailTemplate, MailText};

pub const SOURCE_PROBLEM: &str = "problem";
pub const SOURCE_TEST: &str = "test";
pub const KEY_PROBLEM_RAISED: &str = "notify.problem.raised";
pub const KEY_PROBLEM_CLEARED: &str = "notify.problem.cleared";
pub const KEY_TEST: &str = "notify.test";
pub const EVENT_MESSAGE_CREATED: &str = "notify.message.created";
pub const EVENT_PROBLEM_RAISED: &str = "notify.problem.raised";
pub const EVENT_PROBLEM_CLEARED: &str = "notify.problem.cleared";
pub const FRAME_KIND: &str = "notification";
pub const ERROR_SEVERITY: &str = "notify.error.severity";
pub const ERROR_LEVEL: &str = "notify.error.level";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "lowercase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "lowercase")]
pub enum Severity {
    Warning,
    Critical,
}

impl Severity {
    pub const ALL: [Severity; 2] = [Severity::Warning, Severity::Critical];

    pub const fn as_str(self) -> &'static str {
        match self {
            Severity::Warning => "warning",
            Severity::Critical => "critical",
        }
    }

    pub const fn level(self) -> NotificationLevel {
        match self {
            Severity::Warning => NotificationLevel::Warning,
            Severity::Critical => NotificationLevel::Error,
        }
    }
}

impl FromStr for Severity {
    type Err = ();

    fn from_str(text: &str) -> std::result::Result<Self, ()> {
        Severity::ALL
            .into_iter()
            .find(|severity| severity.as_str() == text)
            .ok_or(())
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Raise {
    #[schemars(length(min = 1, max = 128))]
    pub key: String,
    #[schemars(length(min = 1, max = 256))]
    pub subject: String,
    pub severity: Severity,
    #[serde(default)]
    pub params: Value,
    #[serde(default)]
    pub account: Option<Id>,
    #[serde(default)]
    #[schemars(length(max = 128))]
    pub source: String,
}

impl Raise {
    pub fn new(key: &str, subject: &str, severity: Severity) -> Self {
        Self {
            key: key.to_owned(),
            subject: subject.to_owned(),
            severity,
            params: Value::Object(Default::default()),
            account: None,
            source: String::new(),
        }
    }

    pub fn params(mut self, params: impl Serialize) -> Result<Self> {
        self.params = serde_json::to_value(params)?;
        Ok(self)
    }

    pub fn account(mut self, account: Id) -> Self {
        self.account = Some(account);
        self
    }

    pub fn source(mut self, source: &str) -> Self {
        self.source = source.to_owned();
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "lowercase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "lowercase")]
pub enum Recipients {
    Admins,
    Principal(Id),
    Account(Id),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub struct Problem {
    pub id: Id,
    pub key: String,
    pub subject: String,
    pub severity: Severity,
    pub params: Value,
    pub account_id: Option<Id>,
    pub source: String,
    pub count: u64,
    pub open: bool,
    pub raised_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub cleared_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Cleared {
    pub cleared: bool,
    pub problem: Option<Problem>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Created {
    pub ids: Vec<Id>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Message {
    pub recipients: Vec<Recipients>,
    #[schemars(length(min = 1, max = 128))]
    pub key: String,
    #[serde(default)]
    pub params: Value,
    pub level: NotificationLevel,
    #[schemars(length(min = 1, max = 64))]
    pub source: String,
    #[serde(default)]
    pub problem: Option<Id>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Pair {
    #[schemars(length(min = 1, max = 128))]
    pub key: String,
    #[schemars(length(min = 1, max = 256))]
    pub subject: String,
}

pub struct Problems;

impl Problems {
    pub async fn raise(caller: &dyn Caller, raise: Raise) -> Result<Problem> {
        let answered = caller
            .call(
                owner(),
                PROBLEM_RAISE,
                serde_json::to_value(raise)?,
                CallerOptions::default(),
            )
            .await?;
        Ok(serde_json::from_value(answered)?)
    }

    pub async fn clear(caller: &dyn Caller, key: &str, subject: &str) -> Result<Cleared> {
        let answered = caller
            .call(
                owner(),
                PROBLEM_CLEAR,
                serde_json::to_value(Pair {
                    key: key.to_owned(),
                    subject: subject.to_owned(),
                })?,
                CallerOptions::default(),
            )
            .await?;
        Ok(serde_json::from_value(answered)?)
    }
}

pub struct Notifier;

impl Notifier {
    pub async fn notify(
        caller: &dyn Caller,
        recipients: Recipients,
        key: &str,
        params: Value,
        level: NotificationLevel,
    ) -> Result<Vec<Id>> {
        Self::send(
            caller,
            Message {
                recipients: vec![recipients],
                key: key.to_owned(),
                params,
                level,
                source: key
                    .split_once('.')
                    .map_or(key, |(module, _)| module)
                    .to_owned(),
                problem: None,
            },
        )
        .await
    }

    pub async fn send(caller: &dyn Caller, message: Message) -> Result<Vec<Id>> {
        let answered = caller
            .call(
                owner(),
                MESSAGE_CREATE,
                serde_json::to_value(message)?,
                CallerOptions::default(),
            )
            .await?;
        let created: Created = serde_json::from_value(answered)?;
        Ok(created.ids)
    }
}

type PollFuture = std::pin::Pin<Box<dyn Future<Output = Result<Vec<Raise>>> + Send + 'static>>;
type Poll = Arc<dyn Fn(Db) -> PollFuture + Send + Sync>;

#[derive(Clone)]
pub struct ProblemSource {
    id: String,
    poll: Poll,
}

impl ProblemSource {
    pub fn new<F, Fut>(id: &str, poll: F) -> Self
    where
        F: Fn(Db) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Vec<Raise>>> + Send + 'static,
    {
        Self {
            id: id.to_owned(),
            poll: Arc::new(move |db| Box::pin(poll(db))),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn module(&self) -> &str {
        self.id
            .split_once('.')
            .map_or(&self.id, |(module, _)| module)
    }

    pub async fn poll(&self, db: Db) -> Result<Vec<Raise>> {
        (self.poll)(db).await
    }
}

impl fmt::Debug for ProblemSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProblemSource")
            .field("id", &self.id)
            .finish()
    }
}

pub fn severity_of(text: &str, problem: &str) -> Result<Severity> {
    text.parse().map_err(|()| {
        KeelError::internal(ERROR_SEVERITY)
            .param("id", problem)
            .param("severity", text)
    })
}

pub fn level_of(text: &str, message: &str) -> Result<NotificationLevel> {
    text.parse().map_err(|_| {
        KeelError::internal(ERROR_LEVEL)
            .param("id", message)
            .param("level", text)
    })
}

#[cfg(test)]
mod tests {
    use keel_sdk::serde_json::json;

    use super::*;

    #[test]
    fn a_severity_round_trips_and_maps_to_the_notification_level() {
        for severity in Severity::ALL {
            assert_eq!(severity.as_str().parse::<Severity>(), Ok(severity));
            assert_eq!(json!(severity), json!(severity.as_str()));
            assert_eq!(severity.to_string(), severity.as_str());
        }
        assert_eq!(Severity::Warning.level(), NotificationLevel::Warning);
        assert_eq!(Severity::Critical.level(), NotificationLevel::Error);
        assert_eq!("loud".parse::<Severity>(), Err(()));
        let error = severity_of("loud", "01J").expect_err("loud is not a severity");
        assert_eq!(error.key(), ERROR_SEVERITY);
        assert_eq!(error.params()["severity"], json!("loud"));
        let error = level_of("loud", "01J").expect_err("loud is not a level");
        assert_eq!(error.key(), ERROR_LEVEL);
    }

    #[test]
    fn a_raise_builds_with_defaults_and_recipients_serialise_by_their_kind() {
        let account = Id::generate();
        let raise = Raise::new("note.overdue", &account.to_string(), Severity::Warning)
            .params(json!({ "count": 3 }))
            .expect("params serialise")
            .account(account)
            .source("note.overdue");
        assert_eq!(raise.key, "note.overdue");
        assert_eq!(raise.params, json!({ "count": 3 }));
        assert_eq!(raise.account, Some(account));
        assert_eq!(raise.source, "note.overdue");
        let bare = Raise::new("disk.full", "/", Severity::Critical);
        assert_eq!(bare.params, json!({}));
        assert!(bare.account.is_none());
        assert_eq!(bare.source, "");
        assert_eq!(json!(Recipients::Admins), json!("admins"));
        assert_eq!(
            json!(Recipients::Account(account)),
            json!({ "account": account.to_string() })
        );
        let parsed: Vec<Recipients> = serde_json::from_value(json!([
            "admins",
            { "principal": account.to_string() }
        ]))
        .expect("recipients parse");
        assert_eq!(parsed, [Recipients::Admins, Recipients::Principal(account)]);
        let source = ProblemSource::new("note.overdue", |_db| async { Ok(Vec::new()) });
        assert_eq!(source.id(), "note.overdue");
        assert_eq!(source.module(), "note");
        assert_eq!(
            format!("{source:?}"),
            "ProblemSource { id: \"note.overdue\" }"
        );
    }
}
