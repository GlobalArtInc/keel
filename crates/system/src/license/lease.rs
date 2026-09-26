use keel_sdk::chrono::{DateTime, Days, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ConnectionTrait, DbBackend, Statement, Value as DbValue};
use keel_sdk::system::{Installation, LicenseKey};
use keel_sdk::types::LicenseState;

pub const KV_KEY: &str = "license_lease";
pub const ERROR_SIGNATURE: &str = "license.error.signature";
pub const ERROR_LEASE: &str = "license.error.lease";
pub const ERROR_INSTANCE: &str = "license.error.instance";
pub const ERROR_OLDER: &str = "license.error.older";
const ERROR_STORED: &str = "license.error.stored";

const FIELD: &str = "lease";

#[derive(Clone, Debug, Deserialize)]
#[serde(crate = "keel_sdk::serde")]
pub(super) struct Lease {
    pub(super) instance_id: Id,
    pub(super) issued_at: DateTime<Utc>,
    pub(super) expires_at: DateTime<Utc>,
    pub(super) grace_days: u32,
    pub(super) products: Vec<Leased>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(crate = "keel_sdk::serde")]
pub(super) struct Leased {
    pub(super) id: String,
    pub(super) edition: String,
}

impl Lease {
    pub(super) fn open(
        signed: &str,
        key: &dyn LicenseKey,
        installation: &Installation,
    ) -> Result<Self> {
        let payload = key.open(signed.as_bytes()).map_err(|reason| {
            KeelError::validation(ERROR_SIGNATURE)
                .field(FIELD)
                .param("reason", reason)
        })?;
        let lease: Lease = keel_sdk::serde_json::from_slice(&payload).map_err(|error| {
            KeelError::validation(ERROR_LEASE)
                .field(FIELD)
                .param("reason", error.to_string())
        })?;
        if installation.instance != Some(lease.instance_id) {
            return Err(KeelError::validation(ERROR_INSTANCE)
                .field(FIELD)
                .param("instance", lease.instance_id.to_string()));
        }
        Ok(lease)
    }

    pub(super) fn replaces(&self, installed: Option<&Lease>) -> Result<()> {
        match installed {
            Some(installed) if self.issued_at < installed.issued_at => {
                Err(KeelError::validation(ERROR_OLDER)
                    .field(FIELD)
                    .param("issued", self.issued_at.to_rfc3339())
                    .param("installed", installed.issued_at.to_rfc3339()))
            }
            _ => Ok(()),
        }
    }

    pub(super) fn edition(&self, product: &str) -> Option<&str> {
        self.products
            .iter()
            .find(|leased| leased.id == product)
            .map(|leased| leased.edition.as_str())
    }

    pub(super) fn grace_ends_at(&self) -> DateTime<Utc> {
        self.expires_at
            .checked_add_days(Days::new(u64::from(self.grace_days)))
            .unwrap_or(DateTime::<Utc>::MAX_UTC)
    }

    pub(super) fn state(&self, now: DateTime<Utc>) -> LicenseState {
        if now < self.expires_at {
            LicenseState::Valid
        } else if now < self.grace_ends_at() {
            LicenseState::Grace
        } else {
            LicenseState::ReadOnly
        }
    }
}

pub(super) async fn stored(db: &Db) -> Result<Option<String>> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT value FROM keel_kv WHERE key = $1",
            [DbValue::from(KV_KEY)],
        ))
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    match row.try_get::<Value>("", "value")? {
        Value::String(signed) => Ok(Some(signed)),
        other => Err(KeelError::internal(ERROR_STORED).param("value", other.to_string())),
    }
}

pub(super) async fn store(db: &Db, signed: &str) -> Result<()> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO keel_kv (key, value) VALUES ($1, $2) \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
        [
            DbValue::from(KV_KEY),
            DbValue::Json(Some(Box::new(Value::String(signed.to_owned())))),
        ],
    ))
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use keel_sdk::chrono::TimeZone;

    use super::*;

    fn lease(expires_at: DateTime<Utc>, grace_days: u32) -> Lease {
        Lease {
            instance_id: Id::ROOT_ACCOUNT,
            issued_at: expires_at - Days::new(365),
            expires_at,
            grace_days,
            products: vec![Leased {
                id: "dns".to_owned(),
                edition: "pro".to_owned(),
            }],
        }
    }

    #[test]
    fn a_lease_is_valid_until_it_expires_then_in_grace_then_read_only() {
        let expires = Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap();
        let lease = lease(expires, 30);
        assert_eq!(lease.state(expires - Days::new(1)), LicenseState::Valid);
        assert_eq!(lease.state(expires), LicenseState::Grace);
        assert_eq!(
            lease.grace_ends_at(),
            Utc.with_ymd_and_hms(2027, 1, 31, 0, 0, 0).unwrap()
        );
        assert_eq!(lease.state(lease.grace_ends_at()), LicenseState::ReadOnly);
        assert_eq!(lease.edition("dns"), Some("pro"));
        assert_eq!(lease.edition("panel"), None);
    }

    #[test]
    fn a_lease_replaces_one_issued_at_the_same_time_or_earlier_but_never_a_newer_one() {
        let expires = Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap();
        let installed = lease(expires, 30);
        let mut older = lease(expires + Days::new(365), 30);
        older.issued_at = installed.issued_at - Days::new(1);
        let error = older.replaces(Some(&installed)).unwrap_err();
        assert_eq!(error.key(), ERROR_OLDER);
        assert_eq!(error.field_path(), Some(FIELD));
        assert!(lease(expires, 0).replaces(Some(&installed)).is_ok());
        assert!(older.replaces(None).is_ok());
        assert!(installed.replaces(Some(&older)).is_ok());
    }

    #[test]
    fn a_lease_without_grace_turns_read_only_when_it_expires() {
        let expires = Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(lease(expires, 0).state(expires), LicenseState::ReadOnly);
    }
}
