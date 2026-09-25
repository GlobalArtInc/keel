use std::sync::Arc;

use keel_sdk::prelude::*;
use keel_sdk::types::LicenseState;

use crate::notify::api::{ProblemSource, Raise, Severity};

use super::grant::Grant;

pub const EXPIRING: &str = "license.expiring";
pub const EXPIRED: &str = "license.expired";
pub const PRODUCT_UNLISTED: &str = "license.product_unlisted";
pub(super) const SOURCE: &str = "license.lease";
const SUBJECT: &str = "lease";

pub(super) fn source(grant: Arc<Grant>) -> ProblemSource {
    ProblemSource::new(SOURCE, move |_db| {
        let grant = grant.clone();
        async move { reported(&grant) }
    })
}

fn reported(grant: &Grant) -> Result<Vec<Raise>> {
    let Some(lease) = grant.lease() else {
        return Ok(Vec::new());
    };
    let dates = json!({
        "expiresAt": lease.expires_at,
        "graceEndsAt": lease.grace_ends_at(),
    });
    let mut raised = Vec::new();
    match lease.state(grant.now()?) {
        LicenseState::Grace => {
            raised.push(Raise::new(EXPIRING, SUBJECT, Severity::Warning).params(dates)?);
        }
        LicenseState::ReadOnly => {
            raised.push(Raise::new(EXPIRED, SUBJECT, Severity::Critical).params(dates)?);
        }
        LicenseState::None | LicenseState::Valid => {}
    }
    for product in grant.products() {
        if lease.edition(product.id).is_none() {
            raised.push(
                Raise::new(PRODUCT_UNLISTED, product.id, Severity::Warning)
                    .params(json!({ "product": product.id }))?,
            );
        }
    }
    Ok(raised)
}
