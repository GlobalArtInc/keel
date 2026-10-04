use std::future::{Future, IntoFuture};
use std::marker::PhantomData;
use std::pin::Pin;

use http::header::ACCEPT;
use keel_types::{Envelope, Meta, MetaPrincipal};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::client::{Client, Request, Response, shape};
use crate::credential::{Credential, OnBehalf};
use crate::error::{Error, Refusal};
use crate::{CALL_PATH, META_VERSION};

const JSON: &str = "application/json";

#[derive(Clone, Debug, PartialEq)]
pub struct Answer<T> {
    pub data: T,
    pub meta: Meta,
    pub replayed: bool,
    pub meta_version: Option<String>,
    pub session: Option<Credential>,
}

impl<T> Answer<T> {
    pub fn request_id(&self) -> &str {
        &self.meta.request_id
    }

    pub fn principal(&self) -> Option<&MetaPrincipal> {
        self.meta.principal.as_ref()
    }
}

impl<T: DeserializeOwned> Answer<T> {
    pub(crate) fn read(response: &Response) -> Result<Self, Error> {
        if (300..400).contains(&response.status) {
            return Err(response.outside("a redirect is not followed"));
        }
        let envelope: Envelope<T> = serde_json::from_slice(&response.body).map_err(|error| {
            response.outside(&format!(
                "the body is not an envelope of the expected data: {}",
                shape(&error)
            ))
        })?;
        match envelope {
            Envelope::Ok { data, meta } if response.status == 200 => Ok(Self {
                data,
                meta,
                replayed: response.replayed(),
                meta_version: response.header(META_VERSION).map(str::to_owned),
                session: response.session(),
            }),
            Envelope::Err { error } if response.status == error.code.status() => {
                Err(Error::Refused(Box::new(Refusal {
                    url: response.url.clone(),
                    status: response.status,
                    error,
                })))
            }
            Envelope::Ok { .. } => Err(response.outside("an ok envelope under another status")),
            Envelope::Err { error } => Err(response.outside(&format!(
                "error.code {} belongs to status {}",
                error.code.as_str(),
                error.code.status()
            ))),
        }
    }
}

pub struct Call<'a, T> {
    client: &'a Client,
    request: Result<Request, Error>,
    marker: PhantomData<fn() -> T>,
}

fn function_name(func: &str) -> bool {
    func.split('.').all(|segment| {
        !segment.is_empty()
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    })
}

impl<'a, T: DeserializeOwned> Call<'a, T> {
    pub(crate) fn new(client: &'a Client, func: &str, params: impl Serialize) -> Self {
        let request = if function_name(func) {
            Ok(Request::post(&format!("{CALL_PATH}/{func}"), &params).header(ACCEPT, JSON))
        } else {
            Err(Error::invalid(format!("{func:?} is not a function name")))
        };
        Self {
            client,
            request,
            marker: PhantomData,
        }
    }

    pub fn idempotency_key(mut self, key: &str) -> Self {
        self.request = self.request.map(|request| request.idempotency_key(key));
        self
    }

    pub fn credential(mut self, credential: Credential) -> Self {
        self.request = self.request.map(|request| request.credential(credential));
        self
    }

    pub fn on_behalf(mut self, on_behalf: OnBehalf) -> Self {
        self.request = self.request.map(|request| request.on_behalf(on_behalf));
        self
    }

    pub async fn send(self) -> Result<Answer<T>, Error> {
        let response = self.client.send(self.request?).await?;
        Answer::read(&response)
    }
}

impl<'a, T: DeserializeOwned + Send + 'a> IntoFuture for Call<'a, T> {
    type Output = Result<Answer<T>, Error>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.send())
    }
}

#[cfg(test)]
mod tests {
    use http::header::SET_COOKIE;
    use http::{HeaderMap, HeaderValue};
    use keel_types::{ErrorCode, Page};
    use serde_json::{Value, json};

    use super::*;

    fn response(status: u16, headers: &[(&'static str, &'static str)], body: Value) -> Response {
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            map.append(*name, HeaderValue::from_static(value));
        }
        Response {
            url: "https://dns.example/api/v1/call/keel.account.list".to_owned(),
            status,
            headers: map,
            body: body.to_string().into_bytes(),
        }
    }

    #[test]
    fn an_ok_envelope_under_200_answers_the_typed_data_with_its_meta_and_markers() {
        let ok = response(
            200,
            &[
                ("x-keel-meta-version", "m-0011223344556677"),
                ("x-keel-idempotent-replay", "true"),
                (SET_COOKIE.as_str(), "__Host-keel_session=ks_abc; Path=/"),
            ],
            json!({ "ok": true, "data": { "data": [], "total": 0 }, "meta": { "requestId": "req-1" } }),
        );
        let answer = Answer::<Page<Value>>::read(&ok).unwrap();
        assert_eq!(answer.data.total, 0);
        assert_eq!(answer.request_id(), "req-1");
        assert!(answer.principal().is_none());
        assert!(answer.replayed);
        assert_eq!(answer.meta_version.as_deref(), Some("m-0011223344556677"));
        assert_eq!(answer.session, Some(Credential::session("ks_abc").unwrap()));
        let misfit = Answer::<Vec<u8>>::read(&ok).unwrap_err();
        assert!(
            matches!(misfit, Error::Answer { status: 200, .. }),
            "{misfit}"
        );
    }

    #[test]
    fn an_error_envelope_is_a_refusal_only_under_the_status_of_its_code() {
        let body = json!({ "ok": false, "error": {
            "code": "validation", "key": "keel.error.validation", "requestId": "req-2",
            "fields": [ { "path": "login", "key": "keel.schema.required" } ] } });
        let refused = Answer::<Value>::read(&response(422, &[], body.clone())).unwrap_err();
        assert_eq!(refused.code(), Some(ErrorCode::Validation));
        assert_eq!(
            refused
                .field("login")
                .and_then(|field| field.key.as_deref()),
            Some("keel.schema.required")
        );
        assert_eq!(refused.refusal().unwrap().request_id(), Some("req-2"));
        for status in [200, 403, 500] {
            let error = Answer::<Value>::read(&response(status, &[], body.clone())).unwrap_err();
            assert!(matches!(error, Error::Answer { .. }), "{status}: {error}");
        }
        let ok = json!({ "ok": true, "data": 1, "meta": { "requestId": "r" } });
        assert!(matches!(
            Answer::<Value>::read(&response(201, &[], ok)).unwrap_err(),
            Error::Answer { status: 201, .. }
        ));
        let moved = response(302, &[("location", "https://elsewhere.test/")], json!({}));
        let error = Answer::<Value>::read(&moved).unwrap_err();
        assert!(
            matches!(&error, Error::Answer { status: 302, reason, .. } if reason.contains("redirect")),
            "{error}"
        );
        for garbage in [json!("<html>"), json!({ "ok": true, "data": 1 }), json!({})] {
            assert!(matches!(
                Answer::<Value>::read(&response(502, &[], garbage)).unwrap_err(),
                Error::Answer { status: 502, .. }
            ));
        }
    }

    #[test]
    fn a_body_that_does_not_fit_is_reported_without_its_values() {
        type Narrow = std::collections::BTreeMap<String, u8>;
        let minted = response(
            200,
            &[],
            json!({ "ok": true, "data": { "token": "kat_secret" }, "meta": { "requestId": "r" } }),
        );
        let error = Answer::<Narrow>::read(&minted).unwrap_err().to_string();
        assert!(!error.contains("kat_secret"), "{error}");
        assert!(error.contains("line 1 column"), "{error}");
        let error = minted.json::<Vec<Narrow>>().unwrap_err().to_string();
        assert!(!error.contains("kat_secret"), "{error}");
    }

    #[test]
    fn a_function_name_cannot_leave_the_call_path() {
        assert!(function_name("svc.dns.zones.open"));
        assert!(function_name("hosting.file-manager"));
        for refused in [
            "",
            ".",
            "..",
            "note..edit",
            ".note",
            "note.",
            "../x",
            "note.edit?x=1",
            "note.edit#x",
            "note edit",
            "note/edit",
            "%2e%2e",
        ] {
            assert!(!function_name(refused), "{refused}");
        }
    }
}
