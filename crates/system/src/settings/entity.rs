pub mod value {
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "settings_value")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub scope_id: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub key: String,
        #[sea_orm(column_type = "JsonBinary")]
        pub value: Json,
        pub updated_at: DateTimeUtc,
        pub updated_by: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod pref {
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "settings_pref")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub principal_id: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub key: String,
        #[sea_orm(column_type = "JsonBinary")]
        pub value: Json,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod brand {
    use keel_sdk::schemars::JsonSchema;
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::entity::prelude::*;
    use keel_sdk::serde::Serialize;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, JsonSchema)]
    #[sea_orm(table_name = "settings_brand")]
    #[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
    #[schemars(
        crate = "keel_sdk::schemars",
        rename = "BrandRow",
        rename_all = "camelCase"
    )]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub host: String,
        pub name: Option<String>,
        pub logo: Option<String>,
        pub hue: Option<i32>,
        pub chroma: Option<f64>,
        pub updated_at: DateTimeUtc,
        pub updated_by: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}
