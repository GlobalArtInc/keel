use std::fmt;

use keel_types::Level;

use crate::module::ModuleDef;

pub struct ProductBundle {
    pub id: &'static str,
    pub name: &'static str,
    pub version: &'static str,
    pub api: u32,
    pub brand: Brand,
    pub modules: &'static [&'static ModuleDef],
    pub menu: &'static [MenuGroup],
    pub levels: LevelLabels,
    pub account_kinds: &'static [&'static str],
    pub editions: &'static [Edition],
    pub i18n: &'static [(&'static str, &'static str)],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Brand {
    pub logo: &'static str,
    pub hue: u16,
    pub chroma: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuGroup {
    pub id: &'static str,
    pub label: &'static str,
    pub icon: &'static str,
    pub order: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LevelLabels {
    pub labels: &'static [(Level, &'static str)],
    pub disabled: &'static [Level],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edition {
    pub id: &'static str,
    pub features: &'static [&'static str],
}

impl ProductBundle {
    pub fn module_ids(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.modules.iter().map(|module| module.id)
    }

    pub fn default_edition(&self) -> Option<&'static Edition> {
        self.editions.first()
    }
}

impl fmt::Debug for ProductBundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProductBundle")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("version", &self.version)
            .field("api", &self.api)
            .field("brand", &self.brand)
            .field("modules", &self.module_ids().collect::<Vec<_>>())
            .field("menu", &self.menu)
            .field("levels", &self.levels)
            .field("account_kinds", &self.account_kinds)
            .field("editions", &self.editions)
            .field(
                "i18n",
                &self.i18n.iter().map(|(lang, _)| *lang).collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registrar::Registrar;

    fn init(_: &mut Registrar) -> crate::Result<()> {
        Ok(())
    }

    static NOTE: ModuleDef = ModuleDef {
        id: "note",
        version: "0.1.0",
        sdk: crate::VERSION,
        requires: &[],
        migrations: None,
        init,
    };

    static DEMO: ProductBundle = ProductBundle {
        id: "demo",
        name: "product.demo.brand",
        version: "0.1.0",
        api: 1,
        brand: Brand {
            logo: "product.demo.logo",
            hue: 150,
            chroma: 0.1,
        },
        modules: &[&NOTE],
        menu: &[MenuGroup {
            id: "demo",
            label: "menu.demo",
            icon: "note",
            order: 10,
        }],
        levels: LevelLabels {
            labels: &[(Level::User, "product.demo.level.user")],
            disabled: &[Level::Reseller],
        },
        account_kinds: &["user"],
        editions: &[
            Edition {
                id: "lite",
                features: &["note.basic"],
            },
            Edition {
                id: "pro",
                features: &["note.basic", "note.export"],
            },
        ],
        i18n: &[("en", r#"{ "product": { "demo": { "brand": "Demo" } } }"#)],
    };

    #[test]
    fn a_bundle_lives_in_a_static_and_names_its_modules_by_definition() {
        assert_eq!(DEMO.module_ids().collect::<Vec<_>>(), ["note"]);
        assert!(std::ptr::eq(DEMO.modules[0], &NOTE));
        assert_eq!(
            DEMO.default_edition().map(|edition| edition.id),
            Some("lite")
        );
        assert_eq!(
            LevelLabels::default(),
            LevelLabels {
                labels: &[],
                disabled: &[]
            }
        );
        assert_eq!(DEMO.menu[0].label, "menu.demo");
        assert_eq!(DEMO.levels.disabled, [Level::Reseller]);
        let debug = format!("{DEMO:?}");
        assert!(debug.contains("id: \"demo\""), "{debug}");
        assert!(debug.contains("modules: [\"note\"]"), "{debug}");
        assert!(debug.contains("hue: 150"), "{debug}");
        assert!(debug.contains("i18n: [\"en\"]"), "{debug}");
    }
}
