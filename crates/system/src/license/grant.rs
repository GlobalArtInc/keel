use std::sync::{Mutex, OnceLock, PoisonError, RwLock};

use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::system::{ContextExt, Degradation, Features, Installation, LicenseKey};
use keel_sdk::types::{BootLicense, LicenseState};

use super::clock;
use super::lease::{self, Lease};

pub(super) const ERROR_NOT_STARTED: &str = "license.error.not_started";

#[derive(Default)]
pub(super) struct Grant {
    started: OnceLock<Started>,
    lease: RwLock<Option<Lease>>,
    seen: Mutex<Option<DateTime<Utc>>>,
}

struct Started {
    installation: Arc<Installation>,
    key: Arc<dyn LicenseKey>,
    clock: Arc<dyn Clock>,
}

impl Grant {
    pub(super) async fn start(&self, ctx: &Context) -> Result<()> {
        let started = Started {
            installation: ctx.require::<Installation>()?,
            key: ctx.require::<dyn LicenseKey>()?,
            clock: ctx.require::<dyn Clock>()?,
        };
        if self.started.set(started).is_err() {
            tracing::warn!("the license module was started twice; the first start stands");
        }
        let db = ctx.db();
        self.advance(&db).await?;
        self.load(&db).await
    }

    pub(super) async fn advance(&self, db: &Db) -> Result<()> {
        let stored = clock::advance(db, self.now()?).await?;
        self.observe(stored);
        Ok(())
    }

    pub(super) async fn load(&self, db: &Db) -> Result<()> {
        let started = self.started()?;
        let lease = match lease::stored(db).await? {
            Some(signed) => {
                match Lease::open(&signed, started.key.as_ref(), &started.installation) {
                    Ok(lease) => Some(lease),
                    Err(error) => {
                        tracing::warn!(
                            key = error.key(),
                            params = ?error.params(),
                            "the stored lease no longer holds; the installation runs without one"
                        );
                        None
                    }
                }
            }
            None => None,
        };
        *self.lease.write().unwrap_or_else(PoisonError::into_inner) = lease;
        Ok(())
    }

    pub(super) fn open(&self, signed: &str) -> Result<Lease> {
        let started = self.started()?;
        Lease::open(signed, started.key.as_ref(), &started.installation)
    }

    pub(super) fn lease(&self) -> Option<Lease> {
        self.lease
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(super) fn instance(&self) -> Option<Id> {
        self.started
            .get()
            .and_then(|started| started.installation.instance)
    }

    pub(super) fn now(&self) -> Result<DateTime<Utc>> {
        Ok(self.observe(self.started()?.clock.now()))
    }

    fn observe(&self, at: DateTime<Utc>) -> DateTime<Utc> {
        let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        let latest = seen.map_or(at, |seen| seen.max(at));
        *seen = Some(latest);
        latest
    }

    fn started(&self) -> Result<&Started> {
        self.started
            .get()
            .ok_or_else(|| KeelError::internal(ERROR_NOT_STARTED))
    }

    pub(super) fn products(&self) -> &[&'static ProductManifest] {
        self.started
            .get()
            .map(|started| started.installation.products.as_slice())
            .unwrap_or_default()
    }
}

fn edition_of(lease: Option<&Lease>, product: &ProductManifest) -> Option<&'static Edition> {
    match lease {
        Some(lease) => lease
            .edition(product.id)
            .and_then(|id| product.editions.iter().find(|edition| edition.id == id)),
        None => product.default_edition(),
    }
}

fn features_of(lease: Option<&Lease>, products: &[&'static ProductManifest]) -> Vec<String> {
    let mut features: Vec<String> = Vec::new();
    let granted = products
        .iter()
        .filter_map(|product| edition_of(lease, product))
        .flat_map(|edition| edition.features);
    for feature in granted {
        if !features.iter().any(|known| known == feature) {
            features.push((*feature).to_owned());
        }
    }
    features
}

pub(super) fn boot_license(lease: Option<&Lease>, now: DateTime<Utc>) -> BootLicense {
    match lease {
        Some(lease) => BootLicense {
            state: lease.state(now),
            expires_at: Some(lease.expires_at),
            grace_ends_at: Some(lease.grace_ends_at()),
        },
        None => BootLicense {
            state: LicenseState::None,
            expires_at: None,
            grace_ends_at: None,
        },
    }
}

impl Features for Grant {
    fn enabled(&self) -> Vec<String> {
        features_of(self.lease().as_ref(), self.products())
    }

    fn allows(&self, feature: &str) -> bool {
        self.enabled().iter().any(|known| known == feature)
    }

    fn edition(&self, product: &ProductManifest) -> Option<String> {
        edition_of(self.lease().as_ref(), product).map(|edition| edition.id.to_owned())
    }

    fn license(&self) -> Option<BootLicense> {
        let now = self.now().ok()?;
        Some(boot_license(self.lease().as_ref(), now))
    }

    fn degradation(&self, module: &str) -> Option<Degradation> {
        let lease = self.lease()?;
        if lease.state(self.now().ok()?) == LicenseState::ReadOnly {
            return Some(Degradation::ReadOnly);
        }
        let carriers: Vec<&ProductManifest> = self
            .products()
            .iter()
            .copied()
            .filter(|product| product.module_ids().any(|id| id == module))
            .collect();
        if carriers
            .iter()
            .any(|product| lease.edition(product.id).is_some())
        {
            return None;
        }
        carriers.first().map(|product| Degradation::Unlisted {
            product: product.id.to_owned(),
        })
    }

    fn may_enable(&self, product: &str) -> bool {
        self.lease()
            .is_none_or(|lease| lease.edition(product).is_some())
    }
}
