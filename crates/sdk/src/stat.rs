use std::fmt;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt;
use futures::future::BoxFuture;
use keel_types::Id;

use crate::db::Db;
use crate::error::Result;
use crate::limit::Unit;

type Collect = Arc<dyn Fn(Db) -> BoxFuture<'static, Result<Vec<(String, f64)>>> + Send + Sync>;

#[derive(Clone)]
pub struct StatCollector {
    key: String,
    unit: Unit,
    interval: Duration,
    per_account: bool,
    collect: Collect,
}

impl StatCollector {
    pub fn new<F, Fut>(key: &str, unit: Unit, interval: Duration, collect: F) -> Self
    where
        F: Fn(Db) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<f64>> + Send + 'static,
    {
        Self {
            key: key.to_owned(),
            unit,
            interval,
            per_account: false,
            collect: Arc::new(move |db| {
                collect(db)
                    .map(|value| value.map(|value| vec![(String::new(), value)]))
                    .boxed()
            }),
        }
    }

    pub fn per_account<F, Fut>(key: &str, unit: Unit, interval: Duration, collect: F) -> Self
    where
        F: Fn(Db) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Vec<(Id, f64)>>> + Send + 'static,
    {
        Self {
            key: key.to_owned(),
            unit,
            interval,
            per_account: true,
            collect: Arc::new(move |db| {
                collect(db)
                    .map(|values| {
                        values.map(|values| {
                            values
                                .into_iter()
                                .map(|(account, value)| (account.to_string(), value))
                                .collect()
                        })
                    })
                    .boxed()
            }),
        }
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn unit(&self) -> Unit {
        self.unit
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }

    pub fn is_per_account(&self) -> bool {
        self.per_account
    }

    pub fn sample(&self, db: Db) -> BoxFuture<'static, Result<Vec<(String, f64)>>> {
        (self.collect)(db)
    }
}

impl fmt::Debug for StatCollector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StatCollector")
            .field("key", &self.key)
            .field("unit", &self.unit.as_str())
            .field("interval", &self.interval)
            .field("per_account", &self.per_account)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::DatabaseConnection;

    use super::*;

    fn db() -> Db {
        Db::from_connection(DatabaseConnection::Disconnected)
    }

    #[tokio::test]
    async fn an_installation_collector_samples_one_value_without_a_subject() {
        let collector = StatCollector::new(
            "keel.accounts",
            Unit::Count,
            Duration::from_secs(3600),
            |_| async { Ok(3.0) },
        );
        assert_eq!(collector.key(), "keel.accounts");
        assert_eq!(collector.unit(), Unit::Count);
        assert_eq!(collector.interval(), Duration::from_secs(3600));
        assert!(!collector.is_per_account());
        assert_eq!(
            collector.sample(db()).await.unwrap(),
            [(String::new(), 3.0)]
        );
        assert_eq!(
            format!("{collector:?}"),
            "StatCollector { key: \"keel.accounts\", unit: \"count\", interval: 3600s, per_account: false }"
        );
    }

    #[tokio::test]
    async fn a_per_account_collector_samples_a_value_per_account() {
        let account: Id = "01J8A0000000000000000000A1".parse().unwrap();
        let collector = StatCollector::per_account(
            "dns.zones",
            Unit::Count,
            Duration::from_secs(300),
            move |_| async move { Ok(vec![(account, 2.0)]) },
        );
        assert!(collector.is_per_account());
        assert_eq!(
            collector.sample(db()).await.unwrap(),
            [(account.to_string(), 2.0)]
        );
    }
}
