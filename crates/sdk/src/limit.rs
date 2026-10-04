use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Count,
    Bytes,
    Seconds,
}

impl Unit {
    pub const fn as_str(self) -> &'static str {
        match self {
            Unit::Count => "count",
            Unit::Bytes => "bytes",
            Unit::Seconds => "seconds",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct LimitDef {
    key: String,
    unit: Unit,
    default: Option<u64>,
    unlimited: bool,
}

impl LimitDef {
    pub fn new(key: &str, unit: Unit) -> Self {
        Self {
            key: key.to_owned(),
            unit,
            default: None,
            unlimited: false,
        }
    }

    pub fn default(mut self, value: u64) -> Self {
        self.default = Some(value);
        self
    }

    pub fn unlimited(mut self, allowed: bool) -> Self {
        self.unlimited = allowed;
        self
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn default_value(&self) -> Option<u64> {
        self.default
    }

    pub fn allows_unlimited(&self) -> bool {
        self.unlimited
    }
}

impl fmt::Debug for LimitDef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LimitDef")
            .field("key", &self.key)
            .field("unit", &self.unit.as_str())
            .field("default", &self.default)
            .field("unlimited", &self.unlimited)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use keel_types::{ErrorCode, Id, Level};
    use sea_orm::DatabaseConnection;
    use serde_json::json;

    use super::*;
    use crate::call::CallBuilder;
    use crate::db::Db;
    use crate::error::Result;
    use crate::services::Services;
    use crate::system::LimitSource;

    struct Fixed;

    #[async_trait]
    impl LimitSource for Fixed {
        async fn effective(&self, key: &str, account: Id, _: &Db) -> Result<Option<u64>> {
            Ok(match key {
                "dns.zones" if account.to_string().ends_with("A1") => Some(2),
                _ => None,
            })
        }

        async fn store(&self, _: &str, _: Id, _: Option<u64>, _: &Db) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_call_is_refused_once_the_current_count_reaches_the_effective_limit() {
        let mut services = Services::new();
        services.provide::<dyn LimitSource>(Arc::new(Fixed));
        let handle = CallBuilder::new(crate::tests::principal(Level::User), "req-1")
            .services(Arc::new(services))
            .read(DatabaseConnection::Disconnected);
        let call = handle.call(());
        assert!(call.check_limit("dns.zones", 1).await.is_ok());
        let refused = call.check_limit("dns.zones", 2).await.unwrap_err();
        assert_eq!(refused.code(), ErrorCode::LimitExceeded);
        assert_eq!(refused.key(), "keel.error.limit_exceeded");
        assert_eq!(refused.params()["limit"], json!("dns.zones"));
        assert_eq!(refused.params()["max"], json!(2));
        assert!(call.check_limit("dns.other", 1_000).await.is_ok());
        let bare = crate::tests::read_handle();
        let missing = bare.call(()).check_limit("dns.zones", 0).await.unwrap_err();
        assert_eq!(missing.key(), "keel.error.service_missing");
    }

    #[test]
    fn a_limit_carries_its_key_unit_default_and_whether_it_may_be_unlimited() {
        let limit = LimitDef::new("dns.zones", Unit::Count)
            .default(50)
            .unlimited(true);
        assert_eq!(limit.key(), "dns.zones");
        assert_eq!(limit.default_value(), Some(50));
        assert!(limit.allows_unlimited());
        let bare = LimitDef::new("dns.zones", Unit::Count);
        assert_eq!(bare.default_value(), None);
        assert!(!bare.allows_unlimited());
        assert_eq!(
            format!("{limit:?}"),
            "LimitDef { key: \"dns.zones\", unit: \"count\", default: Some(50), unlimited: true }"
        );
    }
}
