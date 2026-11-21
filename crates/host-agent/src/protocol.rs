use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const LINE_LIMIT: usize = 1024 * 1024;
pub(crate) const FIELD_LIMIT: usize = 1024;

pub(crate) const ERROR_FRAME: &str = "host.error.frame";
pub(crate) const ERROR_UNKNOWN_OP: &str = "host.error.unknown_op";
pub(crate) const ERROR_ARGS: &str = "host.error.args";
pub(crate) const ERROR_POLICY: &str = "host.error.policy";
pub(crate) const ERROR_NOT_FOUND: &str = "host.error.not_found";
pub(crate) const ERROR_RENDER_FAILED: &str = "host.error.render_failed";
pub(crate) const ERROR_VALIDATE_FAILED: &str = "host.error.validate_failed";
pub(crate) const ERROR_RELOAD_FAILED: &str = "host.error.reload_failed";
pub(crate) const ERROR_EXEC_FAILED: &str = "host.error.exec_failed";
pub(crate) const ERROR_IO: &str = "host.error.io";
pub(crate) const ERROR_UNAVAILABLE: &str = "host.error.unavailable";
pub(crate) const ERROR_EXISTS: &str = "host.error.exists";
pub(crate) const ERROR_TIMEOUT: &str = "host.error.timeout";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Actor {
    pub(crate) login: String,
    pub(crate) level: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    pub(crate) id: u64,
    pub(crate) op: String,
    pub(crate) actor: Actor,
    pub(crate) request_id: String,
    pub(crate) product: String,
    #[serde(default)]
    pub(crate) args: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Failure {
    pub(crate) key: String,
    pub(crate) detail: String,
}

impl Failure {
    pub fn new(key: &str, detail: impl Into<String>) -> Self {
        Self {
            key: key.to_owned(),
            detail: detail.into(),
        }
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }

    pub(crate) fn policy(detail: impl Into<String>) -> Self {
        Self::new(ERROR_POLICY, detail)
    }

    pub(crate) fn args(detail: impl Into<String>) -> Self {
        Self::new(ERROR_ARGS, detail)
    }

    pub(crate) fn io(what: impl std::fmt::Display, error: &std::io::Error) -> Self {
        Self::new(ERROR_IO, format!("{what}: {error}"))
    }

    pub(crate) fn rekeyed(self, key: &str) -> Self {
        if self.key == ERROR_TIMEOUT {
            self
        } else {
            Self::new(key, self.detail)
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Answer {
    pub(crate) id: u64,
    pub(crate) ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<Failure>,
}

impl Answer {
    pub(crate) fn ok(id: u64, data: Value) -> Self {
        Self {
            id,
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    pub(crate) fn failed(id: u64, failure: Failure) -> Self {
        Self {
            id,
            ok: false,
            data: None,
            error: Some(failure),
        }
    }
}

pub(crate) fn parse_request(line: &[u8]) -> Result<Request, (u64, Failure)> {
    let request: Request = serde_json::from_slice(line).map_err(|error| {
        let id = serde_json::from_slice::<Value>(line)
            .ok()
            .and_then(|value| value.get("id").and_then(Value::as_u64))
            .unwrap_or(0);
        (id, Failure::new(ERROR_FRAME, error.to_string()))
    })?;
    let fields = [
        ("op", &request.op),
        ("actor.login", &request.actor.login),
        ("actor.level", &request.actor.level),
        ("request_id", &request.request_id),
        ("product", &request.product),
    ];
    for (name, value) in fields {
        if value.is_empty() {
            return Err((
                request.id,
                Failure::new(ERROR_FRAME, format!("{name} is empty")),
            ));
        }
        if value.len() > FIELD_LIMIT {
            return Err((
                request.id,
                Failure::new(ERROR_FRAME, format!("{name} is over {FIELD_LIMIT} bytes")),
            ));
        }
        if value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
        {
            return Err((
                request.id,
                Failure::new(
                    ERROR_FRAME,
                    format!("{name} carries whitespace or a control character"),
                ),
            ));
        }
    }
    Ok(request)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const DESIGN_LINE: &str = r#"{"id":1,"op":"host.template.apply","actor":{"login":"alice","level":"user"},"request_id":"req-1a2b","product":"dns","args":{"template":"bind/zone.j2","vars":{"a":1},"dest":{"root":"zones","name":"keel-dns-a.org.zone"},"mode":"0640","owner":"bind","validate":"named-checkzone","reload":"named"}}"#;

    #[test]
    fn the_request_frame_of_the_design_parses_with_its_actor_and_args() {
        let request = parse_request(DESIGN_LINE.as_bytes()).unwrap();
        assert_eq!(request.id, 1);
        assert_eq!(request.op, "host.template.apply");
        assert_eq!(
            request.actor,
            Actor {
                login: "alice".to_owned(),
                level: "user".to_owned()
            }
        );
        assert_eq!(request.request_id, "req-1a2b");
        assert_eq!(request.product, "dns");
        assert_eq!(request.args["dest"]["root"], "zones");
        assert_eq!(request.args["vars"]["a"], 1);
        let echoed = serde_json::to_string(&request).unwrap();
        assert_eq!(parse_request(echoed.as_bytes()).unwrap(), request);
    }

    #[test]
    fn a_line_that_is_not_a_request_fails_as_a_frame_keeping_its_id_when_it_has_one() {
        let (id, failure) = parse_request(br#"{"id":7,"op":"host.user.create"}"#).unwrap_err();
        assert_eq!(id, 7);
        assert_eq!(failure.key, ERROR_FRAME);
        assert!(
            failure.detail.contains("missing field"),
            "{}",
            failure.detail
        );
        let (id, failure) = parse_request(b"not json").unwrap_err();
        assert_eq!(id, 0);
        assert_eq!(failure.key, ERROR_FRAME);
        let (id, _) = parse_request(br#"{"id":"one"}"#).unwrap_err();
        assert_eq!(id, 0);
        let (id, failure) = parse_request(
            br#"{"id":3,"op":"x","actor":{"login":"a","level":"user"},"request_id":"r","product":"demo","extra":1}"#,
        )
        .unwrap_err();
        assert_eq!(id, 3);
        assert!(
            failure.detail.contains("unknown field"),
            "{}",
            failure.detail
        );
        let (_, failure) = parse_request(
            br#"{"id":3,"op":"x","actor":{"login":"","level":"user"},"request_id":"r","product":"demo"}"#,
        )
        .unwrap_err();
        assert_eq!(failure.detail, "actor.login is empty");
        let forged = [
            ("op", "host.user.create outcome=ok"),
            ("actor.login", "alice\naudit op=host.user.create actor=bob"),
            ("actor.level", "user\tadmin"),
            ("request_id", "r 1"),
            ("product", "demo\u{7}"),
        ];
        for (field, value) in forged {
            let mut request = json!({
                "id": 4,
                "op": "x",
                "actor": { "login": "a", "level": "user" },
                "request_id": "r",
                "product": "demo",
            });
            match field {
                "actor.login" => request["actor"]["login"] = json!(value),
                "actor.level" => request["actor"]["level"] = json!(value),
                other => request[other] = json!(value),
            }
            let (id, failure) = parse_request(request.to_string().as_bytes()).unwrap_err();
            assert_eq!(id, 4, "{field}");
            assert_eq!(failure.key, ERROR_FRAME, "{field}");
            assert_eq!(
                failure.detail,
                format!("{field} carries whitespace or a control character")
            );
        }
    }

    fn frame(field: &str, value: Value) -> Value {
        let mut request = json!({
            "id": 5,
            "op": "host.fs.read",
            "actor": { "login": "alice", "level": "user" },
            "request_id": "req-1",
            "product": "dns",
        });
        match field {
            "actor.login" => request["actor"]["login"] = value,
            other => request[other] = value,
        }
        request
    }

    #[test]
    fn a_frame_field_over_the_field_limit_is_refused_before_it_reaches_the_journal() {
        for field in ["op", "actor.login", "request_id", "product"] {
            let at_limit = frame(field, json!("a".repeat(FIELD_LIMIT)));
            assert!(
                parse_request(at_limit.to_string().as_bytes()).is_ok(),
                "{field}"
            );
            let over = frame(field, json!("a".repeat(FIELD_LIMIT + 1)));
            let (id, failure) = parse_request(over.to_string().as_bytes()).unwrap_err();
            assert_eq!(id, 5, "{field}");
            assert_eq!(failure.key, ERROR_FRAME);
            assert_eq!(
                failure.detail,
                format!("{field} is over {FIELD_LIMIT} bytes")
            );
        }
    }

    #[test]
    fn a_frame_nested_past_the_depth_limit_is_refused_as_a_frame_without_overflowing() {
        let depth = 100_000;
        let args = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        let line = format!(
            r#"{{"id":6,"op":"host.fs.read","actor":{{"login":"a","level":"user"}},"request_id":"r","product":"dns","args":{args}}}"#
        );
        let (_, failure) = parse_request(line.as_bytes()).unwrap_err();
        assert_eq!(failure.key, ERROR_FRAME);
        assert!(
            failure.detail.contains("recursion limit"),
            "{}",
            failure.detail
        );
        let shallow = format!("{}{}", "[".repeat(100), "]".repeat(100));
        let line = format!(
            r#"{{"id":6,"op":"host.fs.read","actor":{{"login":"a","level":"user"}},"request_id":"r","product":"dns","args":{shallow}}}"#
        );
        assert!(parse_request(line.as_bytes()).is_ok());
    }

    #[test]
    fn a_frame_naming_a_field_twice_is_refused_so_no_reader_sees_another_product() {
        let line = br#"{"id":8,"op":"host.fs.read","actor":{"login":"a","level":"user"},"request_id":"r","product":"dns","product":"panel"}"#;
        let (id, failure) = parse_request(line).unwrap_err();
        assert_eq!(id, 8);
        assert_eq!(failure.key, ERROR_FRAME);
        assert!(
            failure.detail.contains("duplicate field `product`"),
            "{}",
            failure.detail
        );
        let line = br#"{"id":8,"op":"host.fs.read","actor":{"login":"a","level":"user","login":"b"},"request_id":"r","product":"dns"}"#;
        let (_, failure) = parse_request(line).unwrap_err();
        assert!(
            failure.detail.contains("duplicate field `login`"),
            "{}",
            failure.detail
        );
    }

    #[test]
    fn answers_serialize_to_the_two_shapes_of_the_protocol() {
        let ok = Answer::ok(1, json!({ "changed": true }));
        assert_eq!(
            serde_json::to_string(&ok).unwrap(),
            r#"{"id":1,"ok":true,"data":{"changed":true}}"#
        );
        let failed = Answer::failed(
            2,
            Failure::new(ERROR_VALIDATE_FAILED, "nginx: [emerg] unknown directive"),
        );
        assert_eq!(
            serde_json::to_string(&failed).unwrap(),
            r#"{"id":2,"ok":false,"error":{"key":"host.error.validate_failed","detail":"nginx: [emerg] unknown directive"}}"#
        );
        assert_eq!(
            serde_json::from_str::<Answer>(r#"{"id":1,"ok":true,"data":{"changed":true}}"#)
                .unwrap(),
            ok
        );
        assert_eq!(
            serde_json::from_str::<Answer>(
                r#"{"id":2,"ok":false,"error":{"key":"host.error.validate_failed","detail":"nginx: [emerg] unknown directive"}}"#
            )
            .unwrap(),
            failed
        );
    }
}
