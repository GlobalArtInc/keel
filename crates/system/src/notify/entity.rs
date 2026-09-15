pub mod problem {
    use keel_sdk::Owned;
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "notify_problem")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub key: String,
        pub subject: String,
        pub severity: String,
        #[sea_orm(column_type = "JsonBinary")]
        pub params_json: Json,
        pub account_id: Option<String>,
        pub source: String,
        pub count: i64,
        pub raised_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
        pub cleared_at: Option<DateTimeUtc>,
        pub open: bool,
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

pub mod message {
    use keel_sdk::Owned;
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "notify_message")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub principal_id: String,
        pub account_id: String,
        pub key: String,
        #[sea_orm(column_type = "JsonBinary")]
        pub params_json: Json,
        pub level: String,
        pub source: String,
        pub problem_id: Option<String>,
        pub read_at: Option<DateTimeUtc>,
        pub created_at: DateTimeUtc,
        pub unread: bool,
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
