mod functions;
mod hooks;
mod item;

use keel::prelude::*;

pub const MAX_PER_ACCOUNT: u64 = 20;
pub const LOCKED_PREFIX: &str = "locked";
pub const FUNCTIONS: [&str; 4] = ["note.list", "note.get", "note.edit", "note.delete"];

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/migrations").table("keel_migration_note");

pub static MODULE: ModuleDef = ModuleDef {
    id: "note",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &[],
    migrations: Some(&MIGRATIONS),
    init,
};

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Pick {
    pub id: Id,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct NoteEdit {
    pub id: Option<Id>,
    #[schemars(length(min = 1, max = 200))]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[schemars(extend("x-keel" = json!({ "level": "reseller", "options": "keel.account.options" })))]
    pub account: Option<Id>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Batch {
    #[schemars(length(min = 1))]
    pub ids: Vec<Id>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Deleted {
    pub deleted: u64,
}

fn init(r: &mut Registrar) -> Result<()> {
    r.function(functions::list())
        .function(functions::get())
        .function(functions::edit())
        .function(functions::delete())
        .permission("note.read", &[Level::User, Level::Reseller])
        .permission("note.manage", &[Level::User, Level::Reseller])
        .menu(
            MenuNode::new("demo/notes", "note.list")
                .icon("notebook")
                .order(10),
        )
        .i18n("en", include_str!("../../i18n/note/en.json"))
        .i18n("ru", include_str!("../../i18n/note/ru.json"))
        .before("note.delete", hooks::refuse_locked)
        .within("note.edit", hooks::enforce_limit);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_module_registers_its_functions_permissions_menu_dictionaries_and_hooks() {
        let mut registrar = Registrar::new(MODULE.id);
        (MODULE.init)(&mut registrar).expect("init registers");
        let registrations = registrar
            .into_registrations()
            .expect("the registrations are valid");
        let functions: Vec<(&str, Kind, Mode)> = registrations
            .functions
            .iter()
            .map(|def| (def.name(), def.kind(), def.mode()))
            .collect();
        assert_eq!(
            functions,
            [
                ("note.list", Kind::List, Mode::Read),
                ("note.get", Kind::Get, Mode::Read),
                ("note.edit", Kind::Form, Mode::Write),
                ("note.delete", Kind::Action, Mode::Write),
            ]
        );
        let names: Vec<&str> = functions.iter().map(|(name, _, _)| *name).collect();
        assert_eq!(names, FUNCTIONS);
        for def in &registrations.functions {
            assert!(def.owned().is_some(), "{} is owned", def.name());
            let descriptor = def.descriptor().expect("the descriptor compiles");
            assert_eq!(descriptor.module, "note");
        }
        let keys: Vec<&str> = registrations
            .permissions
            .iter()
            .map(|permission| permission.key.as_str())
            .collect();
        assert_eq!(keys, ["note.read", "note.manage"]);
        assert_eq!(registrations.menu.len(), 1);
        assert_eq!(registrations.menu[0].path(), "demo/notes");
        assert_eq!(registrations.menu[0].func(), "note.list");
        let languages: Vec<&str> = registrations
            .i18n
            .iter()
            .map(|bundle| bundle.lang.as_str())
            .collect();
        assert_eq!(languages, ["en", "ru"]);
        assert_eq!(registrations.before[0].function, "note.delete");
        assert_eq!(registrations.within[0].function, "note.edit");
        assert_eq!(MODULE.sdk, VERSION);
        assert_eq!(
            MODULE.migrations.map(|migrations| migrations.ledger()),
            Some("keel_migration_note")
        );
    }
}
