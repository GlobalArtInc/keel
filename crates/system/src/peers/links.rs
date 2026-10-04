use std::sync::{Arc, OnceLock};

use keel_sdk::async_trait::async_trait;
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use keel_sdk::system::{PeerLink, PeerLinks, SecretCipher};
use keel_sdk::{Db, Id, KeelError, Result, Secret, serde_json};

use super::entity::product;
use super::functions::{ERROR_STORED, STATUS_ACTIVE};

#[derive(Default)]
pub struct Links {
    cipher: OnceLock<Arc<dyn SecretCipher>>,
}

impl Links {
    pub fn attach(&self, cipher: Arc<dyn SecretCipher>) {
        let _ = self.cipher.set(cipher);
    }

    fn cipher(&self) -> Result<&dyn SecretCipher> {
        self.cipher
            .get()
            .map(AsRef::as_ref)
            .ok_or_else(|| KeelError::internal("peer.error.not_started"))
    }
}

pub(super) fn offers(row: &product::Model, product: &str) -> Result<bool> {
    let products: Vec<String> = serde_json::from_value(row.products_json.clone())
        .map_err(|error| stored(row).source(error))?;
    Ok(products.iter().any(|offered| offered == product))
}

fn stored(row: &product::Model) -> KeelError {
    KeelError::internal(ERROR_STORED).param("id", row.id.as_str())
}

#[async_trait]
impl PeerLinks for Links {
    async fn offering(&self, product: &str, db: &Db) -> Result<Option<PeerLink>> {
        let rows = product::Entity::find()
            .filter(product::Column::Status.eq(STATUS_ACTIVE))
            .filter(product::Column::Url.is_not_null())
            .order_by_asc(product::Column::CreatedAt)
            .order_by_asc(product::Column::Id)
            .all(db)
            .await?;
        for row in rows {
            if !offers(&row, product)? {
                continue;
            }
            let Some(url) = row.url.clone() else {
                continue;
            };
            let id: Id = row.id.parse().map_err(|_| stored(&row))?;
            let instance: Id = row.instance_id.parse().map_err(|_| stored(&row))?;
            let token = String::from_utf8(self.cipher()?.open(&row.outbound_token_enc)?)
                .map_err(|_| stored(&row))?;
            return Ok(Some(PeerLink {
                id,
                instance,
                url,
                token: Secret::new(token),
            }));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use keel_sdk::chrono::Utc;
    use keel_sdk::serde_json::json;
    use keel_sdk::types::ErrorCode;

    use super::*;

    fn link(products: serde_json::Value) -> product::Model {
        product::Model {
            id: Id::generate().to_string(),
            instance_id: Id::generate().to_string(),
            products_json: products,
            keel_version: "0.3.0".into(),
            protocol: 1,
            url: Some("https://dns.example:8443".into()),
            public_key: "ed25519:key".into(),
            inbound_token_hash: "hash".into(),
            inbound_scopes_json: json!({}),
            outbound_token_enc: "sealed".into(),
            webhook_secret_enc: None,
            status: STATUS_ACTIVE.into(),
            last_seen_at: None,
            created_at: Utc::now(),
            principal_id: None,
            events: Vec::new(),
            pull_seq: None,
            feed_seq: 0,
            identity: false,
        }
    }

    #[test]
    fn a_link_offers_exactly_the_products_its_peer_published() {
        let both = link(json!(["dns", "panel"]));
        assert!(offers(&both, "dns").unwrap());
        assert!(offers(&both, "panel").unwrap());
        assert!(!offers(&both, "bill").unwrap());
        assert!(!offers(&link(json!([])), "dns").unwrap());
        assert!(!offers(&link(json!(["dnsx"])), "dns").unwrap());
        let broken = link(json!({ "dns": true }));
        let error = offers(&broken, "dns").unwrap_err();
        assert_eq!(error.code(), ErrorCode::Internal);
        assert_eq!(error.key(), ERROR_STORED);
        assert_eq!(error.params()["id"], broken.id.as_str());
    }
}
