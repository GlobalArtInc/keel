use keel::prelude::*;

pub mod note;

pub const LOGO: &str = "product.demo.logo";

pub static MANIFEST: ProductManifest = ProductManifest {
    id: "demo",
    name: "product.demo.brand",
    version: env!("CARGO_PKG_VERSION"),
    api: keel::types::DESCRIPTOR_VERSION,
    brand: Brand {
        logo: LOGO,
        hue: 264,
        chroma: 0.18,
    },
    modules: &[&note::MODULE],
    menu: &[MenuGroup {
        id: "demo",
        label: "menu.demo",
        icon: "book-open",
        order: 10,
    }],
    levels: LevelLabels {
        labels: &[],
        disabled: &[],
    },
    account_kinds: &["user"],
    editions: &[],
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

    #[test]
    fn the_bundle_names_the_note_module_one_group_and_no_edition() {
        assert_eq!(MANIFEST.id, "demo");
        assert_eq!(MANIFEST.module_ids().collect::<Vec<_>>(), ["note"]);
        assert!(std::ptr::eq(MANIFEST.modules[0], &note::MODULE));
        assert_eq!(MANIFEST.menu.len(), 1);
        assert_eq!(MANIFEST.menu[0].id, "demo");
        assert_eq!(MANIFEST.menu[0].label, "menu.demo");
        assert_eq!(MANIFEST.account_kinds, ["user"]);
        assert!(MANIFEST.default_edition().is_none());
        assert_eq!(MANIFEST.levels, LevelLabels::default());
        let languages: Vec<&str> = MANIFEST.i18n.iter().map(|(lang, _)| *lang).collect();
        assert_eq!(languages, ["en", "ru"]);
        let logo = MANIFEST.logo().expect("the logo asset");
        assert_eq!(logo.id, LOGO);
        assert_eq!(logo.content_type, "image/svg+xml");
        assert_eq!(logo.bytes, include_bytes!("../brand/logo.svg"));
        assert!(std::str::from_utf8(logo.bytes).is_ok_and(|svg| svg.starts_with("<svg")));
        for (_, json) in MANIFEST.i18n {
            let messages: Value = serde_json::from_str(json).expect("a JSON dictionary");
            assert_eq!(
                messages["product"]["demo"]["brand"]
                    .as_str()
                    .map(str::is_empty),
                Some(false)
            );
            assert_eq!(
                messages["menu"]["demo"].as_str().map(str::is_empty),
                Some(false)
            );
        }
    }

    #[test]
    fn the_demo_logo_is_filled_with_the_platform_palette() {
        let svg = std::str::from_utf8(MANIFEST.logo().expect("the logo asset").bytes)
            .expect("an SVG document");
        let fills: Vec<&str> = svg
            .split("fill=\"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .filter(|fill| !fill.eq_ignore_ascii_case("#ffffff"))
            .collect();
        assert!(!fills.is_empty());
        assert!(fills.iter().all(|fill| *fill == PLATFORM_FILL), "{fills:?}");
    }

    const PLATFORM_FILL: &str = "#315fcf";
}
