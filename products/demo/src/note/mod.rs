mod export;
mod functions;
mod hooks;
mod import;
mod item;
mod problems;
mod tags;

use keel::prelude::*;
use keel::sdk::chrono::NaiveDate;

pub const MAX_PER_ACCOUNT: u64 = 20;
pub const MAX_LINE: usize = 200;
pub const LOCKED_PREFIX: &str = "locked";
pub const DELETE_BATCH_THRESHOLD: u32 = 10;
pub const FUNCTIONS: [&str; 9] = [
    "note.list",
    "note.get",
    "note.edit",
    "note.delete",
    "note.recompute",
    "note.import",
    "note.import.check",
    "note.tag.list",
    "note.export",
];

pub use export::{
    CLASS as EXPORT_CLASS, CSV_HEADER as EXPORT_CSV_HEADER, LOG_PREFIX as EXPORT_LOG_PREFIX,
};
pub use problems::{OVERDUE as OVERDUE_PROBLEM, overdue};

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/migrations").table("keel_migration_note");

pub static MODULE: ModuleDef = ModuleDef {
    id: "note",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &["task", "notify"],
    migrations: Some(&MIGRATIONS),
    init,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", rename_all = "lowercase")]
#[schemars(crate = "keel::sdk::schemars")]
pub enum NoteKind {
    #[default]
    Memo,
    Todo,
}

impl NoteKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            NoteKind::Memo => "memo",
            NoteKind::Todo => "todo",
        }
    }
}

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
    #[serde(default)]
    pub kind: NoteKind,
    #[serde(default)]
    #[schemars(extend("x-keel" = json!({ "check": "name" }), "x-ui" = json!({ "control": "tags" })))]
    pub tags: Vec<String>,
    #[schemars(extend("x-ui" = json!({ "control": "date" })))]
    pub due: Option<NaiveDate>,
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
        .function(functions::recompute())
        .function(import::import())
        .function(import::check())
        .function(tags::list())
        .function(export::export())
        .permission("note.read", &[Level::User, Level::Reseller])
        .permission("note.manage", &[Level::User, Level::Reseller])
        .menu(
            MenuNode::new("demo/notes", "note.list")
                .icon("book-open")
                .order(10)
                .badge(OVERDUE_PROBLEM),
        )
        .contribute(problems::source())
        .menu(
            MenuNode::new("demo/tags", "note.tag.list")
                .icon("tag")
                .order(20),
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
                ("note.recompute", Kind::Action, Mode::Read),
                ("note.import", Kind::Wizard, Mode::Write),
                ("note.import.check", Kind::Action, Mode::Read),
                ("note.tag.list", Kind::List, Mode::Read),
                ("note.export", Kind::Action, Mode::Read),
            ]
        );
        let names: Vec<&str> = functions.iter().map(|(name, _, _)| *name).collect();
        assert_eq!(names, FUNCTIONS);
        for def in &registrations.functions {
            let owned = matches!(
                def.name(),
                "note.list" | "note.get" | "note.edit" | "note.delete"
            );
            assert_eq!(def.owned().is_some(), owned, "{} ownership", def.name());
            assert_ne!(
                *def.ownership(),
                keel::sdk::function::Ownership::Undeclared,
                "{} declares its ownership",
                def.name()
            );
            let descriptor = def.descriptor().expect("the descriptor compiles");
            assert_eq!(descriptor.module, "note");
        }
        let keys: Vec<&str> = registrations
            .permissions
            .iter()
            .map(|permission| permission.key.as_str())
            .collect();
        assert_eq!(keys, ["note.read", "note.manage"]);
        assert_eq!(registrations.menu.len(), 2);
        assert_eq!(registrations.menu[0].path(), "demo/notes");
        assert_eq!(registrations.menu[0].func(), "note.list");
        assert_eq!(registrations.menu[0].badge_problem(), Some(OVERDUE_PROBLEM));
        let sources: Vec<&str> = registrations
            .contributions
            .of::<keel_system::notify::api::ProblemSource>()
            .map(|source| source.id())
            .collect();
        assert_eq!(sources, [OVERDUE_PROBLEM]);
        assert_eq!(registrations.menu[1].path(), "demo/tags");
        assert_eq!(registrations.menu[1].func(), "note.tag.list");
        let languages: Vec<&str> = registrations
            .i18n
            .iter()
            .map(|bundle| bundle.lang.as_str())
            .collect();
        assert_eq!(languages, ["en", "ru"]);
        assert_eq!(registrations.before[0].function, "note.delete");
        assert_eq!(registrations.within[0].function, "note.edit");
        let delete = registrations
            .functions
            .iter()
            .find(|def| def.name() == "note.delete")
            .expect("note.delete");
        assert_eq!(
            delete.limits().batch_threshold,
            Some(DELETE_BATCH_THRESHOLD)
        );
        assert_eq!(MODULE.requires, ["task", "notify"]);
        assert_eq!(MODULE.sdk, VERSION);
        assert_eq!(
            MODULE.migrations.map(|migrations| migrations.ledger()),
            Some("keel_migration_note")
        );
    }

    #[test]
    fn a_note_kind_names_itself_and_defaults_to_memo() {
        assert_eq!(NoteKind::default(), NoteKind::Memo);
        assert_eq!(NoteKind::Memo.as_str(), "memo");
        assert_eq!(NoteKind::Todo.as_str(), "todo");
        assert_eq!(
            keel::sdk::serde_json::to_value(NoteKind::Todo).expect("serialises"),
            json!("todo")
        );
    }
}
