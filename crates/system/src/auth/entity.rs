pub mod password {
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "auth_password")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub principal_id: String,
        pub hash: String,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod session {
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "auth_session")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id_hash: String,
        pub principal_id: String,
        pub account_id: String,
        pub actor_principal_id: Option<String>,
        pub kind: String,
        pub ip: Option<String>,
        pub user_agent: Option<String>,
        pub created_at: DateTimeUtc,
        pub idle_until: DateTimeUtc,
        pub absolute_until: DateTimeUtc,
        pub revoked_at: Option<DateTimeUtc>,
        pub reason: Option<String>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}
