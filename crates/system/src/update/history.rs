use std::fmt;

use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::schemars::JsonSchema;
use keel_sdk::sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use keel_sdk::serde::{Deserialize, Serialize};
use semver::Version;

use super::channel::Channel;
use super::entity::run;

const ERROR_HISTORY: &str = "update.error.history";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "lowercase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "lowercase")]
pub(super) enum RunKind {
    Apply,
    Rollback,
}

impl RunKind {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            RunKind::Apply => "apply",
            RunKind::Rollback => "rollback",
        }
    }
}

impl fmt::Display for RunKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub(super) struct Run<'a> {
    pub(super) kind: RunKind,
    pub(super) from: &'a Version,
    pub(super) to: &'a Version,
    pub(super) channel: Channel,
    pub(super) principal: &'a Principal,
    pub(super) request_id: &'a str,
    pub(super) dump_dir: &'a str,
    pub(super) tables: usize,
    pub(super) at: DateTime<Utc>,
}

pub(super) async fn previous_of(db: &Db, running: &Version) -> Result<Option<Version>> {
    let latest = run::Entity::find()
        .filter(run::Column::ToVersion.eq(running.to_string()))
        .order_by_desc(run::Column::RequestedAt)
        .order_by_desc(run::Column::Id)
        .one(db)
        .await?;
    latest
        .map(|row| {
            row.from_version.parse::<Version>().map_err(|error| {
                KeelError::internal(ERROR_HISTORY)
                    .param("id", row.id.as_str())
                    .param("version", row.from_version.as_str())
                    .source(error)
            })
        })
        .transpose()
}

pub(super) async fn record(db: &Db, run: Run<'_>) -> Result<Id> {
    let id = Id::generate();
    run::ActiveModel {
        id: Set(id.to_string()),
        kind: Set(run.kind.as_str().to_owned()),
        from_version: Set(run.from.to_string()),
        to_version: Set(run.to.to_string()),
        channel: Set(run.channel.as_str().to_owned()),
        requested_by: Set(run.principal.login.clone()),
        principal_id: Set(run.principal.id.to_string()),
        request_id: Set(run.request_id.to_owned()),
        dump_dir: Set(run.dump_dir.to_owned()),
        tables: Set(i32::try_from(run.tables).unwrap_or(i32::MAX)),
        requested_at: Set(run.at),
    }
    .insert(db)
    .await?;
    Ok(id)
}
