use keel_sdk::KeelError;
use keel_sdk::system::Features;
use keel_types::{Id, Level, Principal};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, Value};
use sha2::{Digest, Sha256};

use crate::registry::{ModuleInfo, Registry};

const VERSION_PREFIX: &str = "m-";
const VERSION_HEX_LEN: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MetaKey<'a> {
    pub(crate) build: &'a str,
    pub(crate) modules: &'a [ModuleInfo],
    pub(crate) products: &'a [&'a str],
    pub(crate) licensed: &'a str,
    pub(crate) lang: &'a str,
    pub(crate) principal: Id,
    pub(crate) acl_gen: i32,
}

pub(crate) async fn version(
    registry: &Registry,
    db: &DatabaseConnection,
    principal: &Principal,
) -> Result<String, KeelError> {
    let acl_gen = acl_gen(db, principal).await?;
    Ok(MetaKey {
        build: &registry.platform().version,
        modules: registry.modules(),
        products: &registry.product_ids(),
        licensed: &licensed(registry),
        lang: &principal.lang,
        principal: principal.id,
        acl_gen,
    }
    .version())
}

pub(crate) fn catalogue_version(registry: &Registry, lang: &str) -> String {
    MetaKey {
        build: &registry.platform().version,
        modules: registry.modules(),
        products: &registry.product_ids(),
        licensed: "",
        lang,
        principal: Id::ROOT_ACCOUNT,
        acl_gen: 0,
    }
    .version()
}

fn licensed(registry: &Registry) -> String {
    let Some(features) = registry.services().get::<dyn Features>() else {
        return String::new();
    };
    let Some(license) = features.license() else {
        return String::new();
    };
    let mut enabled = features.enabled();
    enabled.sort_unstable();
    let editions: Vec<String> = registry
        .products()
        .iter()
        .map(|manifest| format!("{}:{:?}", manifest.id, features.edition(manifest)))
        .collect();
    format!("{}|{}|{license:?}", enabled.join(","), editions.join(","))
}

impl MetaKey<'_> {
    pub(crate) fn version(&self) -> String {
        let mut modules: Vec<String> = self
            .modules
            .iter()
            .map(|module| format!("{}@{}", module.id, module.version))
            .collect();
        modules.sort_unstable();
        let mut products: Vec<&str> = self.products.to_vec();
        products.sort_unstable();
        let mut hasher = Sha256::new();
        let licensed = (!self.licensed.is_empty()).then(|| self.licensed.to_owned());
        let lines = [
            Some(self.build.to_owned()),
            Some(modules.join(",")),
            Some(products.join(",")),
            licensed,
            Some(self.lang.to_owned()),
            Some(self.principal.to_string()),
            Some(self.acl_gen.to_string()),
        ];
        for line in lines.into_iter().flatten() {
            hasher.update(line.as_bytes());
            hasher.update(b"\n");
        }
        let digest = hasher.finalize();
        let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        format!("{VERSION_PREFIX}{}", &hex[..VERSION_HEX_LEN])
    }
}

pub(crate) async fn acl_gen(
    db: &DatabaseConnection,
    principal: &Principal,
) -> Result<i32, KeelError> {
    if principal.level == Level::Guest {
        return Ok(0);
    }
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT acl_gen FROM keel_principal WHERE id = $1",
            [Value::from(principal.id.to_string())],
        ))
        .await?;
    Ok(match row {
        Some(row) => row.try_get::<i32>("", "acl_gen")?,
        None => 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modules() -> Vec<ModuleInfo> {
        vec![
            ModuleInfo {
                id: "keel".into(),
                version: "0.1.0".into(),
            },
            ModuleInfo {
                id: "dns".into(),
                version: "1.2.0".into(),
            },
        ]
    }

    fn key<'a>(modules: &'a [ModuleInfo], products: &'a [&'a str]) -> MetaKey<'a> {
        MetaKey {
            build: "0.1.0",
            modules,
            products,
            licensed: "",
            lang: "en",
            principal: Id::ROOT_ACCOUNT,
            acl_gen: 0,
        }
    }

    #[test]
    fn the_version_is_stable_and_independent_of_module_order() {
        let modules = modules();
        let mut reversed = modules.clone();
        reversed.reverse();
        let version = key(&modules, &[]).version();
        assert_eq!(version, key(&reversed, &[]).version());
        assert!(version.starts_with("m-"), "{version}");
        assert_eq!(version.len(), 2 + VERSION_HEX_LEN, "{version}");
    }

    #[test]
    fn every_input_changes_the_version() {
        let modules = modules();
        let base = key(&modules, &[]);
        let baseline = base.version();
        let mut other = modules.clone();
        other[1].version = "1.3.0".into();
        let cases = [
            MetaKey {
                build: "0.2.0",
                ..base.clone()
            },
            MetaKey {
                modules: &other,
                ..base.clone()
            },
            MetaKey {
                products: &["dns"],
                ..base.clone()
            },
            MetaKey {
                licensed: "dns.zones|dns:Some(\"lite\")|valid",
                ..base.clone()
            },
            MetaKey {
                lang: "ru",
                ..base.clone()
            },
            MetaKey {
                principal: Id::generate(),
                ..base.clone()
            },
            MetaKey {
                acl_gen: 1,
                ..base.clone()
            },
        ];
        for case in cases {
            assert_ne!(case.version(), baseline, "{case:?}");
        }
    }
}
