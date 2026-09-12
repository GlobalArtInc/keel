use keel_sdk::prelude::*;

use crate::products::{self, Composed, ProductsError};

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
pub struct ProductRow {
    pub id: String,
    pub name: String,
    pub version: String,
    pub state: ProductState,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ProductChange {
    pub id: String,
    pub state: ProductState,
    pub changed: bool,
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
                .bundles()
                .iter()
                .map(|bundle| ProductRow {
                    id: bundle.id.to_owned(),
                    name: bundle.name.to_owned(),
                    version: bundle.version.to_owned(),
                    state: state_of(&enabled, bundle.id),
                })
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

async fn change(call: &Call<ProductPick>, wanted: ProductState) -> Result<ProductChange> {
    let id = call.params().id.as_str();
    let composed = call.require::<Composed>()?;
    if composed.bundle(id).is_none() {
        return Err(KeelError::validation("keel.product.error.unknown")
            .field("id")
            .param("id", id));
    }
    let tx = call.tx()?;
    let mut enabled = products::enabled(tx).await.map_err(stored)?;
    let changed = match wanted {
        ProductState::Enabled if !enabled.iter().any(|known| known == id) => {
            enabled.push(id.to_owned());
            true
        }
        ProductState::Disabled if enabled.iter().any(|known| known == id) => {
            enabled.retain(|known| known != id);
            true
        }
        ProductState::Enabled | ProductState::Disabled => false,
    };
    if changed {
        products::write(tx, &enabled).await?;
    }
    Ok(ProductChange {
        id: id.to_owned(),
        state: wanted,
        changed,
    })
}

fn state_of(enabled: &[String], id: &str) -> ProductState {
    if enabled.iter().any(|known| known == id) {
        ProductState::Enabled
    } else {
        ProductState::Disabled
    }
}

fn stored(error: ProductsError) -> KeelError {
    match error {
        ProductsError::Database(error) => KeelError::from(error),
        ProductsError::Stored { reason } => {
            KeelError::internal("keel.product.error.stored").param("reason", reason)
        }
    }
}
