use chrono::{DateTime, Utc};
use keel_sdk::prelude::*;

use crate::products::{self, Composed, EnabledSet, ProductsError};

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ProductPick {
    pub id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ProductState {
    Enabled,
    Disabled,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProductRow {
    pub id: String,
    pub name: String,
    pub version: String,
    pub state: ProductState,
    pub enabled_at: Option<DateTime<Utc>>,
    pub enabled_by: Option<String>,
    pub modules: Vec<String>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ProductChange {
    pub id: String,
    pub state: ProductState,
    pub changed: bool,
    pub restart: bool,
}

pub fn list() -> FunctionDef {
    function::<ListQuery, Page<ProductRow>>("keel.product.list")
        .kind(Kind::List)
        .level(Level::Admin)
        .unowned()
        .ui(List::new("keel.product.list.title")
            .column(Column::new("id", ColumnType::Text).primary())
            .column(Column::new("name", ColumnType::Text))
            .column(Column::new("version", ColumnType::Text))
            .column(
                Column::new("state", ColumnType::Status)
                    .map([("enabled", Tone::Success), ("disabled", Tone::Neutral)]),
            )
            .column(Column::new("enabledAt", ColumnType::Datetime))
            .column(Column::new("enabledBy", ColumnType::Text))
            .column(Column::new("modules", ColumnType::Text))
            .action(
                Action::new("enable", ActionRole::Button)
                    .function("keel.product.enable")
                    .params([("id", "$id")])
                    .confirm(),
            )
            .action(
                Action::new("disable", ActionRole::Button)
                    .function("keel.product.disable")
                    .params([("id", "$id")])
                    .confirm(),
            )
            .row_actions(["enable", "disable"])
            .empty("keel.product.empty", None)
            .invalidate_on(["keel.product.*"]))
        .handler(|call: Call<ListQuery>| async move {
            let composed = call.require::<Composed>()?;
            let enabled = products::enabled(call.db()).await.map_err(stored)?;
            let rows: Vec<ProductRow> = composed
                .manifests()
                .iter()
                .map(|manifest| row(manifest, &enabled))
                .collect();
            let total = rows.len() as u64;
            Ok(Page::new(rows, total))
        })
}

pub fn enable() -> FunctionDef {
    function::<ProductPick, ProductChange>("keel.product.enable")
        .kind(Kind::Action)
        .level(Level::Admin)
        .unowned()
        .handler(
            |call: Call<ProductPick>| async move { change(&call, ProductState::Enabled).await },
        )
}

pub fn disable() -> FunctionDef {
    function::<ProductPick, ProductChange>("keel.product.disable")
        .kind(Kind::Action)
        .level(Level::Admin)
        .unowned()
        .handler(
            |call: Call<ProductPick>| async move { change(&call, ProductState::Disabled).await },
        )
}

fn row(manifest: &ProductManifest, enabled: &EnabledSet) -> ProductRow {
    let record = enabled.get(manifest.id);
    ProductRow {
        id: manifest.id.to_owned(),
        name: manifest.name.to_owned(),
        version: manifest.version.to_owned(),
        state: match record {
            Some(_) => ProductState::Enabled,
            None => ProductState::Disabled,
        },
        enabled_at: record.map(|record| record.enabled_at),
        enabled_by: record.map(|record| record.by.clone()),
        modules: manifest.module_ids().map(str::to_owned).collect(),
    }
}

async fn change(call: &Call<ProductPick>, wanted: ProductState) -> Result<ProductChange> {
    let id = call.params().id.as_str();
    let composed = call.require::<Composed>()?;
    if composed.manifest(id).is_none() {
        return Err(KeelError::validation("keel.product.error.unknown")
            .field("id")
            .param("id", id));
    }
    let tx = call.tx()?;
    let changed = match wanted {
        ProductState::Enabled => {
            let now = call.require::<dyn Clock>()?.now();
            products::enable(tx, id, &call.principal().login, now).await
        }
        ProductState::Disabled => products::disable(tx, id).await,
    }
    .map_err(stored)?;
    if changed {
        tracing::info!(
            product = id,
            state = ?wanted,
            by = call.principal().login,
            "the enabled product set changed; a restart applies it"
        );
    }
    Ok(ProductChange {
        id: id.to_owned(),
        state: wanted,
        changed,
        restart: changed,
    })
}

fn stored(error: ProductsError) -> KeelError {
    match error {
        ProductsError::Database(error) => KeelError::from(error),
        ProductsError::Stored { reason } => {
            KeelError::internal("keel.product.error.stored").param("reason", reason)
        }
    }
}
