pub mod subscription {
    use keel_sdk::Owned;
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "webhook_subscription")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub account_id: String,
        pub principal_id: String,
        pub name: String,
        pub url: String,
        pub secret: String,
        pub events: Vec<String>,
        pub enabled: bool,
        pub created_at: DateTimeUtc,
        pub created_by: String,
        pub updated_at: DateTimeUtc,
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

pub mod delivery {
    use keel_sdk::Owned;
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "webhook_delivery")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub subscription_id: String,
        pub account_id: String,
        pub event_id: String,
        pub seq: i64,
        #[sea_orm(column_name = "type")]
        pub kind: String,
        pub attempt: i32,
        pub state: String,
        pub next_at: DateTimeUtc,
        pub last_status: Option<i32>,
        pub last_error: Option<String>,
        pub delivered_at: Option<DateTimeUtc>,
        pub created_at: DateTimeUtc,
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

pub mod cursor {
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "webhook_cursor")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub consumer: String,
        pub seq: i64,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}
