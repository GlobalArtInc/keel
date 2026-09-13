use keel_sdk::prelude::*;
use keel_sdk::types::{Filter, FilterOp, FilterValue};

use super::store::{Chain, Definition, Store};

pub const PERMISSION_READ: &str = "settings.read";
pub const PERMISSION_MANAGE: &str = "settings.manage";

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct SettingRow {
    pub key: String,
    pub module: String,
    pub schema: Value,
    pub default: Option<Value>,
    pub level: Level,
    pub value: Option<Value>,
    pub scope: Option<Id>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Pick {
    #[schemars(length(min = 1, max = 128))]
    pub key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct SettingSet {
    #[schemars(length(min = 1, max = 128))]
    pub key: String,
    pub value: Value,
}

fn row_of(definition: &Definition, chain: &Chain) -> SettingRow {
    let def = definition.def();
    let resolved = chain.resolve(definition);
    let mut schema = def.schema().clone();
    schema.remove("$schema");
    SettingRow {
        key: def.key().to_owned(),
        module: def.module().to_owned(),
        schema: schema.to_value(),
        default: def.default_value().cloned(),
        level: def.level_value(),
        value: resolved.value,
        scope: resolved.scope,
    }
}

fn visible<'a>(store: &'a Store, call: &Call<impl Send>, key: &str) -> Result<&'a Definition> {
    let definition = store
        .definition(key)?
        .ok_or_else(|| KeelError::not_found("keel.error.not_found"))?;
    let level = definition.def().level_value();
    if level > call.principal().level {
        return Err(KeelError::forbidden("settings.error.level")
            .param("key", key)
            .param("level", level.as_str())
            .param("caller", call.principal().level.as_str()));
    }
    Ok(definition)
}

fn column_text(row: &SettingRow, key: &str) -> Option<String> {
    match key {
        "key" => Some(row.key.clone()),
        "module" => Some(row.module.clone()),
        "level" => Some(row.level.as_str().to_owned()),
        "value" => row.value.as_ref().map(text_of),
        "scope" => row.scope.map(|scope| scope.to_string()),
        _ => None,
    }
}

fn text_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn matches(row: &SettingRow, filter: &Filter, index: usize) -> Result<bool> {
    let actual = column_text(row, &filter.key);
    Ok(match (&filter.value, filter.op) {
        (FilterValue::Many(wanted), FilterOp::Eq) => {
            actual.is_some_and(|actual| wanted.contains(&actual))
        }
        (FilterValue::Many(wanted), FilterOp::Ne) => {
            !actual.is_some_and(|actual| wanted.contains(&actual))
        }
        (FilterValue::Many(_), _) => {
            return Err(KeelError::validation("keel.error.list.list_value_op")
                .field(format!("filters[{index}].op"))
                .param("field", filter.key.clone()));
        }
        (FilterValue::One(wanted), op) => {
            let Some(actual) = actual else {
                return Ok(op == FilterOp::Ne);
            };
            match op {
                FilterOp::Eq => actual == *wanted,
                FilterOp::Ne => actual != *wanted,
                FilterOp::Gt => actual > *wanted,
                FilterOp::Gte => actual >= *wanted,
                FilterOp::Lt => actual < *wanted,
                FilterOp::Lte => actual <= *wanted,
            }
        }
    })
}

struct SettingSource(Arc<Store>);

#[async_trait]
impl ListSource for SettingSource {
    type Row = SettingRow;

    async fn fetch(&self, call: &Call<ListQuery>, spec: &ListSpec) -> Result<Page<SettingRow>> {
        let query = call.params();
        let chain = self.0.chain(call.principal().account_id, call.db()).await?;
        let search = query.search.to_lowercase();
        let mut rows = Vec::new();
        for definition in self.0.visible(call.principal().level)? {
            let row = row_of(definition, &chain);
            let searched = search.is_empty()
                || spec.search.iter().any(|key| {
                    column_text(&row, key).is_some_and(|text| text.to_lowercase().contains(&search))
                });
            if !searched {
                continue;
            }
            let mut kept = true;
            for (index, filter) in query.filters.iter().enumerate() {
                if !matches(&row, filter, index)? {
                    kept = false;
                    break;
                }
            }
            if kept {
                rows.push(row);
            }
        }
        if let Some(sort) = &query.sort {
            rows.sort_by(|a, b| {
                let order = column_text(a, &sort.key).cmp(&column_text(b, &sort.key));
                match sort.dir {
                    SortDir::Asc => order,
                    SortDir::Desc => order.reverse(),
                }
                .then_with(|| a.key.cmp(&b.key))
            });
        }
        let total = rows.len() as u64;
        let page_size = query.page_size as usize;
        let start = (query.page as usize - 1).saturating_mul(page_size);
        let data = rows.into_iter().skip(start).take(page_size).collect();
        Ok(Page {
            data,
            total,
            totals: None,
        })
    }
}

pub fn list(store: Arc<Store>) -> FunctionDef {
    function::<ListQuery, Page<SettingRow>>("settings.list")
        .kind(Kind::List)
        .access(Level::Reseller, PERMISSION_READ)
        .unowned()
        .ui(List::new("settings.list.title")
            .search(["key", "module"])
            .column(Column::new("key", ColumnType::Text).sortable().primary())
            .column(Column::new("module", ColumnType::Text).sortable())
            .column(
                Column::new("level", ColumnType::Text)
                    .sortable()
                    .filter_options(["user", "reseller", "admin"]),
            )
            .column(Column::new("value", ColumnType::Text))
            .column(Column::new("scope", ColumnType::Text))
            .action(
                Action::new("set", ActionRole::Edit)
                    .screen("settings.set")
                    .params([("key", "$key")])
                    .icon("pencil"),
            )
            .row_actions(["set"])
            .empty("settings.empty", None)
            .invalidate_on(["settings.*"]))
        .default_sort("key", SortDir::Asc)
        .source(SettingSource(store))
}

pub fn get(store: Arc<Store>) -> FunctionDef {
    function::<Pick, SettingRow>("settings.get")
        .kind(Kind::Get)
        .access(Level::Reseller, PERMISSION_READ)
        .unowned()
        .handler(move |call: Call<Pick>| {
            let store = store.clone();
            async move {
                let definition = visible(&store, &call, &call.params().key)?;
                let chain = store.chain(call.principal().account_id, call.db()).await?;
                Ok(row_of(definition, &chain))
            }
        })
}

pub fn scope_of(principal: &Principal) -> Id {
    if principal.level >= Level::Admin {
        Id::ROOT_ACCOUNT
    } else {
        principal.account_id
    }
}

pub fn set(store: Arc<Store>) -> FunctionDef {
    function::<SettingSet, SettingRow>("settings.set")
        .kind(Kind::Form)
        .access(Level::Reseller, PERMISSION_MANAGE)
        .unowned()
        .ui(Form::new("settings.set.title")
            .read("settings.get")
            .page("main", ["key", "value"])
            .field("key")
            .readonly_if("$mode == 'edit'"))
        .handler(move |call: Call<SettingSet>| {
            let store = store.clone();
            async move {
                let params = call.params();
                let definition = visible(&store, &call, &params.key)?;
                definition.check(&params.value)?;
                let scope = scope_of(call.principal());
                let now = call.require::<dyn Clock>()?.now();
                store
                    .write(
                        call.tx()?,
                        scope,
                        &params.key,
                        params.value.clone(),
                        call.principal().id,
                        now,
                    )
                    .await?;
                let chain = store.chain(call.principal().account_id, call.tx()?).await?;
                Ok(row_of(definition, &chain))
            }
        })
}
