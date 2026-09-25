use std::sync::Arc;

use keel_sdk::runtime::CallHandle;
use keel_sdk::system::TaskSpec;
use keel_sdk::{FunctionDef, KeelError, TaskHandle};
use keel_types::{ScreenResult, TaskRef};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{Executed, Inner, Origin, Target, enrich, idempotency, spawn_after_commit};

pub const BATCH_LOG_PREFIX: &str = "chunk";

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub(super) struct BatchCheckpoint {
    pub done: usize,
    pub results: Vec<Value>,
}

pub(super) struct Chunks {
    key: String,
    ids: Vec<Value>,
    size: usize,
}

pub(super) fn chunks(def: &FunctionDef, key: Option<&str>, params: &Value) -> Option<Chunks> {
    let threshold = usize::try_from(def.limits().batch_threshold?).ok()?;
    let key = key?;
    let ids = params.get(key)?.as_array()?;
    if threshold == 0 || ids.len() <= threshold {
        return None;
    }
    Some(Chunks {
        key: key.to_owned(),
        ids: ids.clone(),
        size: threshold,
    })
}

fn answers_task(target: &Target<'_>) -> bool {
    target
        .function
        .descriptor()
        .ui
        .as_ref()
        .is_some_and(|ui| ui.result == Some(ScreenResult::Task))
}

impl Inner {
    pub(super) async fn entrance(
        self: &Arc<Self>,
        target: &Target<'_>,
        pool: &CallHandle,
        params: &Value,
        batch_key: Option<&str>,
        extra: &Map<String, Value>,
        idempotency: Option<&(String, String)>,
    ) -> Result<Option<Executed>, KeelError> {
        let Some(runner) = &self.runner else {
            return Ok(None);
        };
        let def = target.function.def();
        if !answers_task(target) && chunks(def, batch_key, params).is_none() {
            return Ok(None);
        }
        let principal = pool.principal();
        let request_id = pool.request_id();
        let stored = if extra.is_empty() {
            params.clone()
        } else {
            enrich(params.clone(), Value::Object(extra.clone())).unwrap_or_else(|| params.clone())
        };
        let id = runner
            .spawn(TaskSpec {
                function: def.name().to_owned(),
                params: stored,
                principal: principal.clone(),
                class: def.class().to_owned(),
                resumable: def.is_resumable(),
                request_id: Some(request_id.to_owned()),
            })
            .await?;
        let data = serde_json::to_value(TaskRef::new(id))?;
        if let Some((key, hash)) = idempotency {
            idempotency::record(
                &self.db,
                principal.id,
                key,
                def.name(),
                hash,
                &data,
                self.clock.now(),
            )
            .await?;
        }
        Ok(Some(Executed {
            data,
            replay: false,
            after: Vec::new(),
            context: None,
            session: pool.take_session_change(),
            download: None,
            lines: None,
        }))
    }

    pub(super) async fn chunked(
        self: &Arc<Self>,
        target: &Target<'_>,
        pool: &CallHandle,
        params: Value,
        chunks: Chunks,
        task: TaskHandle,
    ) -> Result<Executed, KeelError> {
        let total = chunks.ids.len();
        let count = total.div_ceil(chunks.size);
        let resumed = task.resume_from::<BatchCheckpoint>()?.unwrap_or_default();
        let mut results = resumed.results;
        let mut processed = resumed.done.saturating_mul(chunks.size).min(total);
        for (index, chunk) in chunks
            .ids
            .chunks(chunks.size)
            .enumerate()
            .skip(resumed.done)
        {
            if task.is_cancelled() {
                return Err(task.cancelled());
            }
            let mut chunk_params = params.clone();
            if let Value::Object(object) = &mut chunk_params {
                object.insert(chunks.key.clone(), Value::Array(chunk.to_vec()));
            }
            let executed = Box::pin(self.execute(
                target,
                pool.principal(),
                pool.request_id(),
                chunk_params,
                None,
                Origin {
                    client: pool.client().cloned(),
                    tz: pool.tz(),
                    task: Some(task.clone()),
                },
            ))
            .await?;
            if let Some(context) = executed.context
                && !executed.after.is_empty()
            {
                spawn_after_commit(executed.after, context);
            }
            results.push(executed.data);
            processed += chunk.len();
            task.progress(processed as u64, Some(total as u64), None)
                .await?;
            task.log(&format!(
                "{BATCH_LOG_PREFIX} {}/{count}: {} ids",
                index + 1,
                chunk.len()
            ))
            .await?;
            task.checkpoint(&BatchCheckpoint {
                done: index + 1,
                results: results.clone(),
            })
            .await?;
        }
        Ok(Executed {
            data: Value::Array(results),
            replay: false,
            after: Vec::new(),
            context: None,
            session: None,
            download: None,
            lines: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use keel_sdk::function;
    use keel_sdk::prelude::*;

    use super::*;

    #[derive(Deserialize, Serialize, JsonSchema)]
    struct Batch {
        ids: Vec<Id>,
    }

    #[derive(Deserialize, Serialize, JsonSchema)]
    struct Nothing {}

    fn batch(threshold: Option<u32>) -> FunctionDef {
        let builder = function::<Batch, Nothing>("t.item.delete")
            .kind(Kind::Action)
            .level(Level::User)
            .key("ids")
            .unowned();
        match threshold {
            Some(threshold) => builder.batch_threshold(threshold),
            None => builder,
        }
        .handler(|_: Call<Batch>| async { Ok(Nothing {}) })
    }

    fn ids(count: usize) -> Value {
        Value::Array(
            (0..count)
                .map(|_| Value::String(Id::generate().to_string()))
                .collect(),
        )
    }

    #[test]
    fn a_key_array_above_the_threshold_is_chunked_and_one_at_or_below_it_is_not() {
        let def = batch(Some(3));
        let key = def.key();
        let above = chunks(&def, key, &json!({ "ids": ids(7) })).expect("seven ids over three");
        assert_eq!(above.key, "ids");
        assert_eq!(above.size, 3);
        assert_eq!(above.ids.len(), 7);
        assert!(chunks(&def, key, &json!({ "ids": ids(3) })).is_none());
        assert!(chunks(&def, key, &json!({ "ids": ids(1) })).is_none());
        assert!(chunks(&def, key, &json!({ "ids": "one" })).is_none());
        assert!(chunks(&def, key, &json!({})).is_none());
        assert!(chunks(&def, None, &json!({ "ids": ids(7) })).is_none());
        assert!(chunks(&def, Some("rows"), &json!({ "ids": ids(7) })).is_none());
        assert!(chunks(&def, Some("rows"), &json!({ "rows": ids(7) })).is_some());
        assert!(chunks(&batch(None), key, &json!({ "ids": ids(7) })).is_none());
        assert!(chunks(&batch(Some(0)), key, &json!({ "ids": ids(7) })).is_none());
    }

    #[test]
    fn a_batch_checkpoint_round_trips_and_starts_empty() {
        let checkpoint = BatchCheckpoint {
            done: 2,
            results: vec![json!({ "deleted": 3 })],
        };
        let value = serde_json::to_value(&checkpoint).unwrap();
        assert_eq!(value, json!({ "done": 2, "results": [{ "deleted": 3 }] }));
        assert_eq!(
            serde_json::from_value::<BatchCheckpoint>(value).unwrap(),
            checkpoint
        );
        assert_eq!(BatchCheckpoint::default().done, 0);
    }
}
