use keel::prelude::*;

pub mod note;

pub static MANIFEST: ProductManifest = ProductManifest {
    id: "demo",
    name: "product.demo.brand",
    version: env!("CARGO_PKG_VERSION"),
    api: keel::types::DESCRIPTOR_VERSION,
    brand: Brand {
        logo: "product.demo.logo",
        hue: 150,
        chroma: 0.1,
    },
    modules: &[&note::MODULE],
    menu: &[MenuGroup {
        id: "demo",
        label: "menu.demo",
        icon: "notebook",
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
}
