use keel_sdk::KeelError;
use serde::{Deserialize, Serialize};
use serde_json::error::Category;
use serde_json::{Map, Value};

use crate::http::BODY_LIMIT;

pub const FRAME_LIMIT: usize = BODY_LIMIT;

#[derive(Clone, Debug, PartialEq)]
pub enum Frame {
    Call {
        func: String,
        params: Value,
        request_id: Option<String>,
        idempotency_key: Option<String>,
        run_as: Option<String>,
    },
    Describe {
        name: String,
    },
    I18n {
        lang: String,
    },
    Boot,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Wire {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    func: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    idempotency_key: Option<String>,
    #[serde(default, rename = "as", skip_serializing_if = "Option::is_none")]
    run_as: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    describe: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    i18n: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    boot: Option<bool>,
}

impl Frame {
    pub fn describe(name: &str) -> Self {
        Frame::Describe {
            name: name.to_owned(),
        }
    }

    pub fn i18n(lang: &str) -> Self {
        Frame::I18n {
            lang: lang.to_owned(),
        }
    }

    pub fn parse(line: &[u8]) -> Result<Self, KeelError> {
        let wire: Wire = serde_json::from_slice(line).map_err(|error| {
            let key = match error.classify() {
                Category::Data => "keel.error.frame_unknown",
                Category::Io | Category::Syntax | Category::Eof => "keel.error.body_not_json",
            };
            KeelError::validation(key).source(error)
        })?;
        match wire {
            Wire {
                func: Some(func),
                params,
                request_id,
                idempotency_key,
                run_as,
                describe: None,
                i18n: None,
                boot: None,
            } => Ok(Frame::Call {
                func,
                params: params.unwrap_or_else(|| Value::Object(Map::new())),
                request_id: request_id.filter(|id| !id.is_empty()),
                idempotency_key: idempotency_key
                    .map(|key| key.trim().to_owned())
                    .filter(|key| !key.is_empty()),
                run_as: run_as
                    .map(|login| login.trim().to_owned())
                    .filter(|login| !login.is_empty()),
            }),
            Wire {
                func: None,
                params: None,
                request_id: None,
                idempotency_key: None,
                run_as: None,
                describe: Some(name),
                i18n: None,
                boot: None,
            } => Ok(Frame::Describe { name }),
            Wire {
                func: None,
                params: None,
                request_id: None,
                idempotency_key: None,
                run_as: None,
                describe: None,
                i18n: Some(lang),
                boot: None,
            } => Ok(Frame::I18n {
                lang: lang.trim().to_ascii_lowercase(),
            }),
            Wire {
                func: None,
                params: None,
                request_id: None,
                idempotency_key: None,
                run_as: None,
                describe: None,
                i18n: None,
                boot: Some(true),
            } => Ok(Frame::Boot),
            _ => Err(KeelError::validation("keel.error.frame_unknown")),
        }
    }

    pub fn to_line(&self) -> Result<String, serde_json::Error> {
        let wire = match self {
            Frame::Call {
                func,
                params,
                request_id,
                idempotency_key,
                run_as,
            } => Wire {
                func: Some(func.clone()),
                params: Some(params.clone()),
                request_id: request_id.clone(),
                idempotency_key: idempotency_key.clone(),
                run_as: run_as.clone(),
                ..Wire::default()
            },
            Frame::Describe { name } => Wire {
                describe: Some(name.clone()),
                ..Wire::default()
            },
            Frame::I18n { lang } => Wire {
                i18n: Some(lang.clone()),
                ..Wire::default()
            },
            Frame::Boot => Wire {
                boot: Some(true),
                ..Wire::default()
            },
        };
        let mut line = serde_json::to_string(&wire)?;
        line.push('\n');
        Ok(line)
    }
}

#[cfg(test)]
mod tests {
    use keel_types::ErrorCode;
    use serde_json::json;

    use super::*;

    #[test]
    fn a_call_frame_carries_the_function_its_parameters_and_the_optional_ids() {
        let frame = Frame::parse(
            br#"{"func":"keel.account.list","params":{"page":2},"requestId":"req-1","idempotencyKey":" k1 ","as":" a1 "}"#,
        )
        .unwrap();
        assert_eq!(
            frame,
            Frame::Call {
                func: "keel.account.list".into(),
                params: json!({ "page": 2 }),
                request_id: Some("req-1".into()),
                idempotency_key: Some("k1".into()),
                run_as: Some("a1".into()),
            }
        );
        let bare = Frame::parse(br#"{"func":"keel.account.list","as":""}"#).unwrap();
        assert_eq!(
            bare,
            Frame::Call {
                func: "keel.account.list".into(),
                params: json!({}),
                request_id: None,
                idempotency_key: None,
                run_as: None,
            }
        );
        assert_eq!(
            Frame::parse(br#"{"describe":"keel.account.list"}"#).unwrap(),
            Frame::describe("keel.account.list")
        );
        assert_eq!(
            Frame::parse(br#"{"i18n":" RU "}"#).unwrap(),
            Frame::i18n("ru")
        );
        assert_eq!(Frame::parse(br#"{"boot":true}"#).unwrap(), Frame::Boot);
    }

    #[test]
    fn a_frame_round_trips_through_its_line() {
        for frame in [
            Frame::Call {
                func: "probe.ping".into(),
                params: json!({ "a": [1, "x"] }),
                request_id: Some("req-7".into()),
                idempotency_key: None,
                run_as: Some("alice".into()),
            },
            Frame::describe("probe.ping"),
            Frame::i18n("en"),
            Frame::Boot,
        ] {
            let line = frame.to_line().unwrap();
            assert!(line.ends_with('\n'), "{line:?}");
            assert!(!line[..line.len() - 1].contains('\n'), "{line:?}");
            assert_eq!(Frame::parse(line.trim_end().as_bytes()).unwrap(), frame);
        }
    }

    #[test]
    fn a_line_that_is_not_json_or_not_one_frame_kind_is_refused() {
        for line in [b"{not json".as_slice(), b"", b"   "] {
            let error = Frame::parse(line).unwrap_err();
            assert_eq!(error.code(), ErrorCode::Validation, "{line:?}");
            assert_eq!(error.key(), "keel.error.body_not_json", "{line:?}");
        }
        for line in [
            br#"{}"#.as_slice(),
            br#"{"params":{}}"#,
            br#"{"func":"a.b.c","describe":"a.b.c"}"#,
            br#"{"describe":"a.b.c","i18n":"en"}"#,
            br#"{"i18n":"en","params":{}}"#,
            br#"{"boot":false}"#,
            br#"{"boot":true,"i18n":"en"}"#,
            br#"{"func":"a.b.c","extra":1}"#,
            br#"{"func":1}"#,
            br#"{"describe":"a.b.c","as":"alice"}"#,
            br#"{"boot":true,"as":"alice"}"#,
            br#"[1,2]"#,
        ] {
            let error = Frame::parse(line).unwrap_err();
            assert_eq!(error.code(), ErrorCode::Validation, "{line:?}");
            assert_eq!(error.key(), "keel.error.frame_unknown", "{line:?}");
        }
    }
}
