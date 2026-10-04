use keel_types::{CallError, ErrorCode, FieldError};
use serde_json::{Map, Value};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("{0}")]
    Refused(Box<Refusal>),
    #[error("{url} could not be reached after {attempts} attempt(s): {reason}")]
    Transport {
        url: String,
        attempts: u32,
        reason: String,
    },
    #[error("{url} answered {status} outside the contract: {reason}")]
    Answer {
        url: String,
        status: u16,
        reason: String,
    },
    #[error("{0}")]
    Invalid(String),
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[error("{url} refused the call with {status} {} {}{}", error.code.as_str(), error.key, request(error))]
pub struct Refusal {
    pub url: String,
    pub status: u16,
    pub error: CallError,
}

fn request(error: &CallError) -> String {
    error
        .request_id
        .as_deref()
        .map(|id| format!(" (request {id})"))
        .unwrap_or_default()
}

impl Refusal {
    pub fn code(&self) -> ErrorCode {
        self.error.code
    }

    pub fn key(&self) -> &str {
        &self.error.key
    }

    pub fn params(&self) -> &Map<String, Value> {
        &self.error.params
    }

    pub fn param(&self, name: &str) -> Option<&Value> {
        self.error.params.get(name)
    }

    pub fn fields(&self) -> &[FieldError] {
        &self.error.fields
    }

    pub fn field(&self, path: &str) -> Option<&FieldError> {
        self.error.fields.iter().find(|field| field.path == path)
    }

    pub fn request_id(&self) -> Option<&str> {
        self.error.request_id.as_deref()
    }
}

impl Error {
    pub fn refusal(&self) -> Option<&Refusal> {
        match self {
            Error::Refused(refusal) => Some(refusal),
            _ => None,
        }
    }

    pub fn code(&self) -> Option<ErrorCode> {
        self.refusal().map(Refusal::code)
    }

    pub fn key(&self) -> Option<&str> {
        self.refusal().map(Refusal::key)
    }

    pub fn field(&self, path: &str) -> Option<&FieldError> {
        self.refusal().and_then(|refusal| refusal.field(path))
    }

    pub fn status(&self) -> Option<u16> {
        match self {
            Error::Refused(refusal) => Some(refusal.status),
            Error::Answer { status, .. } => Some(*status),
            Error::Transport { .. } | Error::Invalid(_) => None,
        }
    }

    pub fn is_transport(&self) -> bool {
        matches!(self, Error::Transport { .. })
    }

    pub(crate) fn invalid(reason: impl Into<String>) -> Self {
        Error::Invalid(reason.into())
    }

    pub(crate) fn tried(self, attempts: u32) -> Self {
        match self {
            Error::Transport { url, reason, .. } => Error::Transport {
                url,
                attempts,
                reason,
            },
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn refusal() -> Refusal {
        let mut error = CallError::new(ErrorCode::Validation);
        error.request_id = Some("req-7".to_owned());
        error.params.insert("max".to_owned(), json!(64));
        error.fields.push(FieldError {
            path: "login".to_owned(),
            code: None,
            key: Some("keel.schema.required".to_owned()),
            params: Map::new(),
        });
        Refusal {
            url: "https://dns.example/api/v1/call/keel.principal.create".to_owned(),
            status: 422,
            error,
        }
    }

    #[test]
    fn a_refusal_answers_its_code_key_params_fields_and_request_id() {
        let refused = Error::Refused(Box::new(refusal()));
        assert_eq!(refused.code(), Some(ErrorCode::Validation));
        assert_eq!(refused.key(), Some("keel.error.validation"));
        assert_eq!(refused.status(), Some(422));
        assert_eq!(
            refused
                .field("login")
                .and_then(|field| field.key.as_deref()),
            Some("keel.schema.required")
        );
        assert!(refused.field("level").is_none());
        let inner = refused.refusal().expect("a refusal");
        assert_eq!(inner.param("max"), Some(&json!(64)));
        assert_eq!(inner.params().len(), 1);
        assert_eq!(inner.fields().len(), 1);
        assert_eq!(inner.request_id(), Some("req-7"));
        assert_eq!(
            refused.to_string(),
            "https://dns.example/api/v1/call/keel.principal.create refused the call with 422 validation keel.error.validation (request req-7)"
        );
        assert!(!refused.is_transport());
    }

    #[test]
    fn only_a_transport_failure_counts_its_attempts_and_carries_no_status() {
        let failed = Error::Transport {
            url: "https://dns.example".to_owned(),
            attempts: 1,
            reason: "connection refused".to_owned(),
        }
        .tried(3);
        assert!(failed.is_transport());
        assert_eq!(failed.status(), None);
        assert_eq!(failed.code(), None);
        assert_eq!(
            failed.to_string(),
            "https://dns.example could not be reached after 3 attempt(s): connection refused"
        );
        let answer = Error::Answer {
            url: "https://dns.example".to_owned(),
            status: 502,
            reason: "not an envelope".to_owned(),
        }
        .tried(3);
        assert_eq!(answer.status(), Some(502));
        assert!(answer.key().is_none());
        assert_eq!(Error::invalid("no").status(), None);
    }
}
