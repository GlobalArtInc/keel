use keel::prelude::*;

pub const LOGO: &str = "product.dns.logo";

pub static MANIFEST: ProductManifest = ProductManifest {
    id: "dns",
    name: "product.dns.brand",
    version: env!("CARGO_PKG_VERSION"),
    api: keel::types::DESCRIPTOR_VERSION,
    brand: Brand {
        logo: LOGO,
        hue: 264,
        chroma: 0.18,
    },
    modules: &[
        &keel_host::MODULE,
        &keel_dns_zones::MODULE,
        &keel_dns_records::MODULE,
    ],
    menu: &[MenuGroup {
        id: "dns",
        label: "menu.dns",
        icon: "globe",
        order: 10,
    }],
    levels: LevelLabels {
        labels: &[],
        disabled: &[],
    },
    account_kinds: &["user"],
    editions: &[Edition {
        id: "lite",
        features: &["dns.zones"],
    }],
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
    fn the_bundle_names_host_the_zones_and_records_modules_one_group_and_the_lite_edition() {
        assert_eq!(MANIFEST.id, "dns");
        assert_eq!(
            MANIFEST.module_ids().collect::<Vec<_>>(),
            ["host", "dns", "dns_record"]
        );
        assert_eq!(MANIFEST.menu.len(), 1);
        assert_eq!(MANIFEST.menu[0].id, "dns");
        assert_eq!(MANIFEST.menu[0].label, "menu.dns");
        assert_eq!(MANIFEST.menu[0].icon, "globe");
        let edition = MANIFEST.default_edition().expect("the lite edition");
        assert_eq!(edition.id, "lite");
        assert_eq!(edition.features, ["dns.zones"]);
        let logo = MANIFEST.logo().expect("the logo asset");
        assert_eq!(logo.id, LOGO);
        assert_eq!(logo.content_type, "image/svg+xml");
        for (_, json) in MANIFEST.i18n {
            let messages: Value = serde_json::from_str(json).expect("a JSON dictionary");
            assert_eq!(messages["product"]["dns"]["brand"], json!("KeelDNS"));
            assert_eq!(
                messages["menu"]["dns"].as_str().map(str::is_empty),
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
