use keel::prelude::*;

const LOGO: &str = "product.panel.logo";

pub static MANIFEST: ProductManifest = ProductManifest {
    id: "panel",
    name: "product.panel.brand",
    version: env!("CARGO_PKG_VERSION"),
    api: keel::types::DESCRIPTOR_VERSION,
    brand: Brand {
        logo: LOGO,
        hue: 264,
        chroma: 0.18,
    },
    modules: &[
        &keel_host::MODULE,
        &keel_hosting_users::MODULE,
        &keel_hosting_web::MODULE,
        &keel_hosting_ssl::MODULE,
        &keel_hosting_db::MODULE,
        &keel_hosting_mail::MODULE,
        &keel_hosting_ftp::MODULE,
    ],
    bridges: &[Bridge {
        to: "dns",
        local: &keel_dns_local::MODULE,
        remote: &keel_dns_peer::MODULE,
    }],
    menu: &[MenuGroup {
        id: "panel",
        label: "menu.panel",
        icon: "server",
        order: 20,
    }],
    levels: LevelLabels {
        labels: &[],
        disabled: &[],
    },
    account_kinds: &["user"],
    editions: &[
        Edition {
            id: "lite",
            features: &[
                "panel.users",
                "panel.web",
                "panel.databases",
                "panel.mail",
                "panel.ftp",
            ],
        },
        Edition {
            id: "pro",
            features: &[
                "panel.users",
                "panel.web",
                "panel.databases",
                "panel.mail",
                "panel.ftp",
            ],
        },
    ],
    i18n: &[
        ("en", include_str!("../i18n/en.json")),
        ("ru", include_str!("../i18n/ru.json")),
    ],
    assets: &[Asset {
        id: LOGO,
        content_type: "image/svg+xml",
        bytes: include_bytes!("../brand/logo.svg"),
    }],
};

#[cfg(test)]
mod tests {
    use keel::sdk::serde_json;

    use super::*;

    const PLATFORM_FILL: &str = "#315fcf";

    #[test]
    fn the_bundle_names_its_modules_the_dns_bridge_one_group_and_the_lite_and_pro_editions() {
        assert_eq!(MANIFEST.id, "panel");
        assert_eq!(
            MANIFEST.module_ids().collect::<Vec<_>>(),
            [
                "host",
                "hosting",
                "hosting_site",
                "hosting_certificate",
                "hosting_database",
                "hosting_mail",
                "hosting_ftp"
            ]
        );
        assert_eq!(MANIFEST.bridges.len(), 1);
        assert_eq!(MANIFEST.bridges[0].to, "dns");
        assert_eq!(MANIFEST.bridges[0].side(true).id, "dns_local");
        assert_eq!(MANIFEST.bridges[0].side(false).id, "dns_peer");
        assert_eq!(MANIFEST.menu.len(), 1);
        assert_eq!(MANIFEST.menu[0].id, "panel");
        assert_eq!(MANIFEST.menu[0].label, "menu.panel");
        assert_eq!(MANIFEST.menu[0].icon, "server");
        let editions: Vec<&str> = MANIFEST.editions.iter().map(|edition| edition.id).collect();
        assert_eq!(editions, ["lite", "pro"]);
        let edition = MANIFEST.default_edition().expect("the lite edition");
        assert_eq!(edition.id, "lite");
        assert_eq!(
            edition.features,
            [
                "panel.users",
                "panel.web",
                "panel.databases",
                "panel.mail",
                "panel.ftp"
            ]
        );
        let logo = MANIFEST.logo().expect("the logo asset");
        assert_eq!(logo.id, LOGO);
        assert_eq!(logo.content_type, "image/svg+xml");
        for (_, json) in MANIFEST.i18n {
            let messages: Value = serde_json::from_str(json).expect("a JSON dictionary");
            assert_eq!(messages["product"]["panel"]["brand"], json!("KeelPanel"));
            assert_eq!(
                messages["menu"]["panel"].as_str().map(str::is_empty),
                Some(false)
            );
        }
    }

    #[test]
    fn the_logo_is_drawn_in_the_platform_palette() {
        let svg = std::str::from_utf8(MANIFEST.logo().expect("the logo asset").bytes)
            .expect("an SVG document");
        assert!(svg.starts_with("<svg"));
        let colours: Vec<&str> = ["fill=\"", "stroke=\""]
            .iter()
            .flat_map(|attribute| svg.split(attribute).skip(1))
            .filter_map(|rest| rest.split('"').next())
            .filter(|colour| !colour.eq_ignore_ascii_case("#ffffff") && *colour != "none")
            .collect();
        assert!(!colours.is_empty());
        assert!(
            colours.iter().all(|colour| *colour == PLATFORM_FILL),
            "{colours:?}"
        );
    }
}
