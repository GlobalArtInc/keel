use keel_sdk::prelude::*;

use crate::dispatcher::idempotency;

pub const PRUNE: &str = "keel.idempotency.prune";

#[derive(Deserialize, Serialize, JsonSchema)]
pub(super) struct Nothing {}

#[derive(Serialize, JsonSchema)]
pub(super) struct Pruned {
    pub(super) pruned: u64,
}

pub(super) fn prune() -> FunctionDef {
    function::<Nothing, Pruned>(PRUNE)
        .kind(Kind::Action)
        .level(Level::Owner)
        .unowned()
        .internal()
        .handler(|call: Call<Nothing>| async move {
            let now = call.require::<dyn Clock>()?.now();
            let pruned = idempotency::prune(call.tx()?, now).await?;
            Ok(Pruned { pruned })
        })
}

pub(super) fn hours() -> SettingDef {
    SettingDef::new(idempotency::HOURS_SETTING, idempotency::hours_schema())
        .default(idempotency::DEFAULT_HOURS)
}
