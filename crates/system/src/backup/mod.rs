mod create;
mod list;
mod restore;

use keel_sdk::prelude::*;
use keel_sdk::system::{Backups, TaskRunner};
use keel_sdk::types::Platform;

pub use create::{CLASS, ERROR_INLINE, ERROR_LINK, PROGRESS_ARCHIVE, PROGRESS_LINK};
pub use list::{ARCHIVE_SUFFIX, ERROR_DIRECTORY};
pub use restore::{ERROR_NAME, ERROR_NOT_FOUND, PROGRESS_CHECK, PROGRESS_REQUEST};

pub const LIST: &str = "backup.list";
pub const CREATE: &str = "backup.create";
pub const RESTORE: &str = "backup.restore";
pub const FUNCTIONS: [&str; 3] = [LIST, CREATE, RESTORE];
pub const MENU_PATH: &str = "system/backups";
pub const MENU_ICON: &str = "archive";

pub static MODULE: ModuleDef = ModuleDef {
    id: "backup",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &["task"],
    migrations: None,
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.require::<dyn Backups>()
        .require::<dyn TaskRunner>()
        .require::<Platform>()
        .function(list::list())
        .function(create::create())
        .function(restore::restore())
        .menu(MenuNode::new(MENU_PATH, LIST).icon(MENU_ICON).order(49))
        .i18n("en", include_str!("../../i18n/backup/en.json"))
        .i18n("ru", include_str!("../../i18n/backup/ru.json"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_module_registers_the_three_functions_behind_the_kernel_seams() {
        let mut registrar = Registrar::new(MODULE.id);
        (MODULE.init)(&mut registrar).expect("init registers");
        assert_eq!(
            registrar.function_names().into_iter().collect::<Vec<_>>(),
            [CREATE, LIST, RESTORE]
        );
        let registrations = registrar
            .into_registrations()
            .expect("the registrations are valid");
        assert_eq!(registrations.requires.len(), 3);
        assert_eq!(registrations.menu.len(), 1);
        assert_eq!(registrations.menu[0].path(), MENU_PATH);
        assert_eq!(registrations.menu[0].func(), LIST);
        assert_eq!(registrations.i18n.len(), 2);
        assert!(MODULE.migrations.is_none());
        assert_eq!(MODULE.requires, ["task"]);
    }
}
