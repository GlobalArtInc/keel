pub mod run {
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "cron_run")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub job: String,
        pub scheduled_at: DateTimeUtc,
        pub started_at: DateTimeUtc,
        pub finished_at: Option<DateTimeUtc>,
        pub outcome: String,
        pub error: Option<String>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}
