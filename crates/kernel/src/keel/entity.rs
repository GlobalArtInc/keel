pub mod account {
    use keel_sdk::Owned;
    use keel_sdk::schemars::JsonSchema;
    use keel_sdk::sea_orm::entity::prelude::*;
    use keel_sdk::serde::Serialize;
    use keel_sdk::serde_json::json;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, JsonSchema)]
    #[sea_orm(table_name = "keel_account")]
    #[schemars(rename = "Account")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub kind: String,
        #[sea_orm(column_name = "parent_id")]
        #[schemars(extend("x-keel" = json!({ "level": "reseller" })))]
        pub parent: Option<String>,
        #[schemars(extend("x-keel" = json!({ "level": "reseller" })))]
        pub path: String,
        pub state: String,
        #[schemars(extend("x-keel" = json!({ "level": "admin" })))]
        pub external_id: Option<String>,
        #[schemars(extend("x-keel" = json!({ "level": "admin" })))]
        pub origin: Option<String>,
        pub created_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    impl Owned for Entity {
        fn account_column() -> Self::Column {
            Column::Id
        }
    }
}

pub mod principal {
    use keel_sdk::Owned;
    use keel_sdk::schemars::JsonSchema;
    use keel_sdk::sea_orm::entity::prelude::*;
    use keel_sdk::serde::Serialize;
    use keel_sdk::serde_json::json;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, JsonSchema)]
    #[sea_orm(table_name = "keel_principal")]
    #[schemars(rename = "Principal")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        #[sea_orm(column_name = "account_id")]
        pub account: String,
        pub login: String,
        pub level: String,
        pub role: String,
        pub state: String,
        pub lang: String,
        #[schemars(extend("x-keel" = json!({ "level": "admin" })))]
        pub external_id: Option<String>,
        #[schemars(extend("x-keel" = json!({ "level": "admin" })))]
        pub origin: Option<String>,
        pub acl_gen: i32,
        pub created_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    impl Owned for Entity {
        fn account_column() -> Self::Column {
            Column::Account
        }
    }
}
