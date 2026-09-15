use std::fs;
use std::path::{Path, PathBuf};

use keel_sdk::prelude::*;
use keel_sdk::runtime::{ConditionSource, ConditionTarget, FieldCondition, OwnedDef, Ownership};
use keel_sdk::types::{Descriptor, Expr};

mod zone {
    use keel_sdk::Owned;
    use keel_sdk::schemars::JsonSchema;
    use keel_sdk::sea_orm::Set;
    use keel_sdk::sea_orm::entity::prelude::*;
    use keel_sdk::serde::Serialize;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, JsonSchema)]
    #[sea_orm(table_name = "dns_zone")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub account_id: String,
        pub name: String,
        pub kind: String,
        pub ttl: i64,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    impl Owned for Entity {
        fn account_column() -> Self::Column {
            Column::AccountId
        }
    }

    pub async fn upsert(
        db: &impl ConnectionTrait,
        account: keel_sdk::Id,
        params: &super::ZoneEdit,
    ) -> Result<Model, DbErr> {
        let row = ActiveModel {
            id: Set(params.id.unwrap_or_else(keel_sdk::Id::generate).to_string()),
            account_id: Set(account.to_string()),
            name: Set(params.name.clone()),
            kind: Set(format!("{:?}", params.kind).to_lowercase()),
            ttl: Set(i64::from(params.ttl)),
        };
        match params.id {
            Some(_) => row.update(db).await,
            None => row.insert(db).await,
        }
    }
}

#[derive(Deserialize, Serialize, JsonSchema, Debug)]
#[serde(rename_all = "lowercase")]
pub enum ZoneKind {
    Master,
    Slave,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ZoneEdit {
    pub id: Option<Id>,
    #[schemars(extend("x-keel" = json!({ "check": "domain" })))]
    pub name: String,
    pub kind: ZoneKind,
    #[schemars(extend("x-keel" = json!({ "check": "ip" }), "x-ui" = json!({ "control": "tags" })))]
    pub master_ips: Vec<String>,
    pub ttl: u32,
    #[schemars(extend("x-keel" = json!({ "secret": true }), "x-ui" = json!({ "control": "password", "generate": true })))]
    pub tsig_key: Option<Secret>,
    #[schemars(extend("x-keel" = json!({ "level": "reseller", "options": "keel.account.options" })))]
    pub account: Option<Id>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct Saved {
    pub id: Id,
}

#[async_trait]
pub trait DnsProvider: Send + Sync {
    async fn apply(&self, ctx: &AfterContext, zone: &zone::Model) -> Result<()>;
}

pub fn edit() -> FunctionDef {
    function::<ZoneEdit, Saved>("dns.zone.edit")
        .kind(Kind::Form)
        .mode(Mode::Write)
        .access(Level::User, "dns.zone.manage")
        .feature("dns.zones")
        .owned::<zone::Entity>()
        .ui(Form::new("dns.zone.edit.title")
            .read("dns.zone.get")
            .page("main", ["name", "kind", "ttl", "account"])
            .page("transfer", ["master_ips", "tsig_key"])
            .show_if("kind == 'slave'")
            .field("name")
            .readonly_if("$mode == 'edit'")
            .field("master_ips")
            .required_if("kind == 'slave'")
            .field("ttl")
            .recompute("dns.zone.recompute")
            .multi(["ttl"]))
        .handler(|call: Call<ZoneEdit>| async move {
            let account = call.resolve_account(call.params().account).await?;
            let row = zone::upsert(call.tx()?, account, call.params())
                .await
                .map_err(|e| {
                    KeelError::conflict("dns.zone.error.exists")
                        .field("name")
                        .source(e)
                })?;
            call.emit("dns.zone.saved", &row)?;
            let id = row
                .id
                .parse()
                .map_err(|e| KeelError::internal("dns.zone.error.id").source(e))?;
            let dns = call.require::<dyn DnsProvider>()?;
            call.after_commit(move |ctx| async move { dns.apply(&ctx, &row).await });
            Ok(Saved { id })
        })
}

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/fixtures/descriptor-dns-zone-edit.json")
}

#[test]
fn the_zone_edit_descriptor_matches_the_committed_snapshot() {
    let descriptor = edit().descriptor().expect("the example compiles");
    let produced = serde_json::to_value(&descriptor).expect("serialises");
    let path = fixture_path();
    if std::env::var_os("KEEL_WRITE_FIXTURES").is_some() {
        let mut text = serde_json::to_string_pretty(&produced).expect("pretty");
        text.push('\n');
        fs::write(&path, text).expect("write the fixture");
    }
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let committed: Value = serde_json::from_str(&text).expect("the fixture is JSON");
    assert_eq!(
        produced,
        committed,
        "the descriptor of dns.zone.edit differs from {}; run with KEEL_WRITE_FIXTURES=1 to refresh it",
        path.display()
    );
    let back: Descriptor = serde_json::from_value(committed).expect("the fixture deserialises");
    assert_eq!(back, descriptor);
}

#[test]
fn the_zone_edit_descriptor_carries_the_parts_of_the_example() {
    let descriptor = edit().descriptor().expect("compiles");
    assert_eq!(descriptor.keel, 1);
    assert_eq!(descriptor.name, "dns.zone.edit");
    assert_eq!(descriptor.module, "dns");
    assert_eq!(descriptor.kind, Kind::Form);
    assert_eq!(descriptor.mode, Mode::Write);
    assert_eq!(
        serde_json::to_value(&descriptor.access).unwrap(),
        json!({ "level": "user", "permission": "dns.zone.manage", "feature": "dns.zones" })
    );
    assert_eq!(descriptor.key.as_deref(), Some("id"));
    let params = descriptor.params.as_value();
    assert_eq!(
        params["properties"]["name"]["x-keel"],
        json!({ "check": "domain" })
    );
    assert_eq!(
        params["properties"]["master_ips"]["x-ui"],
        json!({ "control": "tags" })
    );
    assert_eq!(
        params["properties"]["tsig_key"]["x-keel"],
        json!({ "secret": true })
    );
    assert_eq!(params["$defs"]["Secret"]["writeOnly"], json!(true));
    assert_eq!(
        params["properties"]["account"]["x-keel"],
        json!({ "level": "reseller", "options": "keel.account.options" })
    );
    assert_eq!(
        descriptor.result.as_value()["properties"]["id"]["$ref"],
        json!("#/$defs/Id")
    );
    let ui = descriptor.ui.expect("a form has a screen");
    assert_eq!(ui.title, "dns.zone.edit.title");
    assert_eq!(ui.read.as_deref(), Some("dns.zone.get"));
    assert_eq!(ui.pages.len(), 2);
    assert_eq!(
        ui.pages[1].show_if,
        Some(Expr::parse("kind == 'slave'").unwrap())
    );
    assert_eq!(
        ui.fields["name"].readonly_if,
        Some(Expr::parse("$mode == 'edit'").unwrap())
    );
    assert_eq!(
        ui.fields["master_ips"].required_if,
        Some(Expr::parse("kind == 'slave'").unwrap())
    );
    assert_eq!(
        ui.fields["ttl"].recompute.as_deref(),
        Some("dns.zone.recompute")
    );
    assert_eq!(ui.multi, ["ttl"]);
}

#[test]
fn owned_records_the_table_and_the_account_column_of_the_entity() {
    let def = edit();
    assert_eq!(
        def.ownership(),
        &Ownership::Owned(OwnedDef {
            table: "dns_zone".into(),
            account_column: "account_id".into(),
            primary_key: vec!["id".into()],
        })
    );
    assert_eq!(
        def.owned().map(|owned| owned.account_column.as_str()),
        Some("account_id")
    );
    assert_eq!(def.key(), Some("id"));
}

#[test]
fn the_conditions_stay_as_sources_until_the_kernel_compiles_them() {
    let def = edit();
    let ui = def.ui().expect("ui");
    assert_eq!(
        ui.conditions(),
        &[
            ConditionSource {
                target: ConditionTarget::Page("transfer".into()),
                source: "kind == 'slave'".into()
            },
            ConditionSource {
                target: ConditionTarget::Field("name".into(), FieldCondition::ReadonlyIf),
                source: "$mode == 'edit'".into()
            },
            ConditionSource {
                target: ConditionTarget::Field("master_ips".into(), FieldCondition::RequiredIf),
                source: "kind == 'slave'".into()
            },
        ]
    );
    assert!(ui.screen().pages[1].show_if.is_none());
    assert!(def.known_fields().contains("master_ips"));
}

#[test]
fn a_condition_on_a_field_the_schema_lacks_fails_the_descriptor() {
    let def = function::<ZoneEdit, Saved>("dns.zone.edit")
        .kind(Kind::Form)
        .ui(Form::new("dns.zone.edit.title")
            .page("main", ["name"])
            .field("name")
            .readonly_if("locked"))
        .handler(|_: Call<ZoneEdit>| async { Err(KeelError::internal("dns.zone.error.stub")) });
    let error = def.descriptor().expect_err("locked is not a field");
    assert!(error.to_string().contains("locked"), "{error}");
}
