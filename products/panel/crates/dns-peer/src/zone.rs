use keel::prelude::*;
use keel::sdk::serde_json;
use keel::sdk::system::{TaskRunner, TaskSpec};
use keel::types::{ErrorCode, Scope};

use crate::Peer;
use crate::neighbour::detail;

pub const CREATE: &str = "dns_peer.zone.create";
pub const CLASS: &str = "dns_peer.zone";
const SITE_GET: &str = "hosting.site.get";
const SITE: &str = "dns_peer.error.site";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct ZoneTask {
    site: Id,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct ZoneCreated {
    site: Id,
    zone: Option<String>,
}

#[derive(Deserialize)]
#[serde(crate = "keel::sdk::serde", rename_all = "camelCase")]
struct SiteView {
    domain: String,
    account_id: Id,
}

pub(crate) fn create(peer: Arc<Peer>) -> FunctionDef {
    function::<ZoneTask, ZoneCreated>(CREATE)
        .kind(Kind::Action)
        .mode(Mode::Read)
        .level(Level::User)
        .unowned()
        .internal()
        .class(CLASS)
        .handler(move |call: Call<ZoneTask>| {
            let peer = Arc::clone(&peer);
            async move { create_zone(&call, &peer).await }
        })
}

pub(crate) async fn spawn(runner: &dyn TaskRunner, ctx: &AfterContext, site: Id) -> Result<()> {
    let mut principal = ctx.principal().clone();
    principal.scope = Scope::Full;
    runner
        .spawn(TaskSpec {
            function: CREATE.to_owned(),
            params: json!({ "site": site }),
            principal,
            class: CLASS.to_owned(),
            resumable: false,
            request_id: Some(ctx.request_id().to_owned()),
        })
        .await
        .map(drop)
}

async fn create_zone(call: &Call<ZoneTask>, peer: &Peer) -> Result<ZoneCreated> {
    let site = call.params().site;
    let Some(view) = site_of(call, site).await? else {
        note(
            call,
            &format!("the site {site} is gone, so no zone is created"),
        )
        .await?;
        return Ok(ZoneCreated { site, zone: None });
    };
    let key = format!("{CREATE}:{site}:{}", call.request_id());
    let created = async {
        let neighbour = peer.neighbour(call.db(), &view.domain, false).await?;
        let customer = peer.customer(view.account_id)?;
        neighbour.create(&customer, &view.domain, &key).await
    }
    .await;
    let sites = peer.sites()?;
    match created {
        Ok(zone) => {
            sites.zone_failed(call.db(), &site, None).await?;
            note(
                call,
                &format!("the zone {} is created on the neighbour", view.domain),
            )
            .await?;
            Ok(ZoneCreated {
                site,
                zone: zone["id"].as_str().map(str::to_owned),
            })
        }
        Err(error) => {
            let detail = detail(&error);
            sites.zone_failed(call.db(), &site, Some(&detail)).await?;
            note(
                call,
                &format!("the zone {} was not created: {detail}", view.domain),
            )
            .await?;
            Err(error)
        }
    }
}

async fn site_of(call: &Call<ZoneTask>, site: Id) -> Result<Option<SiteView>> {
    match call.invoke(SITE_GET, json!({ "id": site })).await {
        Ok(value) => serde_json::from_value(value).map(Some).map_err(|error| {
            KeelError::internal(SITE)
                .param("site", site.to_string())
                .source(error)
        }),
        Err(error) if error.code() == ErrorCode::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

async fn note(call: &Call<ZoneTask>, line: &str) -> Result<()> {
    match call.task() {
        Some(task) => task.log(line).await,
        None => Ok(()),
    }
}
