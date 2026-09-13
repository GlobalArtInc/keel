use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::descriptor::Kind;
use crate::envelope::MetaActor;
use crate::id::Id;
use crate::level::Level;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Boot {
    pub keel: Platform,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BootProduct {
    pub id: String,
    pub name: String,
    pub edition: Option<String>,
    pub brand: BootBrand,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<Kind>,
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
    fn a_boot_payload_names_keel_the_products_with_their_brands_and_the_guest_without_a_principal()
    {
        let boot = Boot {
            keel: Platform {
                version: "0.1.0".into(),
                api: 1,
            },
            products: vec![BootProduct {
                id: "dns".into(),
                name: "product.dns.brand".into(),
                edition: None,
                brand: BootBrand {
                    name: "product.dns.brand".into(),
                    logo: Some("/brand/dns/logo.svg".into()),
                    hue: 264,
                    chroma: 0.12,
                },
            }],
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
                    kind: Some(Kind::List),
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
                "keel": { "version": "0.1.0", "api": 1 },
                "products": [
                    { "id": "dns", "name": "product.dns.brand", "edition": null,
                      "brand": { "name": "product.dns.brand", "logo": "/brand/dns/logo.svg", "hue": 264, "chroma": 0.12 } }
                ],
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
    fn a_menu_item_of_a_view_carries_no_kind_member_and_reads_back_without_one() {
        let item = BootMenuItem {
            path: "hosting/files".into(),
            label: "hosting.file_manager.title".into(),
            func: "hosting.file-manager".into(),
            kind: None,
            icon: Some("folder".into()),
            badge: None,
        };
        let wire = serde_json::to_value(&item).unwrap();
        assert_eq!(
            wire,
            json!({ "path": "hosting/files", "label": "hosting.file_manager.title",
                    "func": "hosting.file-manager", "icon": "folder" })
        );
        assert_eq!(serde_json::from_value::<BootMenuItem>(wire).unwrap(), item);
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
