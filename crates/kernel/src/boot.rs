use keel_sdk::KeelError;
use keel_types::{Boot, BootBrand, BootPrincipal, BootProduct, Principal};
use sea_orm::DatabaseConnection;

use crate::dispatcher;
use crate::menu;
use crate::meta;
use crate::registry::Registry;

pub const PLATFORM_BRAND: &str = "keel.brand";
pub const PLATFORM_HUE: u16 = 220;
pub const PLATFORM_CHROMA: f64 = 0.08;

pub fn platform_brand() -> BootBrand {
    BootBrand {
        name: PLATFORM_BRAND.to_owned(),
        logo: None,
        hue: PLATFORM_HUE,
        chroma: PLATFORM_CHROMA,
    }
}

pub fn brand(registry: &Registry) -> BootBrand {
    match registry.products() {
        [single] => BootBrand {
            name: single.name.to_owned(),
            logo: Some("/brand/logo.svg".to_owned()),
            hue: single.brand.hue,
            chroma: single.brand.chroma,
        },
        _ => platform_brand(),
    }
}

pub fn products(registry: &Registry) -> Vec<BootProduct> {
    registry
        .products()
        .iter()
        .map(|bundle| BootProduct {
            id: bundle.id.to_owned(),
            name: bundle.name.to_owned(),
            edition: None,
        })
        .collect()
}

pub fn principal(principal: &Principal) -> Result<Option<BootPrincipal>, KeelError> {
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
        tz: None,
        totp: false,
    }))
}

pub async fn payload(
    registry: &Registry,
    db: &DatabaseConnection,
    principal: &Principal,
) -> Result<Boot, KeelError> {
    let meta = meta::version(registry, db, principal).await?;
    Ok(Boot {
        platform: registry.platform().clone(),
        products: products(registry),
        brand: brand(registry),
        principal: self::principal(principal)?,
        features: Vec::new(),
        license: None,
        menu: menu::project(registry, principal),
        dashboard: Vec::new(),
        views: Vec::new(),
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
        assert!(principal(&someone(Level::Guest)).unwrap().is_none());
        let user = principal(&someone(Level::User)).unwrap().unwrap();
        assert_eq!(user.login, "alice");
        assert_eq!(user.account, Id::ROOT_ACCOUNT);
        assert_eq!(user.lang, "ru");
        assert!(user.actor.is_none());
        assert!(user.tz.is_none());
        assert!(!user.totp);
        let brand = platform_brand();
        assert_eq!(brand.name, PLATFORM_BRAND);
        assert!(brand.logo.is_none());
    }

    #[test]
    fn an_actor_that_arrived_without_delegation_is_an_internal_error_not_a_guest() {
        let mut user = someone(Level::User);
        user.actor = Some(Box::new(someone(Level::Admin)));
        let error = principal(&user).unwrap_err();
        assert_eq!(error.code(), ErrorCode::Internal);
        assert_eq!(error.key(), "keel.error.actor_not_delegated");
        assert_eq!(error.params()["via"], json!("http"));
    }
}
