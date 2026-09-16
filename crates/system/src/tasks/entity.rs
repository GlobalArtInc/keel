pub mod job {
    use keel_sdk::Owned;
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "task_job")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub class: String,
        pub function: String,
        #[sea_orm(column_type = "JsonBinary")]
        pub params_json: Json,
        #[sea_orm(column_type = "JsonBinary")]
        pub params_masked_json: Json,
        pub principal_id: String,
        #[sea_orm(column_type = "JsonBinary")]
        pub principal_json: Json,
        pub account_id: String,
        pub state: String,
        pub resumable: bool,
        pub progress_done: i64,
        pub progress_total: Option<i64>,
        pub message: Option<String>,
        #[sea_orm(column_type = "JsonBinary", nullable)]
        pub checkpoint_json: Option<Json>,
        #[sea_orm(column_type = "JsonBinary", nullable)]
        pub result_json: Option<Json>,
        #[sea_orm(column_type = "JsonBinary", nullable)]
        pub error_json: Option<Json>,
        pub request_id: Option<String>,
        pub created_at: DateTimeUtc,
        pub started_at: Option<DateTimeUtc>,
        pub finished_at: Option<DateTimeUtc>,
        pub file_name: Option<String>,
        pub file_type: Option<String>,
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

pub mod step {
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "task_step")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub job_id: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub seq: i64,
        pub at: DateTimeUtc,
        pub done: i64,
        pub total: Option<i64>,
        pub message: Option<String>,
        #[sea_orm(column_type = "JsonBinary", nullable)]
        pub checkpoint_json: Option<Json>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}
