use std::error::Error as StdError;
use std::fmt;

use keel_types::{CallError, ErrorCode, FieldError};
use serde_json::{Map, Value};

pub type Result<T, E = KeelError> = std::result::Result<T, E>;

type Source = Box<dyn StdError + Send + Sync + 'static>;

#[derive(Debug)]
pub struct KeelError(Box<Inner>);

#[derive(Debug)]
struct Inner {
    code: ErrorCode,
    key: String,
    params: Map<String, Value>,
    field: Option<String>,
    source: Option<Source>,
}

macro_rules! constructors {
    ($($name:ident => $code:ident),* $(,)?) => {
        $(
            pub fn $name(key: impl Into<String>) -> Self {
                Self::with_key(ErrorCode::$code, key)
            }
        )*
    };
}

impl KeelError {
    pub fn new(code: ErrorCode) -> Self {
        Self::with_key(code, code.key())
    }

    pub fn with_key(code: ErrorCode, key: impl Into<String>) -> Self {
        Self(Box::new(Inner {
            code,
            key: key.into(),
            params: Map::new(),
            field: None,
            source: None,
        }))
    }

    constructors! {
        validation => Validation,
        forbidden => Forbidden,
        forbidden_field => ForbiddenField,
        unauthenticated => Unauthenticated,
        not_found => NotFound,
        conflict => Conflict,
        idempotency_mismatch => IdempotencyMismatch,
        limit_exceeded => LimitExceeded,
        license => License,
        rate_limited => RateLimited,
        unavailable => Unavailable,
        peer => Peer,
        internal => Internal,
    }

    pub fn field(mut self, path: impl Into<String>) -> Self {
        self.0.field = Some(path.into());
        self
    }

    pub fn param(mut self, name: impl Into<String>, value: impl Into<Value>) -> Self {
        self.0.params.insert(name.into(), value.into());
        self
    }

    pub fn source(mut self, source: impl StdError + Send + Sync + 'static) -> Self {
        self.0.source = Some(Box::new(source));
        self
    }

    pub fn code(&self) -> ErrorCode {
        self.0.code
    }

    pub fn key(&self) -> &str {
        &self.0.key
    }

    pub fn params(&self) -> &Map<String, Value> {
        &self.0.params
    }

    pub fn field_path(&self) -> Option<&str> {
        self.0.field.as_deref()
    }

    pub fn status(&self) -> u16 {
        self.to_call_error().status()
    }

    pub fn to_call_error(&self) -> CallError {
        let inner = &self.0;
        match &inner.field {
            Some(path) => {
                let mut error = CallError::new(ErrorCode::Validation);
                error.fields.push(FieldError {
                    path: path.clone(),
                    code: Some(inner.key.clone()),
                    key: Some(inner.key.clone()),
                    params: inner.params.clone(),
                });
                error
            }
            None => {
                let mut error = CallError::new(inner.code);
                error.key = inner.key.clone();
                error.params = inner.params.clone();
                error
            }
        }
    }
}

impl fmt::Display for KeelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.0.code.as_str(), self.0.key)?;
        if let Some(field) = &self.0.field {
            write!(f, " at {field}")?;
        }
        if !self.0.params.is_empty() {
            write!(f, " {}", Value::Object(self.0.params.clone()))?;
        }
        Ok(())
    }
}

impl StdError for KeelError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.0
            .source
            .as_deref()
            .map(|source| source as &(dyn StdError + 'static))
    }
}

impl From<sea_orm::DbErr> for KeelError {
    fn from(error: sea_orm::DbErr) -> Self {
        let code = match &error {
            sea_orm::DbErr::RecordNotFound(_) | sea_orm::DbErr::RecordNotUpdated => {
                ErrorCode::NotFound
            }
            _ => ErrorCode::Internal,
        };
        KeelError::new(code).source(error)
    }
}

impl From<serde_json::Error> for KeelError {
    fn from(error: serde_json::Error) -> Self {
        KeelError::new(ErrorCode::Internal).source(error)
    }
}

impl From<KeelError> for CallError {
    fn from(error: KeelError) -> Self {
        error.to_call_error()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_error_code_has_a_constructor_that_keeps_the_key() {
        let built = [
            KeelError::validation("k"),
            KeelError::forbidden("k"),
            KeelError::forbidden_field("k"),
            KeelError::unauthenticated("k"),
            KeelError::not_found("k"),
            KeelError::conflict("k"),
            KeelError::idempotency_mismatch("k"),
            KeelError::limit_exceeded("k"),
            KeelError::license("k"),
            KeelError::rate_limited("k"),
            KeelError::unavailable("k"),
            KeelError::peer("k"),
            KeelError::internal("k"),
        ];
        assert_eq!(built.len(), ErrorCode::ALL.len());
        for (error, code) in built.iter().zip(ErrorCode::ALL) {
            assert_eq!(error.code(), code);
            assert_eq!(error.key(), "k");
            assert_eq!(error.status(), code.status());
        }
    }

    #[test]
    fn new_takes_the_default_key_of_the_code() {
        let error = KeelError::new(ErrorCode::NotFound);
        assert_eq!(error.key(), "keel.error.not_found");
        assert_eq!(
            serde_json::to_value(error.to_call_error()).unwrap(),
            json!({ "code": "not_found", "key": "keel.error.not_found" })
        );
    }

    #[test]
    fn an_error_attached_to_a_field_becomes_a_validation_field_error() {
        let error = KeelError::conflict("dns.zone.error.exists")
            .field("name")
            .param("name", "a.org");
        assert_eq!(error.code(), ErrorCode::Conflict);
        assert_eq!(error.field_path(), Some("name"));
        let wire = error.to_call_error();
        assert_eq!(wire.code, ErrorCode::Validation);
        assert_eq!(wire.status(), 422);
        assert_eq!(
            serde_json::to_value(&wire).unwrap(),
            json!({
                "code": "validation", "key": "keel.error.validation",
                "fields": [ { "path": "name", "code": "dns.zone.error.exists", "key": "dns.zone.error.exists", "params": { "name": "a.org" } } ]
            })
        );
    }

    #[test]
    fn an_error_without_a_field_keeps_its_code_key_and_params() {
        let error = KeelError::limit_exceeded("dns.zone.error.limit").param("max", 50);
        let wire: CallError = error.into();
        assert_eq!(
            serde_json::to_value(&wire).unwrap(),
            json!({ "code": "limit_exceeded", "key": "dns.zone.error.limit", "params": { "max": 50 } })
        );
    }

    #[test]
    fn source_is_kept_on_the_error_chain() {
        let inner = std::io::Error::other("disk");
        let error = KeelError::internal("keel.error.internal").source(inner);
        assert_eq!(
            StdError::source(&error).map(|source| source.to_string()),
            Some("disk".to_owned())
        );
        assert_eq!(error.to_string(), "internal: keel.error.internal");
        assert_eq!(
            KeelError::conflict("x")
                .field("f")
                .param("a", 1)
                .to_string(),
            "conflict: x at f {\"a\":1}"
        );
    }

    #[test]
    fn database_errors_map_to_not_found_or_internal() {
        let missing: KeelError = sea_orm::DbErr::RecordNotFound("zone".into()).into();
        assert_eq!(missing.code(), ErrorCode::NotFound);
        let unchanged: KeelError = sea_orm::DbErr::RecordNotUpdated.into();
        assert_eq!(unchanged.code(), ErrorCode::NotFound);
        let other: KeelError = sea_orm::DbErr::Custom("boom".into()).into();
        assert_eq!(other.code(), ErrorCode::Internal);
        assert!(StdError::source(&other).is_some());
    }
}
