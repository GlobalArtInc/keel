use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::types::LicenseState;

use super::grant::{Grant, boot_license};
use super::lease::{self, Lease};
use super::{GET, INSTALL, MENU_ICON};

const LEASE_LIMIT: u32 = 65_536;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Nothing {}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Install {
    #[schemars(length(min = 1, max = LEASE_LIMIT))]
    lease: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
struct Status {
    state: LicenseState,
    instance_id: Option<Id>,
    products: Vec<Product>,
    issued_at: Option<DateTime<Utc>>,
    expires_at: Option<DateTime<Utc>>,
    grace_ends_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Product {
    id: String,
    edition: String,
}

impl Status {
    fn of(lease: Option<&Lease>, instance_id: Option<Id>, now: DateTime<Utc>) -> Self {
        let boot = boot_license(lease, now);
        Self {
            state: boot.state,
            instance_id,
            products: lease
                .map(|lease| {
                    lease
                        .products
                        .iter()
                        .map(|leased| Product {
                            id: leased.id.clone(),
                            edition: leased.edition.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            issued_at: lease.map(|lease| lease.issued_at),
            expires_at: boot.expires_at,
            grace_ends_at: boot.grace_ends_at,
        }
    }
}

pub(super) fn get(grant: Arc<Grant>) -> FunctionDef {
    function::<Nothing, Status>(GET)
        .kind(Kind::Get)
        .level(Level::Admin)
        .unowned()
        .ui(ObjectScreen::new("license.get.title")
            .action(
                Action::new("install", ActionRole::Button)
                    .screen(INSTALL)
                    .icon(MENU_ICON),
            )
            .action(Action::new("refresh", ActionRole::Refresh)))
        .handler(move |_: Call<Nothing>| {
            let grant = grant.clone();
            async move {
                Ok(Status::of(
                    grant.lease().as_ref(),
                    grant.instance(),
                    grant.now()?,
                ))
            }
        })
}

pub(super) fn install(grant: Arc<Grant>) -> FunctionDef {
    function::<Install, Status>(INSTALL)
        .kind(Kind::Form)
        .level(Level::Admin)
        .unowned()
        .exempt_from_read_only()
        .ui(Form::new("license.install.title"))
        .handler(move |call: Call<Install>| {
            let grant = grant.clone();
            async move {
                let signed = &call.params().lease;
                let lease = grant.open(signed)?;
                lease.replaces(grant.lease().as_ref())?;
                lease::store(call.tx()?, signed).await?;
                let status = Status::of(Some(&lease), grant.instance(), grant.now()?);
                call.after_commit(move |context| async move { grant.load(context.db()?).await });
                Ok(status)
            }
        })
}
