use async_trait::async_trait;
use serde_json::Value;

use crate::call::Call;
use crate::error::Result;

#[async_trait]
pub trait Guard: Send + Sync {
    async fn check(&self, function: &str, call: &Call<Value>) -> Result<()>;
}
