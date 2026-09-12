use std::fmt;

use keel_kernel::{Composition, Config, Storage, products};
use keel_sdk::ProductBundle;
use tokio::net::UnixStream;

use crate::error::Error;
use crate::runtime::Runtime;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProductState {
    pub id: String,
    pub version: Option<&'static str>,
    pub enabled: bool,
}

pub(crate) struct Enabled {
    pub(crate) kernel: Vec<String>,
    pub(crate) ids: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Enabled,
    AlreadyEnabled,
    Disabled,
    AlreadyDisabled,
}

impl fmt::Display for ProductState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = if self.enabled { "enabled" } else { "disabled" };
        match self.version {
            Some(version) => write!(f, "{}\t{state}\t{version}", self.id),
            None => write!(f, "{}\t{state}\tnot carried by this binary", self.id),
        }
    }
}

impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Change::Enabled => "enabled",
            Change::AlreadyEnabled => "already enabled",
            Change::Disabled => "disabled",
            Change::AlreadyDisabled => "already disabled",
        })
    }
}

impl Runtime {
    pub(crate) fn bundle(&self, id: &str) -> Option<&'static ProductBundle> {
        self.products.iter().copied().find(|bundle| bundle.id == id)
    }

    pub(crate) fn known_ids(&self) -> Vec<String> {
        self.products
            .iter()
            .map(|bundle| bundle.id.to_owned())
            .collect()
    }

    pub(crate) fn composition(&self, enabled: &[String]) -> Result<Composition, Error> {
        let mut composition = Composition::new();
        let mut composed: Vec<&'static str> = Vec::new();
        for module in &self.modules {
            composed.push(module.id);
            composition = composition.module(module);
        }
        for id in enabled {
            let bundle = self.bundle(id).ok_or_else(|| Error::UnknownProduct {
                id: id.clone(),
                known: self.known_ids(),
            })?;
            for module in bundle.modules {
                if composed.contains(&module.id) {
                    continue;
                }
                composed.push(module.id);
                composition = composition.module(module);
            }
        }
        Ok(composition)
    }

    pub async fn list_products(&self, config: &Config) -> Result<Vec<ProductState>, Error> {
        let storage = Storage::connect(config).await?;
        let outcome = self.states(&storage).await;
        storage.close().await?;
        outcome
    }

    pub async fn enable_product(&self, config: &Config, id: &str) -> Result<Change, Error> {
        if self.bundle(id).is_none() {
            return Err(Error::UnknownProduct {
                id: id.to_owned(),
                known: self.known_ids(),
            });
        }
        refuse_while_running(config).await?;
        let storage = Storage::connect(config).await?;
        let outcome = async {
            let mut enabled = self.enabled(&storage).await?.ids;
            if enabled.iter().any(|known| known == id) {
                return Ok(Change::AlreadyEnabled);
            }
            enabled.push(id.to_owned());
            products::set_enabled(storage.db(), &enabled).await?;
            Ok(Change::Enabled)
        }
        .await;
        storage.close().await?;
        outcome
    }

    pub async fn disable_product(&self, config: &Config, id: &str) -> Result<Change, Error> {
        refuse_while_running(config).await?;
        let storage = Storage::connect(config).await?;
        let outcome = async {
            let mut enabled = self.enabled(&storage).await?.ids;
            let before = enabled.len();
            enabled.retain(|known| known != id);
            if enabled.len() == before {
                if self.bundle(id).is_none() {
                    return Err(Error::UnknownProduct {
                        id: id.to_owned(),
                        known: self.known_ids(),
                    });
                }
                return Ok(Change::AlreadyDisabled);
            }
            products::set_enabled(storage.db(), &enabled).await?;
            Ok(Change::Disabled)
        }
        .await;
        storage.close().await?;
        outcome
    }

    pub(crate) async fn enabled(&self, storage: &Storage) -> Result<Enabled, Error> {
        let kernel = storage.migrate().await?;
        let ids = products::enabled(storage.db()).await?;
        Ok(Enabled { kernel, ids })
    }

    async fn states(&self, storage: &Storage) -> Result<Vec<ProductState>, Error> {
        let enabled = self.enabled(storage).await?.ids;
        let mut states: Vec<ProductState> = self
            .products
            .iter()
            .map(|bundle| ProductState {
                id: bundle.id.to_owned(),
                version: Some(bundle.version),
                enabled: enabled.iter().any(|id| id == bundle.id),
            })
            .collect();
        for id in enabled {
            if self.bundle(&id).is_none() {
                states.push(ProductState {
                    id,
                    version: None,
                    enabled: true,
                });
            }
        }
        Ok(states)
    }
}

async fn refuse_while_running(config: &Config) -> Result<(), Error> {
    match UnixStream::connect(&config.socket).await {
        Ok(_) => Err(Error::ServiceRunning {
            socket: config.socket.clone(),
        }),
        Err(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_product_state_prints_its_id_state_and_version() {
        let carried = ProductState {
            id: "demo".into(),
            version: Some("0.1.0"),
            enabled: true,
        };
        assert_eq!(carried.to_string(), "demo\tenabled\t0.1.0");
        let foreign = ProductState {
            id: "dns".into(),
            version: None,
            enabled: true,
        };
        assert_eq!(
            foreign.to_string(),
            "dns\tenabled\tnot carried by this binary"
        );
        assert_eq!(Change::AlreadyEnabled.to_string(), "already enabled");
    }
}
