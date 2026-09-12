use std::fmt;

use crate::Result;
use crate::registrar::Registrar;

pub struct ModuleDef {
    pub id: &'static str,
    pub version: &'static str,
    pub sdk: &'static str,
    pub requires: &'static [&'static str],
    pub migrations: Option<&'static migrator::Migrations>,
    pub init: fn(&mut Registrar) -> Result<()>,
}

impl fmt::Debug for ModuleDef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModuleDef")
            .field("id", &self.id)
            .field("version", &self.version)
            .field("sdk", &self.sdk)
            .field("requires", &self.requires)
            .field(
                "migrations",
                &self.migrations.map(|migrations| migrations.ledger()),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init(_: &mut Registrar) -> Result<()> {
        Ok(())
    }

    static MODULE: ModuleDef = ModuleDef {
        id: "probe",
        version: "0.0.1",
        sdk: crate::VERSION,
        requires: &["auth"],
        migrations: None,
        init,
    };

    #[test]
    fn a_module_definition_lives_in_a_static() {
        assert_eq!(MODULE.id, "probe");
        assert_eq!(MODULE.sdk, env!("CARGO_PKG_VERSION"));
        assert_eq!(MODULE.requires, &["auth"]);
        assert!(MODULE.migrations.is_none());
        let mut registrar = Registrar::new(MODULE.id);
        assert!((MODULE.init)(&mut registrar).is_ok());
        assert_eq!(
            format!("{MODULE:?}"),
            format!(
                "ModuleDef {{ id: \"probe\", version: \"0.0.1\", sdk: {:?}, requires: [\"auth\"], migrations: None }}",
                crate::VERSION
            )
        );
    }
}
