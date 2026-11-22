use std::error::Error;
use std::io;
use std::time::Duration;

use serde_json::Value;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::Directive;

use crate::protocol::{Failure, Request};

const AUDIT: &str = "keel_host_agent::audit";
const DETAIL_LIMIT: usize = 1024;

#[derive(Debug, thiserror::Error)]
#[error("the journal could not be initialised: {0}")]
pub struct JournalError(#[source] Box<dyn Error + Send + Sync>);

pub fn init() -> Result<(), JournalError> {
    tracing_subscriber::fmt()
        .with_env_filter(filter(EnvFilter::try_from_default_env().ok())?)
        .with_writer(io::stderr)
        .with_ansi(false)
        .try_init()
        .map_err(JournalError)
}

fn filter(from_env: Option<EnvFilter>) -> Result<EnvFilter, JournalError> {
    let audit: Directive = format!("{AUDIT}=info")
        .parse()
        .map_err(|error| JournalError(Box::new(error)))?;
    Ok(from_env
        .unwrap_or_else(|| EnvFilter::new("info"))
        .add_directive(audit))
}

pub(crate) fn operation(
    peer: u32,
    request: &Request,
    class: Option<&str>,
    outcome: &Result<Value, Failure>,
    elapsed: Duration,
) {
    let elapsed_ms = elapsed.as_millis();
    match outcome {
        Ok(_) => tracing::info!(
            target: AUDIT,
            op = %request.op,
            actor = %request.actor.login,
            level = %request.actor.level,
            request_id = %request.request_id,
            product = %request.product,
            class = class.map(tracing::field::display),
            peer_uid = peer,
            outcome = %"ok",
            elapsed_ms,
        ),
        Err(failure) => tracing::warn!(
            target: AUDIT,
            op = %request.op,
            actor = %request.actor.login,
            level = %request.actor.level,
            request_id = %request.request_id,
            product = %request.product,
            class = class.map(tracing::field::display),
            peer_uid = peer,
            outcome = %failure.key,
            detail = ?clip(&failure.detail),
            elapsed_ms,
        ),
    }
}

pub(crate) struct Stop<'a> {
    pub(crate) op: &'a str,
    pub(crate) actor: &'a str,
    pub(crate) request_id: &'a str,
    pub(crate) product: &'a str,
    pub(crate) peer_uid: u32,
    pub(crate) user: &'a str,
    pub(crate) uid: u32,
    pub(crate) sigterm: usize,
    pub(crate) sigkill: usize,
    pub(crate) rounds: usize,
}

pub(crate) fn processes_stopped(stop: &Stop<'_>) {
    tracing::info!(
        target: AUDIT,
        op = %stop.op,
        actor = %stop.actor,
        request_id = %stop.request_id,
        product = %stop.product,
        peer_uid = stop.peer_uid,
        user = %stop.user,
        uid = stop.uid,
        sigterm = stop.sigterm,
        sigkill = stop.sigkill,
        rounds = stop.rounds,
        outcome = %"processes_stopped",
    );
}

pub(crate) fn frame(peer: u32, id: u64, failure: &Failure) {
    tracing::warn!(
        target: AUDIT,
        op = "-",
        actor = "-",
        level = "-",
        request_id = "-",
        product = "-",
        peer_uid = peer,
        id,
        outcome = %failure.key,
        detail = ?clip(&failure.detail),
    );
}

pub(crate) fn refused_peer(peer: Option<u32>, admitted: u32, reason: &str) {
    match peer {
        Some(peer) => tracing::warn!(
            target: AUDIT,
            peer_uid = peer,
            admitted_uid = admitted,
            outcome = %"refused",
            reason = %reason,
        ),
        None => tracing::warn!(
            target: AUDIT,
            peer_uid = "-",
            admitted_uid = admitted,
            outcome = %"refused",
            reason = %reason,
        ),
    }
}

fn clip(detail: &str) -> &str {
    match detail.char_indices().nth(DETAIL_LIMIT) {
        Some((end, _)) => &detail[..end],
        None => detail,
    }
}

#[cfg(test)]
mod tests {
    use tracing::Level;
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;

    #[test]
    fn the_audit_target_stays_on_whatever_the_environment_filters() {
        let filter = filter(Some(EnvFilter::new("error"))).unwrap();
        let subscriber = tracing_subscriber::registry().with(filter);
        tracing::subscriber::with_default(subscriber, || {
            assert!(tracing::enabled!(target: AUDIT, Level::INFO));
            assert!(!tracing::enabled!(target: "keel_host_agent::agent", Level::INFO));
        });
    }
}
