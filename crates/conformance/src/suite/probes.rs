use keel_types::Id;
use serde_json::{Value, json};

use super::{Suite, id_of};
use crate::client::{Answer, Credential};
use crate::report::{Failure, ensure};

pub const SET: &str = "webhook.subscription.set";
pub const LIST: &str = "webhook.subscription.list";
pub const DELETE: &str = "webhook.subscription.delete";
pub const SAVED: &str = "webhook.subscription.saved";
pub const DELETED: &str = "webhook.subscription.deleted";
pub const MASK: &str = "webhook.subscription.*";
pub const SILENT_MASK: &str = "conformance.silent.*";
pub const SINK: &str = "http://127.0.0.1:9/conformance";

pub fn probe(name: &str) -> Value {
    json!({ "name": name, "url": SINK, "events": [SILENT_MASK], "enabled": false })
}

pub async fn save(suite: &mut Suite, params: Value) -> Result<Answer, Failure> {
    suite
        .client
        .call(SET, params, Credential::Session, &[])
        .await
}

pub fn saved(answer: Result<Answer, Failure>) -> Result<Id, Failure> {
    let answer = answer?.confidential();
    let (data, _) = answer.data::<Value>()?;
    id_of(&data, "id").map_err(|failure| answer.failure(failure.0))
}

pub async fn create(suite: &mut Suite, name: &str) -> Result<Id, Failure> {
    saved(save(suite, probe(name)).await)
}

pub async fn rename(suite: &mut Suite, id: Id, name: &str) -> Result<Id, Failure> {
    let mut params = probe(name);
    params["id"] = json!(id);
    let edited = saved(save(suite, params).await)?;
    ensure(edited == id, || {
        format!("editing the probe {id} answered another probe {edited}")
    })?;
    Ok(edited)
}

pub async fn delete(suite: &mut Suite, ids: &[Id]) -> Result<(), Failure> {
    for id in ids {
        let answer = suite
            .client
            .call(DELETE, json!({ "id": id }), Credential::Session, &[])
            .await?;
        let (data, _) = answer.data::<Value>()?;
        ensure(data["deleted"] == json!(true), || {
            answer.failure("data.deleted is not true").0
        })?;
    }
    Ok(())
}
