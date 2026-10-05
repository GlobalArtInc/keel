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
    use keel_sdk::Owned;
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
        pub link_principal_id: Option<String>,
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

pub mod token {
    use keel_sdk::Owned;
    use keel_sdk::schemars::JsonSchema;
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::FromJsonQueryResult;
    use keel_sdk::sea_orm::entity::prelude::*;
    use keel_sdk::serde::{Deserialize, Serialize};
    use keel_sdk::serde_json;
    use keel_sdk::types::Scope;

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
    #[serde(crate = "keel_sdk::serde", rename_all = "snake_case")]
    #[schemars(
        crate = "keel_sdk::schemars",
        rename = "Scope",
        rename_all = "snake_case"
    )]
    pub enum TokenScope {
        Full,
        Functions(Vec<String>),
        ReadOnly,
    }

    impl TokenScope {
        pub fn of(scope: &Scope) -> Self {
            match scope {
                Scope::Full => Self::Full,
                Scope::Functions(masks) => Self::Functions(masks.clone()),
                Scope::ReadOnly => Self::ReadOnly,
                Scope::Peer(peer) => Self::Functions(peer.functions.clone()),
            }
        }
    }

    impl From<TokenScope> for Scope {
        fn from(scope: TokenScope) -> Self {
            match scope {
                TokenScope::Full => Scope::Full,
                TokenScope::Functions(masks) => Scope::Functions(masks),
                TokenScope::ReadOnly => Scope::ReadOnly,
            }
        }
    }

    #[derive(
        Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, FromJsonQueryResult,
    )]
    #[serde(crate = "keel_sdk::serde", transparent)]
    #[schemars(crate = "keel_sdk::schemars")]
    pub struct StoredScope(pub TokenScope);

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
        #[serde(skip)]
        pub link_principal_id: Option<String>,
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

pub mod login_key {
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "auth_login_key")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub key_hash: String,
        pub principal_id: String,
        pub issued_by: String,
        pub redirect: String,
        pub expires_at: DateTimeUtc,
        pub used_at: Option<DateTimeUtc>,
        pub created_at: DateTimeUtc,
        pub link_principal_id: Option<String>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod ip_rule {
    use keel_sdk::schemars::JsonSchema;
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::entity::prelude::*;
    use keel_sdk::serde::Serialize;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, JsonSchema)]
    #[sea_orm(table_name = "auth_ip_rule")]
    #[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
    #[schemars(
        crate = "keel_sdk::schemars",
        rename = "IpRule",
        rename_all = "camelCase"
    )]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub action: String,
        pub cidr: String,
        pub level: String,
        pub comment: Option<String>,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod grant {
    use keel_sdk::Owned;
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "auth_grant")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub principal_id: String,
        pub account_id: String,
        pub key: String,
        pub granted: bool,
        pub created_at: DateTimeUtc,
        pub created_by: String,
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

pub mod principal_address {
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "auth_principal_address")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub principal_id: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub kind: String,
        pub value: String,
        pub verified_at: Option<DateTimeUtc>,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod mail_token {
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "auth_mail_token")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub token_hash: String,
        pub purpose: String,
        pub principal_id: String,
        pub value: Option<String>,
        pub expires_at: DateTimeUtc,
        pub used_at: Option<DateTimeUtc>,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}
