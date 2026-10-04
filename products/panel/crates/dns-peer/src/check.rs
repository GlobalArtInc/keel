use keel::prelude::*;
use keel::sdk::tokio;
use keel_hosting_users::api::HostingUsers;
use keel_hosting_web::api::Sites;

use crate::Peer;
use crate::neighbour::CHECK_TIMEOUT;

const DNS_ZONE: &str = "dnsZone";
const DOMAIN: &str = "domain";
const ALIASES: &str = "aliases";
const USER: &str = "user";

pub(crate) async fn zone(call: Call<Value>, peer: Arc<Peer>) -> Result<Before> {
    let params = call.params();
    if params[DNS_ZONE].as_bool() != Some(true) {
        return Ok(Before::Proceed);
    }
    let (Some(domain), Some(user)) = (
        params[DOMAIN].as_str(),
        params[USER]
            .as_str()
            .and_then(|user| user.parse::<Id>().ok()),
    ) else {
        return Ok(Before::Proceed);
    };
    let db = call.db();
    let Some(owner) = call.require::<dyn HostingUsers>()?.get(db, &user).await? else {
        return Ok(Before::Proceed);
    };
    if owner.home.is_none() || !call.account_scope().includes(db, owner.account).await? {
        return Ok(Before::Proceed);
    }
    let name = domain.to_ascii_lowercase();
    let names: Vec<String> = std::iter::once(name.clone())
        .chain(
            params[ALIASES]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_ascii_lowercase),
        )
        .collect();
    if call
        .require::<dyn Sites>()?
        .taken(db, &names)
        .await?
        .is_some()
    {
        return Ok(Before::Proceed);
    }
    let neighbour = peer.neighbour(db, &name, true).await?;
    let customer = peer.customer(owner.account)?;
    match tokio::time::timeout(CHECK_TIMEOUT, neighbour.check(&customer, &name)).await {
        Ok(checked) => checked?,
        Err(_) => return Err(neighbour.timed_out()),
    }
    Ok(Before::Proceed)
}
