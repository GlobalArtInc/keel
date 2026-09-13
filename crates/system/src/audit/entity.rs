pub mod call {
    use keel_sdk::Owned;
    use keel_sdk::schemars::JsonSchema;
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::entity::prelude::*;
    use keel_sdk::serde::Serialize;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, JsonSchema)]
    #[sea_orm(table_name = "audit_call")]
    #[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
    #[schemars(
        crate = "keel_sdk::schemars",
        rename = "AuditCall",
        rename_all = "camelCase"
    )]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub at: DateTimeUtc,
        pub request_id: String,
        pub function: String,
        pub module: String,
        pub principal_id: Option<String>,
        pub principal_login: String,
        pub account_id: String,
        pub actor_principal_id: Option<String>,
        pub actor_login: Option<String>,
        pub via: String,
        pub ip: Option<String>,
        #[sea_orm(column_type = "JsonBinary")]
        pub params_redacted: Json,
        pub subject: Option<String>,
        #[sea_orm(column_type = "JsonBinary", nullable)]
        pub after: Option<Json>,
        pub status: String,
        pub error_code: Option<String>,
        pub error_key: Option<String>,
        pub elapsed_ms: i64,
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
