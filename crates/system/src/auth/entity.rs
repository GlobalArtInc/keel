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
        pub parent_id_hash: Option<String>,
        pub su_id_hash: Option<String>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod token {
    use keel_sdk::Owned;
    use keel_sdk::schemars::JsonSchema;
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::FromJsonQueryResult;
    use keel_sdk::sea_orm::entity::prelude::*;
    use keel_sdk::serde::{Deserialize, Serialize};
    use keel_sdk::serde_json;
    use keel_sdk::types::Scope;

    #[derive(
        Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, FromJsonQueryResult,
    )]
    #[serde(crate = "keel_sdk::serde", transparent)]
    #[schemars(crate = "keel_sdk::schemars")]
    pub struct StoredScope(pub Scope);

    #[derive(
        Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, FromJsonQueryResult,
    )]
    #[serde(crate = "keel_sdk::serde", transparent)]
    #[schemars(crate = "keel_sdk::schemars")]
    pub struct StoredNets(pub Vec<String>);

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, JsonSchema)]
    #[sea_orm(table_name = "auth_token")]
    #[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
    #[schemars(
        crate = "keel_sdk::schemars",
        rename = "Token",
        rename_all = "camelCase"
    )]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        #[serde(skip)]
        pub token_hash: String,
        pub principal_id: String,
        pub account_id: String,
        pub name: String,
        pub level: String,
        #[sea_orm(column_type = "JsonBinary")]
        pub scope: StoredScope,
        #[sea_orm(column_type = "JsonBinary")]
        pub ip_allow: StoredNets,
        pub expires_at: Option<DateTimeUtc>,
        pub last_used_at: Option<DateTimeUtc>,
        pub created_at: DateTimeUtc,
        pub revoked_at: Option<DateTimeUtc>,
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

pub mod totp {
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "auth_totp")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub principal_id: String,
        pub secret: String,
        pub confirmed_at: Option<DateTimeUtc>,
        pub last_step: Option<i64>,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod recovery_code {
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "auth_recovery_code")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub principal_id: String,
        pub code_hash: String,
        pub used_at: Option<DateTimeUtc>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod login_challenge {
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "auth_login_challenge")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub challenge_hash: String,
        pub principal_id: String,
        pub ip: Option<String>,
        pub expires_at: DateTimeUtc,
        pub used_at: Option<DateTimeUtc>,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}
