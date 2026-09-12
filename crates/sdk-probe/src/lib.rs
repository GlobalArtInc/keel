use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ActiveModelTrait, Set};

mod item {
    use keel_sdk::Owned;
    use keel_sdk::schemars::JsonSchema;
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::entity::prelude::*;
    use keel_sdk::serde::Serialize;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, JsonSchema)]
    #[serde(crate = "keel_sdk::serde")]
    #[schemars(crate = "keel_sdk::schemars")]
    #[sea_orm(table_name = "probe_item")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub account_id: String,
        pub title: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    impl Owned for Entity {
        fn account_column() -> Self::Column {
            Column::AccountId
        }
    }
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct ItemEdit {
    pub id: Option<Id>,
    pub title: String,
    #[schemars(extend("x-keel" = json!({ "level": "reseller", "options": "keel.account.options" })))]
    pub account: Option<Id>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Saved {
    pub id: Id,
}

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/migrations").table("keel_migration_probe");

pub static MODULE: ModuleDef = ModuleDef {
    id: "probe",
    version: env!("CARGO_PKG_VERSION"),
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: Some(&MIGRATIONS),
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.function(list())
        .function(edit())
        .menu(
            MenuNode::new("probe/items", "probe.item.list")
                .icon("notebook")
                .order(10),
        )
        .i18n("en", include_str!("../i18n/en.json"))
        .permission("probe.item.read", &[Level::User, Level::Reseller])
        .permission("probe.item.manage", &[Level::User, Level::Reseller]);
    Ok(())
}

fn list() -> FunctionDef {
    function::<ListQuery, Page<item::Model>>("probe.item.list")
        .kind(Kind::List)
        .access(Level::User, "probe.item.read")
        .owned::<item::Entity>()
        .ui(List::new("probe.item.list.title")
            .search(["title"])
            .column(Column::new("title", ColumnType::Text).sortable().primary())
            .empty("probe.item.empty", None))
        .source(entity_source::<item::Entity>())
}

fn edit() -> FunctionDef {
    function::<ItemEdit, Saved>("probe.item.edit")
        .kind(Kind::Form)
        .access(Level::User, "probe.item.manage")
        .owned::<item::Entity>()
        .ui(Form::new("probe.item.edit.title")
            .page("main", ["title", "account"])
            .field("title")
            .readonly_if("$mode == 'edit'"))
        .handler(|call: Call<ItemEdit>| async move {
            let account = call.resolve_account(call.params().account).await?;
            let id = call.params().id.unwrap_or_else(Id::generate);
            let row = item::ActiveModel {
                id: Set(id.to_string()),
                account_id: Set(account.to_string()),
                title: Set(call.params().title.clone()),
            };
            let saved = match call.params().id {
                Some(_) => row.update(call.tx()?).await?,
                None => row.insert(call.tx()?).await?,
            };
            call.emit("probe.item.saved", &saved)?;
            Ok(Saved { id })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_module_registers_one_function_of_each_mode() {
        let mut registrar = Registrar::new(MODULE.id);
        (MODULE.init)(&mut registrar).expect("init registers");
        let registrations = registrar
            .into_registrations()
            .expect("registrations are valid");
        let modes: Vec<(&str, Mode)> = registrations
            .functions
            .iter()
            .map(|def| (def.name(), def.mode()))
            .collect();
        assert_eq!(
            modes,
            [
                ("probe.item.list", Mode::Read),
                ("probe.item.edit", Mode::Write)
            ]
        );
        assert_eq!(MODULE.sdk, keel_sdk::VERSION);
        assert_eq!(
            MODULE.migrations.map(|migrations| migrations.ledger()),
            Some("keel_migration_probe")
        );
        for def in &registrations.functions {
            let descriptor = def.descriptor().expect("descriptor compiles");
            assert_eq!(descriptor.module, "probe");
        }
    }
}
