use keel::prelude::*;
use keel_system::notify::api::{ProblemSource, Raise, Severity};

use crate::Peer;

pub const FAILED: &str = "dns_peer.zone.failed";

#[derive(Serialize)]
#[serde(crate = "keel::sdk::serde")]
struct Failed {
    zone: String,
    error: String,
}

pub(crate) fn source(peer: Arc<Peer>) -> ProblemSource {
    ProblemSource::new(FAILED, move |db| {
        let peer = Arc::clone(&peer);
        async move { failed(&peer, &db).await }
    })
}

async fn failed(peer: &Peer, db: &Db) -> Result<Vec<Raise>> {
    let mut raises = Vec::new();
    for failure in peer.sites()?.zone_failures(db).await? {
        raises.push(
            Raise::new(FAILED, &failure.site.to_string(), Severity::Critical)
                .params(Failed {
                    zone: failure.domain,
                    error: failure.error,
                })?
                .account(failure.account),
        );
    }
    Ok(raises)
}
