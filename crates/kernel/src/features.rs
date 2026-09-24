use keel_sdk::ProductManifest;
use keel_sdk::system::Features;

pub fn every(products: &[&ProductManifest]) -> Vec<String> {
    let mut features: Vec<String> = Vec::new();
    let declared = products
        .iter()
        .flat_map(|product| product.editions)
        .flat_map(|edition| edition.features);
    for feature in declared {
        if !features.iter().any(|known| known == feature) {
            features.push((*feature).to_owned());
        }
    }
    features
}

pub(crate) struct ProductFeatures {
    enabled: Vec<String>,
}

impl ProductFeatures {
    pub(crate) fn new(products: &[&ProductManifest]) -> Self {
        Self {
            enabled: every(products),
        }
    }
}

impl Features for ProductFeatures {
    fn enabled(&self) -> Vec<String> {
        self.enabled.clone()
    }

    fn allows(&self, feature: &str) -> bool {
        self.enabled.iter().any(|known| known == feature)
    }
}

#[cfg(test)]
mod tests {
    use keel_sdk::{Brand, Edition, LevelLabels};

    use super::*;

    fn bundle(id: &'static str, editions: &'static [Edition]) -> ProductManifest {
        ProductManifest {
            id,
            name: "product.brand",
            version: "0.1.0",
            api: 1,
            brand: Brand {
                logo: "product.logo",
                hue: 0,
                chroma: 0.0,
            },
            modules: &[],
            menu: &[],
            levels: LevelLabels {
                labels: &[],
                disabled: &[],
            },
            account_kinds: &[],
            editions,
            i18n: &[],
            assets: &[],
        }
    }

    #[test]
    fn every_edition_of_every_product_counts_once_in_declaration_order() {
        let zones = bundle(
            "zones",
            &[
                Edition {
                    id: "lite",
                    features: &["zones.basic"],
                },
                Edition {
                    id: "pro",
                    features: &["zones.basic", "zones.signed", "shared.backup"],
                },
            ],
        );
        let sites = bundle(
            "sites",
            &[Edition {
                id: "lite",
                features: &["sites.web", "shared.backup"],
            }],
        );
        let bare = bundle("bare", &[]);
        assert_eq!(
            every(&[&zones, &bare, &sites]),
            ["zones.basic", "zones.signed", "shared.backup", "sites.web"]
        );
        assert!(every(&[&bare]).is_empty());
        let features = ProductFeatures::new(&[&sites]);
        assert_eq!(features.enabled(), ["sites.web", "shared.backup"]);
        assert!(features.allows("shared.backup"));
        assert!(!features.allows("zones.basic"));
    }
}
