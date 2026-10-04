use std::collections::BTreeSet;

use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::Expr;
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use keel_sdk::serde_json;
use keel_sdk::system::Caller;
use keel_sdk::types::{InstanceInfo, PROTOCOL};

use super::entity::{pair_code, product};
use super::inbox;
use super::pairing::compatible;
use super::remote::Remote;
use crate::cron::api::{CronJob, JobRun, JobRunExt};
use crate::notify::api::{Problems, Raise, Severity};

pub const JOB_ID: &str = "peer.check";
pub const SCHEDULE: &str = "40 3 * * *";
pub const PROBLEM_INSTANCE_KEY: &str = "peer.problem.instance_key";
pub const PROBLEM_PROTOCOL: &str = "peer.problem.protocol";
pub const SOURCE: &str = "peer.check";

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Verdict {
    Refused,
    Incompatible,
    Compatible,
}

pub(super) fn verdict(row: &product::Model, presented: &InstanceInfo) -> Verdict {
    if presented.instance_id.to_string() != row.instance_id
        || presented.public_key != row.public_key
    {
        Verdict::Refused
    } else if compatible(presented.protocol) {
        Verdict::Compatible
    } else {
        Verdict::Incompatible
    }
}

pub(super) fn job() -> CronJob {
    CronJob::new(JOB_ID, SCHEDULE, |run| async move { check(&run).await })
}

async fn check(run: &JobRun) -> Result<()> {
    let db = run.db();
    let caller = run.caller();
    let now = run.scheduled_at();
    pair_code::Entity::delete_many()
        .filter(pair_code::Column::ExpiresAt.lte(now))
        .exec(&db)
        .await?;
    inbox::prune(&db, now).await?;
    let links = product::Entity::find()
        .order_by_asc(product::Column::Id)
        .all(&db)
        .await?;
    let linked: BTreeSet<String> = links.iter().map(|row| row.id.clone()).collect();
    for problem in Problems::open(&db, SOURCE).await? {
        if !linked.contains(&problem.subject) {
            Problems::clear(caller.as_ref(), &problem.key, &problem.subject).await?;
        }
    }
    for row in links {
        let Some(url) = row.url.clone() else {
            continue;
        };
        let presented = match Remote::new(&url)?.handshake().await {
            Ok(presented) => presented,
            Err(error) => {
                tracing::warn!(peer = %row.id, %url, error = %error, "the daily check did not reach the peer");
                continue;
            }
        };
        let params = json!({ "url": url, "instanceId": row.instance_id });
        match verdict(&row, &presented) {
            Verdict::Refused => {
                raise(caller.as_ref(), PROBLEM_INSTANCE_KEY, &row.id, params).await?;
                continue;
            }
            Verdict::Incompatible => {
                let mut params = params;
                params["protocol"] = json!(presented.protocol);
                params["supported"] = json!(PROTOCOL);
                raise(caller.as_ref(), PROBLEM_PROTOCOL, &row.id, params).await?;
            }
            Verdict::Compatible => {
                Problems::clear(caller.as_ref(), PROBLEM_PROTOCOL, &row.id).await?;
            }
        }
        Problems::clear(caller.as_ref(), PROBLEM_INSTANCE_KEY, &row.id).await?;
        product::Entity::update_many()
            .col_expr(
                product::Column::ProductsJson,
                Expr::value(serde_json::to_value(&presented.products)?),
            )
            .col_expr(
                product::Column::KeelVersion,
                Expr::value(presented.keel.as_str()),
            )
            .col_expr(
                product::Column::Protocol,
                Expr::value(i32::try_from(presented.protocol).unwrap_or(i32::MAX)),
            )
            .col_expr(product::Column::LastSeenAt, Expr::value(now))
            .filter(product::Column::Id.eq(row.id.as_str()))
            .exec(&db)
            .await?;
    }
    Ok(())
}

pub(super) async fn clear(caller: &dyn Caller, link: &str) -> Result<()> {
    for key in [PROBLEM_INSTANCE_KEY, PROBLEM_PROTOCOL] {
        Problems::clear(caller, key, link).await?;
    }
    Ok(())
}

async fn raise(caller: &dyn Caller, key: &str, subject: &str, params: Value) -> Result<()> {
    Problems::raise(
        caller,
        Raise::new(key, subject, Severity::Warning)
            .params(params)?
            .source(SOURCE),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use keel_sdk::chrono::Utc;

    use super::*;

    fn row() -> product::Model {
        product::Model {
            id: "01J9B00000000000000000000B".into(),
            instance_id: "01J9A00000000000000000000A".into(),
            products_json: json!(["dns"]),
            keel_version: "0.4.0".into(),
            protocol: 1,
            url: Some("https://dns.example".into()),
            public_key: "ed25519:pinned".into(),
            inbound_token_hash: "hash".into(),
            inbound_scopes_json: json!({ "functions": [], "level": "service" }),
            outbound_token_enc: "sealed".into(),
            webhook_secret_enc: None,
            status: "active".into(),
            last_seen_at: None,
            created_at: Utc::now(),
            principal_id: None,
            events: Vec::new(),
            pull_seq: None,
            feed_seq: 0,
        }
    }

    fn presented(instance: &str, key: &str, protocol: u32) -> InstanceInfo {
        InstanceInfo {
            instance_id: instance.parse().unwrap(),
            keel: "0.4.1".into(),
            protocol,
            products: vec!["dns".into()],
            public_key: key.into(),
            features: Vec::new(),
            service_kinds: Vec::new(),
        }
    }

    #[test]
    fn another_key_or_instance_is_refused_and_a_far_protocol_is_incompatible() {
        let row = row();
        let instance = "01J9A00000000000000000000A";
        assert_eq!(
            verdict(&row, &presented(instance, "ed25519:pinned", PROTOCOL)),
            Verdict::Compatible
        );
        assert_eq!(
            verdict(&row, &presented(instance, "ed25519:other", PROTOCOL)),
            Verdict::Refused
        );
        assert_eq!(
            verdict(
                &row,
                &presented("01J9C00000000000000000000C", "ed25519:pinned", PROTOCOL)
            ),
            Verdict::Refused
        );
        assert_eq!(
            verdict(&row, &presented(instance, "ed25519:pinned", PROTOCOL + 2)),
            Verdict::Incompatible
        );
        assert_eq!(
            verdict(&row, &presented(instance, "ed25519:other", PROTOCOL + 2)),
            Verdict::Refused
        );
    }
}
