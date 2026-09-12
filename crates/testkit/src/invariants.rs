use std::collections::BTreeSet;

use keel_sdk::async_trait::async_trait;
use keel_types::{ErrorCode, Id};
use serde_json::{Value, json};

use crate::fixtures::Who;
use crate::harness::Harness;

const DEFAULT_KEY: &str = "id";

#[async_trait]
pub trait Fixture: Send + Sync {
    async fn create(&self, harness: &Harness, account: Id) -> Id;
}

impl Harness {
    pub(crate) fn row_key(&self, function: &str) -> String {
        self.registry()
            .function(function)
            .and_then(|function| function.def().key())
            .unwrap_or(DEFAULT_KEY)
            .to_owned()
    }

    pub async fn assert_reseller_sees_only_subtree(&self, func_list: &str, fixture: &dyn Fixture) {
        let accounts = &self.fixtures().accounts;
        let inside = fixture.create(self, accounts.a1).await;
        let outside = fixture.create(self, accounts.unrelated).await;
        let key = self.row_key(func_list);
        let reseller = self.listed_keys(Who::Reseller, func_list, &key).await;
        assert!(
            reseller.contains(&inside.to_string()),
            "{func_list}: the reseller does not see the row {inside} of account {} in its subtree: {reseller:?}",
            accounts.a1
        );
        assert!(
            !reseller.contains(&outside.to_string()),
            "{func_list}: the reseller sees the row {outside} of the unrelated account {}: {reseller:?}",
            accounts.unrelated
        );
        let unrelated = self.listed_keys(Who::Unrelated, func_list, &key).await;
        assert!(
            !unrelated.contains(&inside.to_string()),
            "{func_list}: the unrelated user sees the row {inside} of account {}: {unrelated:?}",
            accounts.a1
        );
        let owner = self.listed_keys(Who::Owner, func_list, &key).await;
        assert!(
            owner.contains(&inside.to_string()) && owner.contains(&outside.to_string()),
            "{func_list}: the owner does not see both rows {inside} and {outside}: {owner:?}"
        );
    }

    pub async fn assert_user_cannot_edit_foreign_row(
        &self,
        func_edit: &str,
        fixture: &dyn Fixture,
    ) {
        let accounts = &self.fixtures().accounts;
        let foreign = fixture.create(self, accounts.unrelated).await;
        let key = self.row_key(func_edit);
        let params = json!({ key.as_str(): foreign });
        for who in [Who::A1, Who::Reseller] {
            let reply = self.call(who, func_edit, params.clone()).await;
            let code = reply.error().map(|error| error.code);
            assert_eq!(
                code,
                Some(ErrorCode::NotFound),
                "{func_edit}: {who} editing the foreign row {foreign} of account {} must be answered not_found, got {:?}",
                accounts.unrelated,
                reply.envelope
            );
            assert_eq!(
                reply.status(),
                404,
                "{func_edit}: {who}: {:?}",
                reply.envelope
            );
        }
        let owner = self.call(Who::Owner, func_edit, params).await;
        assert_ne!(
            owner.error().map(|error| error.code),
            Some(ErrorCode::NotFound),
            "{func_edit}: the owner cannot reach the row {foreign} either, so the fixture did not create it: {:?}",
            owner.envelope
        );
    }

    async fn listed_keys(&self, who: Who, func_list: &str, key: &str) -> BTreeSet<String> {
        let reply = self.call(who, func_list, json!({})).await;
        let Some(data) = reply.data() else {
            panic!(
                "{func_list}: {who} cannot list: {:?}",
                reply.error().map(|error| (error.code, error.key.clone()))
            );
        };
        let Some(rows) = data.get("data").and_then(Value::as_array) else {
            panic!("{func_list}: the reply carries no data array: {data}");
        };
        rows.iter()
            .map(|row| match row.get(key) {
                Some(Value::String(value)) => value.clone(),
                Some(other) => other.to_string(),
                None => panic!("{func_list}: a row carries no {key}: {row}"),
            })
            .collect()
    }
}
