use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::descriptor::Kind;
use crate::envelope::MetaActor;
use crate::id::Id;
use crate::is_false;
use crate::level::Level;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Boot {
    pub keel: Platform,
    pub products: Vec<BootProduct>,
    pub brand: BootBrand,
    pub principal: Option<BootPrincipal>,
    pub features: Vec<String>,
    pub license: Option<BootLicense>,
    pub menu: Vec<BootMenuGroup>,
    pub dashboard: Vec<BootWidget>,
    pub views: Vec<String>,
    pub meta: String,
    pub problems: Option<BootProblems>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BootWidget {
    pub kind: WidgetKind,
    pub func: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WidgetKind {
    Stat,
    List,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DashboardLayout {
    #[serde(default)]
    pub order: Vec<String>,
    #[serde(default)]
    pub hidden: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StatValue {
    pub value: f64,
    pub unit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<Id>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BootProblems {
    pub open: u64,
    pub critical: u64,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BootLicense {
    pub state: LicenseState,
    pub expires_at: Option<DateTime<Utc>>,
    pub grace_ends_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LicenseState {
    None,
    Valid,
    Grace,
    ReadOnly,
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

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
                hue: 264,
                chroma: 0.18,
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
                "brand": { "name": "keel.brand", "hue": 264, "chroma": 0.18 },
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
    fn a_widget_omits_a_missing_limit_and_the_problem_counts_start_at_zero() {
        let widget = BootWidget {
            kind: WidgetKind::Stat,
            func: "dns.widget.zones".into(),
            limit: None,
            hidden: false,
        };
        let wire = serde_json::to_value(&widget).unwrap();
        assert_eq!(wire, json!({ "kind": "stat", "func": "dns.widget.zones" }));
        assert_eq!(serde_json::from_value::<BootWidget>(wire).unwrap(), widget);
        let hidden = BootWidget {
            hidden: true,
            ..widget
        };
        assert_eq!(
            serde_json::to_value(&hidden).unwrap(),
            json!({ "kind": "stat", "func": "dns.widget.zones", "hidden": true })
        );
        let listed: BootWidget = serde_json::from_value(json!({
            "kind": "list", "func": "notify.problem.list", "limit": 5
        }))
        .unwrap();
        assert_eq!(listed.kind, WidgetKind::List);
        assert_eq!(listed.limit, Some(5));
        assert!(serde_json::from_value::<WidgetKind>(json!("chart")).is_err());
        assert_eq!(
            serde_json::to_value(BootProblems::default()).unwrap(),
            json!({ "open": 0, "critical": 0 })
        );
        let counted: BootProblems =
            serde_json::from_value(json!({ "open": 1, "critical": 0 })).unwrap();
        assert_eq!(counted.open, 1);
        assert_eq!(counted.critical, 0);
    }

    #[test]
    fn a_layout_names_its_order_and_hidden_widgets_and_refuses_anything_else() {
        let layout: DashboardLayout = serde_json::from_value(json!({
            "order": ["dns.widget.zones", "notify.problem.list"],
            "hidden": ["keel.widget.accounts"]
        }))
        .unwrap();
        assert_eq!(layout.order, ["dns.widget.zones", "notify.problem.list"]);
        assert_eq!(layout.hidden, ["keel.widget.accounts"]);
        assert_eq!(
            serde_json::from_value::<DashboardLayout>(json!({})).unwrap(),
            DashboardLayout::default()
        );
        assert!(serde_json::from_value::<DashboardLayout>(json!({ "widths": [] })).is_err());
        assert!(serde_json::from_value::<DashboardLayout>(json!({ "order": [1] })).is_err());
    }

    #[test]
    fn a_stat_value_carries_its_unit_and_names_its_series_only_when_it_has_one() {
        let bare = StatValue {
            value: 12.0,
            unit: "count".into(),
            series: None,
            subject: None,
        };
        assert_eq!(
            serde_json::to_value(&bare).unwrap(),
            json!({ "value": 12.0, "unit": "count" })
        );
        let subject: Id = "01J8A0000000000000000000A2".parse().unwrap();
        let charted = StatValue {
            series: Some("dns.zones".into()),
            subject: Some(subject),
            ..bare
        };
        let wire = serde_json::to_value(&charted).unwrap();
        assert_eq!(
            wire,
            json!({
                "value": 12.0,
                "unit": "count",
                "series": "dns.zones",
                "subject": "01J8A0000000000000000000A2"
            })
        );
        assert_eq!(serde_json::from_value::<StatValue>(wire).unwrap(), charted);
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
    fn a_boot_license_serialises_its_state_in_snake_case_and_its_dates_in_camel_case() {
        let license = BootLicense {
            state: LicenseState::ReadOnly,
            expires_at: None,
            grace_ends_at: None,
        };
        let wire = serde_json::to_value(&license).unwrap();
        assert_eq!(
            wire,
            json!({ "state": "read_only", "expiresAt": null, "graceEndsAt": null })
        );
        assert_eq!(
            serde_json::from_value::<BootLicense>(wire).unwrap(),
            license
        );
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
