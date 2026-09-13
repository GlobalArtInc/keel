use chrono_tz::Tz;
use keel_sdk::system::SecondFactorSource;
use keel_sdk::{Db, KeelError, ProductManifest};
use keel_types::{Boot, BootBrand, BootPrincipal, BootProduct, Level, Principal};
use sea_orm::DatabaseConnection;

use crate::dispatcher;
use crate::http::Projection;
use crate::menu;
use crate::meta;
use crate::registry::Registry;

pub(crate) const PLATFORM_BRAND: &str = "keel.brand";
pub(crate) const PLATFORM_HUE: u16 = 220;
pub(crate) const PLATFORM_CHROMA: f64 = 0.08;
pub(crate) const EFFECTIVE_LOGO: &str = "/brand/logo.svg";

pub(crate) fn platform_brand() -> BootBrand {
    BootBrand {
        name: PLATFORM_BRAND.to_owned(),
        logo: None,
        hue: PLATFORM_HUE,
        chroma: PLATFORM_CHROMA,
    }
}

pub(crate) fn brand(registry: &Registry) -> BootBrand {
    match registry.products() {
        [single] => BootBrand {
            name: single.name.to_owned(),
            logo: Some(EFFECTIVE_LOGO.to_owned()),
            hue: single.brand.hue,
            chroma: single.brand.chroma,
        },
        _ => platform_brand(),
    }
}

pub(crate) fn product_brand(manifest: &ProductManifest) -> BootBrand {
    BootBrand {
        name: manifest.name.to_owned(),
        logo: Some(format!("/brand/{}/logo.svg", manifest.id)),
        hue: manifest.brand.hue,
        chroma: manifest.brand.chroma,
    }
}

pub(crate) fn products(registry: &Registry) -> Vec<BootProduct> {
    registry
        .products()
        .iter()
        .map(|manifest| BootProduct {
            id: manifest.id.to_owned(),
            name: manifest.name.to_owned(),
            edition: manifest
                .default_edition()
                .map(|edition| edition.id.to_owned()),
            brand: product_brand(manifest),
        })
        .collect()
}

pub(crate) fn features(registry: &Registry) -> Vec<String> {
    let mut features: Vec<String> = Vec::new();
    for manifest in registry.products() {
        for feature in manifest
            .default_edition()
            .map(|edition| edition.features)
            .unwrap_or_default()
        {
            if !features.iter().any(|known| known == feature) {
                features.push((*feature).to_owned());
            }
        }
    }
    features
}

pub(crate) fn views(registry: &Registry, principal: &Principal) -> Vec<String> {
    let projection = Projection::new(registry, principal);
    registry
        .views()
        .filter(|view| projection.admits(view.access()).is_ok())
        .map(|view| view.name().to_owned())
        .collect()
}

pub(crate) fn principal(
    principal: &Principal,
    totp: bool,
    tz: Option<Tz>,
) -> Result<Option<BootPrincipal>, KeelError> {
    let Some(meta) = dispatcher::meta_principal(principal)? else {
        return Ok(None);
    };
    Ok(Some(BootPrincipal {
        id: meta.id,
        login: meta.login,
        level: meta.level,
        account: meta.account,
        actor: meta.actor,
        lang: principal.lang.clone(),
        tz: tz.map(|tz| tz.name().to_owned()),
        totp,
    }))
}

pub(crate) async fn second_factor(
    registry: &Registry,
    db: &DatabaseConnection,
    principal: &Principal,
) -> Result<bool, KeelError> {
    if principal.level == Level::Guest || principal.level >= Level::Owner {
        return Ok(false);
    }
    match registry.services().get::<dyn SecondFactorSource>() {
        Some(source) => {
            source
                .enrolled(principal.id, &Db::from_connection(db.clone()))
                .await
        }
        None => Ok(false),
    }
}

pub(crate) async fn payload(
    registry: &Registry,
    db: &DatabaseConnection,
    principal: &Principal,
    tz: Option<Tz>,
) -> Result<Boot, KeelError> {
    let meta = meta::version(registry, db, principal).await?;
    let totp = second_factor(registry, db, principal).await?;
    Ok(Boot {
        keel: registry.platform().clone(),
        products: products(registry),
        brand: brand(registry),
        principal: self::principal(principal, totp, tz)?,
        features: features(registry),
        license: None,
        menu: menu::project(registry, principal),
        dashboard: Vec::new(),
        views: views(registry, principal),
        meta,
        problems: None,
    })
}

#[cfg(test)]
mod tests {
    use keel_types::{ErrorCode, Id, Level, Scope, Via};
    use serde_json::json;

    use super::*;

    fn someone(level: Level) -> Principal {
        Principal {
            id: "01J9Q3W6B1R8N5X2K4T7M0V9A1".parse().unwrap(),
            login: "alice".into(),
            level,
            account_id: Id::ROOT_ACCOUNT,
            role: "owner".into(),
            actor: None,
            scope: Scope::Full,
            via: Via::Http,
            external_id: None,
            origin: None,
            lang: "ru".into(),
        }
    }

    #[test]
    fn a_guest_has_no_boot_principal_and_a_user_carries_the_account_and_language() {
        assert!(
            principal(&someone(Level::Guest), false, None)
                .unwrap()
                .is_none()
        );
        let user = principal(&someone(Level::User), false, None)
            .unwrap()
            .unwrap();
        assert_eq!(user.login, "alice");
        assert_eq!(user.account, Id::ROOT_ACCOUNT);
        assert_eq!(user.lang, "ru");
        assert!(user.actor.is_none());
        assert!(user.tz.is_none());
        assert!(!user.totp);
        let zoned = principal(&someone(Level::User), true, Some(chrono_tz::Asia::Tokyo))
            .unwrap()
            .unwrap();
        assert!(zoned.totp);
        assert_eq!(zoned.tz.as_deref(), Some("Asia/Tokyo"));
        let brand = platform_brand();
        assert_eq!(brand.name, PLATFORM_BRAND);
        assert!(brand.logo.is_none());
    }

    #[test]
    fn an_actor_that_arrived_without_delegation_is_an_internal_error_not_a_guest() {
        let mut user = someone(Level::User);
        user.actor = Some(Box::new(someone(Level::Admin)));
        let error = principal(&user, false, None).unwrap_err();
        assert_eq!(error.code(), ErrorCode::Internal);
        assert_eq!(error.key(), "keel.error.actor_not_delegated");
        assert_eq!(error.params()["via"], json!("http"));
    }
}
