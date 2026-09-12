use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::descriptor::Kind;
use crate::envelope::MetaActor;
use crate::id::Id;
use crate::level::Level;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Boot {
    pub platform: Platform,
    pub products: Vec<BootProduct>,
    pub brand: BootBrand,
    pub principal: Option<BootPrincipal>,
    pub features: Vec<String>,
    pub license: Option<Value>,
    pub menu: Vec<BootMenuGroup>,
    pub dashboard: Vec<Value>,
    pub views: Vec<String>,
    pub meta: String,
    pub problems: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Platform {
    pub version: String,
    pub api: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BootProduct {
    pub id: String,
    pub name: String,
    pub edition: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BootBrand {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo: Option<String>,
    pub hue: u16,
    pub chroma: f64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BootPrincipal {
    pub id: Id,
    pub login: String,
    pub level: Level,
    pub account: Id,
    pub actor: Option<MetaActor>,
    pub lang: String,
    pub tz: Option<String>,
    pub totp: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BootMenuGroup {
    pub id: String,
    pub product: Option<String>,
    pub label: String,
    pub icon: String,
    pub items: Vec<BootMenuItem>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BootMenuItem {
    pub path: String,
    pub label: String,
    pub func: String,
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<Badge>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Badge {
    pub problem: String,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_boot_payload_serialises_the_platform_brand_without_a_logo_and_the_guest_without_a_principal()
     {
        let boot = Boot {
            platform: Platform {
                version: "0.1.0".into(),
                api: 1,
            },
            products: Vec::new(),
            brand: BootBrand {
                name: "keel.brand".into(),
                logo: None,
                hue: 220,
                chroma: 0.08,
            },
            principal: None,
            features: Vec::new(),
            license: None,
            menu: vec![BootMenuGroup {
                id: "system".into(),
                product: None,
                label: "menu.system".into(),
                icon: "settings".into(),
                items: vec![BootMenuItem {
                    path: "system/accounts".into(),
                    label: "keel.account.list.title".into(),
                    func: "keel.account.list".into(),
                    kind: Kind::List,
                    icon: Some("users".into()),
                    badge: None,
                }],
            }],
            dashboard: Vec::new(),
            views: Vec::new(),
            meta: "m-7f3a2c0000000000".into(),
            problems: None,
        };
        let wire = serde_json::to_value(&boot).unwrap();
        assert_eq!(
            wire,
            json!({
                "platform": { "version": "0.1.0", "api": 1 },
                "products": [],
                "brand": { "name": "keel.brand", "hue": 220, "chroma": 0.08 },
                "principal": null,
                "features": [],
                "license": null,
                "menu": [
                    { "id": "system", "product": null, "label": "menu.system", "icon": "settings",
                      "items": [ { "path": "system/accounts", "label": "keel.account.list.title",
                                   "func": "keel.account.list", "kind": "list", "icon": "users" } ] }
                ],
                "dashboard": [],
                "views": [],
                "meta": "m-7f3a2c0000000000",
                "problems": null
            })
        );
        assert_eq!(serde_json::from_value::<Boot>(wire).unwrap(), boot);
    }

    #[test]
    fn a_boot_principal_serialises_a_missing_time_zone_as_null_and_totp_as_a_flag() {
        let principal = BootPrincipal {
            id: Id::ROOT_ACCOUNT,
            login: "root".into(),
            level: Level::Owner,
            account: Id::ROOT_ACCOUNT,
            actor: None,
            lang: "en".into(),
            tz: None,
            totp: false,
        };
        let wire = serde_json::to_value(&principal).unwrap();
        assert_eq!(wire["tz"], Value::Null);
        assert_eq!(wire["totp"], json!(false));
        assert_eq!(
            serde_json::from_value::<BootPrincipal>(wire).unwrap(),
            principal
        );
    }
}
