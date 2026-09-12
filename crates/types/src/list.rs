use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const DEFAULT_PAGE: u32 = 1;
pub const DEFAULT_PAGE_SIZE: u32 = 25;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct ListQuery {
    pub page: u32,
    pub page_size: u32,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub search: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<Sort>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<Filter>,
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub scope: Map<String, Value>,
}

impl Default for ListQuery {
    fn default() -> Self {
        Self {
            page: DEFAULT_PAGE,
            page_size: DEFAULT_PAGE_SIZE,
            search: String::new(),
            sort: None,
            filters: Vec::new(),
            scope: Map::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Sort {
    pub key: String,
    pub dir: SortDir,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SortDir {
    #[default]
    Asc,
    Desc,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Filter {
    pub key: String,
    #[serde(default, skip_serializing_if = "FilterOp::is_eq")]
    pub op: FilterOp,
    pub value: FilterValue,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FilterOp {
    #[default]
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
}

impl FilterOp {
    pub fn is_eq(&self) -> bool {
        *self == FilterOp::Eq
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum FilterValue {
    One(String),
    Many(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "Page_{T}")]
pub struct Page<T> {
    pub data: Vec<T>,
    pub total: u64,
    pub totals: Option<Map<String, Value>>,
}

impl<T> Page<T> {
    pub fn new(data: Vec<T>, total: u64) -> Self {
        Self {
            data,
            total,
            totals: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_empty_query_takes_the_defaults() {
        let query: ListQuery = serde_json::from_value(json!({})).unwrap();
        assert_eq!(query, ListQuery::default());
        assert_eq!(query.page, 1);
        assert_eq!(query.page_size, 25);
        assert_eq!(
            serde_json::to_value(&query).unwrap(),
            json!({ "page": 1, "pageSize": 25 })
        );
    }

    #[test]
    fn filter_op_defaults_to_eq() {
        let filter: Filter =
            serde_json::from_value(json!({ "key": "type", "value": ["A", "AAAA"] })).unwrap();
        assert_eq!(filter.op, FilterOp::Eq);
        assert_eq!(
            filter.value,
            FilterValue::Many(vec!["A".into(), "AAAA".into()])
        );
        assert_eq!(
            serde_json::to_value(&filter).unwrap(),
            json!({ "key": "type", "value": ["A", "AAAA"] })
        );
        let filter: Filter =
            serde_json::from_value(json!({ "key": "ttl", "op": "gte", "value": "300" })).unwrap();
        assert_eq!(filter.op, FilterOp::Gte);
        assert_eq!(filter.value, FilterValue::One("300".into()));
    }

    #[test]
    fn unknown_sort_dir_filter_op_and_value_shape_fail_to_deserialise() {
        assert!(serde_json::from_value::<SortDir>(json!("ascending")).is_err());
        assert!(serde_json::from_value::<FilterOp>(json!("like")).is_err());
        assert!(serde_json::from_value::<Filter>(json!({ "key": "ttl", "value": 300 })).is_err());
    }

    #[test]
    fn page_schema_is_named_after_its_row_type() {
        #[derive(Serialize, schemars::JsonSchema)]
        struct ZoneRow {
            name: String,
        }
        let schema = schemars::schema_for!(Page<ZoneRow>);
        let value = schema.as_value();
        assert_eq!(value["title"], json!("Page_ZoneRow"));
        assert_eq!(
            value["properties"]["data"]["items"],
            json!({ "$ref": "#/$defs/ZoneRow" })
        );
        assert_eq!(value["required"], json!(["data", "total"]));
        assert_eq!(
            <Page<Value> as schemars::JsonSchema>::schema_name(),
            "Page_AnyValue"
        );
    }

    #[test]
    fn page_carries_totals_as_null_when_none_are_declared() {
        let page = Page::new(vec![json!({ "id": 1 })], 7);
        assert_eq!(
            serde_json::to_value(&page).unwrap(),
            json!({ "data": [ { "id": 1 } ], "total": 7, "totals": null })
        );
    }
}
