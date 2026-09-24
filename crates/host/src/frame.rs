use keel_sdk::serde::{Deserialize, Serialize};
use keel_sdk::serde_json::{self, Value};
use keel_sdk::types::ErrorCode;
use keel_sdk::{KeelError, Result};

use crate::api::Origin;

const DETAIL: &str = "detail";
pub(crate) const ERROR_FRAME: &str = "host.error.frame";
pub(crate) const ERROR_ARGS: &str = "host.error.args";
pub(crate) const ERROR_UNREACHABLE: &str = "host.error.unreachable";
pub(crate) const ERROR_TIMEOUT: &str = "host.error.timeout";
const ERROR_POLICY: &str = "host.error.policy";
const ERROR_NOT_FOUND: &str = "host.error.not_found";
const ERROR_RENDER_FAILED: &str = "host.error.render_failed";
const ERROR_VALIDATE_FAILED: &str = "host.error.validate_failed";
const ERROR_UNAVAILABLE: &str = "host.error.unavailable";

#[derive(Serialize)]
#[serde(crate = "keel_sdk::serde")]
struct Request<'a> {
    id: u64,
    op: &'a str,
    actor: Actor<'a>,
    request_id: &'a str,
    product: &'a str,
    args: Value,
}

#[derive(Serialize)]
#[serde(crate = "keel_sdk::serde")]
struct Actor<'a> {
    login: &'a str,
    level: &'a str,
}

#[derive(Deserialize)]
#[serde(crate = "keel_sdk::serde")]
struct Answer {
    id: u64,
    ok: bool,
    #[serde(default)]
    data: Option<Value>,
    #[serde(default)]
    error: Option<Failure>,
}

#[derive(Deserialize)]
#[serde(crate = "keel_sdk::serde")]
struct Failure {
    key: String,
    #[serde(default)]
    detail: String,
}

pub(crate) fn request(id: u64, origin: &Origin, op: &str, args: Value) -> Result<Vec<u8>> {
    let request = Request {
        id,
        op,
        actor: Actor {
            login: &origin.login,
            level: origin.level.as_str(),
        },
        request_id: &origin.request_id,
        product: &origin.product,
        args,
    };
    let mut line = serde_json::to_vec(&request).map_err(|error| failure(ERROR_ARGS, error))?;
    line.push(b'\n');
    Ok(line)
}

pub(crate) fn answer(id: u64, line: &[u8]) -> Result<Value> {
    let answer: Answer =
        serde_json::from_slice(line).map_err(|error| failure(ERROR_FRAME, error))?;
    if answer.id != id {
        return Err(failure(
            ERROR_FRAME,
            format!(
                "the agent answered the request {} to the request {id}",
                answer.id
            ),
        ));
    }
    match (answer.ok, answer.data, answer.error) {
        (true, data, _) => Ok(data.unwrap_or(Value::Null)),
        (false, _, Some(error)) => Err(failure(&error.key, error.detail)),
        (false, _, None) => Err(failure(
            ERROR_FRAME,
            "the agent refused the request without an error",
        )),
    }
}

pub(crate) fn failure(key: &str, detail: impl ToString) -> KeelError {
    KeelError::with_key(code(key), key).param(DETAIL, detail.to_string())
}

fn code(key: &str) -> ErrorCode {
    match key {
        ERROR_POLICY => ErrorCode::Forbidden,
        ERROR_ARGS | ERROR_RENDER_FAILED | ERROR_VALIDATE_FAILED => ErrorCode::Validation,
        ERROR_NOT_FOUND => ErrorCode::NotFound,
        ERROR_UNAVAILABLE | ERROR_UNREACHABLE | ERROR_TIMEOUT => ErrorCode::Unavailable,
        _ => ErrorCode::Internal,
    }
}

#[cfg(test)]
mod tests {
    use keel_sdk::serde_json::json;
    use keel_sdk::types::Level;

    use super::*;

    fn origin() -> Origin {
        Origin {
            product: "dns".to_owned(),
            login: "alice".to_owned(),
            level: Level::User,
            request_id: "req-1a2b".to_owned(),
        }
    }

    #[test]
    fn a_request_is_one_line_in_the_shape_of_the_protocol() {
        let line = request(
            1,
            &origin(),
            "host.template.apply",
            json!({ "template": "bind/zone.j2" }),
        )
        .unwrap();
        assert_eq!(line.last(), Some(&b'\n'));
        assert_eq!(line.iter().filter(|byte| **byte == b'\n').count(), 1);
        let text = std::str::from_utf8(&line).unwrap().trim_end();
        assert_eq!(
            text,
            r#"{"id":1,"op":"host.template.apply","actor":{"login":"alice","level":"user"},"request_id":"req-1a2b","product":"dns","args":{"template":"bind/zone.j2"}}"#
        );
    }

    #[test]
    fn an_answer_yields_its_data_or_its_keyed_error_with_the_detail() {
        assert_eq!(
            answer(1, br#"{"id":1,"ok":true,"data":{"changed":true}}"#).unwrap(),
            json!({ "changed": true })
        );
        let error = answer(
            2,
            br#"{"id":2,"ok":false,"error":{"key":"host.error.validate_failed","detail":"nginx: [emerg] unknown directive"}}"#,
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::Validation);
        assert_eq!(error.key(), "host.error.validate_failed");
        assert_eq!(
            error.params()[DETAIL],
            json!("nginx: [emerg] unknown directive")
        );
        let other = answer(3, br#"{"id":4,"ok":true,"data":null}"#).unwrap_err();
        assert_eq!(other.key(), ERROR_FRAME);
        assert_eq!(
            answer(5, b"not json").unwrap_err().code(),
            ErrorCode::Internal
        );
        assert_eq!(
            answer(6, br#"{"id":6,"ok":false}"#).unwrap_err().key(),
            ERROR_FRAME
        );
    }

    #[test]
    fn every_key_of_the_agent_maps_to_the_code_its_envelope_carries() {
        let expected = [
            ("host.error.policy", ErrorCode::Forbidden),
            ("host.error.args", ErrorCode::Validation),
            ("host.error.render_failed", ErrorCode::Validation),
            ("host.error.validate_failed", ErrorCode::Validation),
            ("host.error.not_found", ErrorCode::NotFound),
            ("host.error.unavailable", ErrorCode::Unavailable),
            ("host.error.unreachable", ErrorCode::Unavailable),
            ("host.error.timeout", ErrorCode::Unavailable),
            ("host.error.reload_failed", ErrorCode::Internal),
            ("host.error.exec_failed", ErrorCode::Internal),
            ("host.error.io", ErrorCode::Internal),
            ("host.error.frame", ErrorCode::Internal),
            ("host.error.unknown_op", ErrorCode::Internal),
            ("vm.error.domain", ErrorCode::Internal),
        ];
        for (key, code) in expected {
            let error = failure(key, "detail");
            assert_eq!(error.code(), code, "{key}");
            assert_eq!(error.key(), key);
        }
    }
}
