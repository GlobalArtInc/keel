mod accounts;
mod files;
mod system;

use keel_sdk::prelude::*;
use keel_sdk::serde::de::DeserializeOwned;
use keel_sdk::serde_json;

use crate::frame::{ERROR_ARGS, ERROR_FRAME, failure};

const INSTALLATION: &str = "keel";

pub use accounts::{Created, Group, Groups, User, Users};
pub use files::{Attributes, Dest, Entry, FileContent, Files, Template};
pub use system::{
    Cron, CronJob, Limits, PackageChange, PackageChanges, Packages, Quota, QuotaReport,
    ServiceState, ServiceStatus, Services, Usage, UserQuota,
};

#[async_trait]
pub trait HostExecutor: Send + Sync {
    async fn execute(&self, origin: &Origin, op: &str, args: Value) -> Result<Value>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    pub(crate) product: String,
    pub(crate) login: String,
    pub(crate) level: Level,
    pub(crate) request_id: String,
}

impl Origin {
    pub fn new(product: &str, principal: &Principal, request_id: &str) -> Self {
        Self {
            product: product.to_owned(),
            login: principal.login.clone(),
            level: principal.level,
            request_id: request_id.to_owned(),
        }
    }

    pub fn installation(product: &str, request_id: &str) -> Self {
        Self {
            product: product.to_owned(),
            login: INSTALLATION.to_owned(),
            level: Level::Service,
            request_id: request_id.to_owned(),
        }
    }
}

impl<'a> dyn HostExecutor + 'a {
    pub fn on(&'a self, origin: Origin) -> Host<'a> {
        Host {
            executor: self,
            origin,
        }
    }
}

pub struct Host<'a> {
    executor: &'a dyn HostExecutor,
    origin: Origin,
}

#[derive(Deserialize)]
#[serde(crate = "keel_sdk::serde")]
struct Output {
    output: String,
}

#[derive(Deserialize)]
#[serde(crate = "keel_sdk::serde")]
struct Captured {
    bytes: u64,
}

impl Host<'_> {
    pub fn users(&self) -> Users<'_> {
        Users::new(self)
    }

    pub fn groups(&self) -> Groups<'_> {
        Groups::new(self)
    }

    pub fn fs(&self) -> Files<'_> {
        Files::new(self)
    }

    pub fn services(&self) -> Services<'_> {
        Services::new(self)
    }

    pub fn pkg(&self) -> Packages<'_> {
        Packages::new(self)
    }

    pub fn cron(&self) -> Cron<'_> {
        Cron::new(self)
    }

    pub fn quota(&self) -> Quota<'_> {
        Quota::new(self)
    }

    pub fn template(&self, id: &str) -> Template<'_> {
        Template::new(self, id)
    }

    pub async fn exec(&self, name: &str) -> Result<String> {
        let answer: Output = self.call("host.exec", json!({ "name": name })).await?;
        Ok(answer.output)
    }

    pub async fn exec_into(&self, name: &str, dest: &Dest) -> Result<u64> {
        let answer: Captured = self
            .call("host.exec", json!({ "name": name, "dest": dest }))
            .await?;
        Ok(answer.bytes)
    }

    pub async fn op<A, R>(&self, name: &str, args: &A) -> Result<R>
    where
        A: Serialize + Sync,
        R: DeserializeOwned,
    {
        let args = serde_json::to_value(args).map_err(|error| failure(ERROR_ARGS, error))?;
        self.call(name, args).await
    }

    async fn call<R: DeserializeOwned>(&self, op: &str, args: Value) -> Result<R> {
        let data = self.executor.execute(&self.origin, op, args).await?;
        serde_json::from_value(data).map_err(|error| failure(ERROR_FRAME, format!("{op}: {error}")))
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::sync::Mutex;

    use super::*;

    pub(crate) struct Recorder {
        answers: Mutex<Vec<Value>>,
        calls: Mutex<Vec<(String, Value)>>,
    }

    impl Recorder {
        pub(crate) fn answering(answers: Vec<Value>) -> Self {
            Self {
                answers: Mutex::new(answers.into_iter().rev().collect()),
                calls: Mutex::new(Vec::new()),
            }
        }

        pub(crate) fn calls(&self) -> Vec<(String, Value)> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl HostExecutor for Recorder {
        async fn execute(&self, origin: &Origin, op: &str, args: Value) -> Result<Value> {
            assert_eq!(origin.product, "demo");
            self.calls.lock().unwrap().push((op.to_owned(), args));
            Ok(self.answers.lock().unwrap().pop().unwrap_or(Value::Null))
        }
    }

    pub(crate) fn origin() -> Origin {
        Origin {
            product: "demo".to_owned(),
            login: "alice".to_owned(),
            level: Level::User,
            request_id: "req-1".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{Recorder, origin};
    use super::*;

    #[derive(Serialize)]
    #[serde(crate = "keel_sdk::serde")]
    struct Stat {
        name: String,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(crate = "keel_sdk::serde")]
    struct Lines {
        lines: usize,
    }

    #[tokio::test]
    async fn exec_and_a_product_operation_send_their_name_and_arguments_and_read_the_answer() {
        let recorder = Recorder::answering(vec![
            json!({ "output": "done\n" }),
            json!({ "bytes": 5 }),
            json!({ "lines": 3 }),
            json!({ "unexpected": true }),
        ]);
        let executor: &dyn HostExecutor = &recorder;
        let host = executor.on(origin());
        assert_eq!(host.exec("stamp").await.unwrap(), "done\n");
        assert_eq!(
            host.exec_into("stamp", &Dest::new("dumps", "keel-demo-stamp.txt"))
                .await
                .unwrap(),
            5
        );
        let stat: Lines = host
            .op(
                "note.digest.stat",
                &Stat {
                    name: "keel-demo-digest.txt".to_owned(),
                },
            )
            .await
            .unwrap();
        assert_eq!(stat, Lines { lines: 3 });
        let error = host
            .op::<Stat, Lines>(
                "note.digest.stat",
                &Stat {
                    name: "x".to_owned(),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.key(), "host.error.frame");
        assert_eq!(
            recorder.calls(),
            [
                ("host.exec".to_owned(), json!({ "name": "stamp" })),
                (
                    "host.exec".to_owned(),
                    json!({ "name": "stamp", "dest": { "root": "dumps", "name": "keel-demo-stamp.txt" } })
                ),
                (
                    "note.digest.stat".to_owned(),
                    json!({ "name": "keel-demo-digest.txt" })
                ),
                ("note.digest.stat".to_owned(), json!({ "name": "x" })),
            ]
        );
    }
}
