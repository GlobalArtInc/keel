use keel_sdk::KeelError;
use keel_types::{Descriptor, ErrorCode, FieldError, Kind, Level};
use serde_json::{Map, Value};

use super::schema::{Tree, push_index, push_segment};

pub const MASK: &str = "***";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan(Tree);

impl Plan {
    pub fn of_params(descriptor: &Descriptor) -> Self {
        Self(Tree::of(descriptor.params.as_value()))
    }

    pub fn of_result(descriptor: &Descriptor) -> Self {
        let mut tree = Tree::of(descriptor.result.as_value());
        if descriptor.kind == Kind::List
            && let Some(ui) = &descriptor.ui
            && let Some(row) = tree
                .fields
                .get_mut("data")
                .and_then(|data| data.nested.items.as_deref_mut())
        {
            for column in &ui.columns {
                if let Some(level) = column.access.as_ref().and_then(|access| access.level) {
                    let node = row.fields.entry(column.key.clone()).or_default();
                    node.attrs.level =
                        Some(node.attrs.level.map_or(level, |known| known.max(level)));
                }
            }
        }
        Self(tree)
    }

    pub fn forbidden(&self, level: Level, params: &Value) -> Result<(), KeelError> {
        let mut paths = Vec::new();
        collect_forbidden(&self.0, level, params, &mut String::new(), &mut paths);
        if paths.is_empty() {
            return Ok(());
        }
        Err(
            KeelError::new(ErrorCode::ForbiddenField).fields(paths.into_iter().map(|path| {
                FieldError {
                    path,
                    code: None,
                    key: None,
                    params: Map::new(),
                }
            })),
        )
    }

    pub fn strip(&self, level: Level, value: &mut Value) {
        strip(&self.0, level, value);
    }

    pub fn mask(&self, value: &mut Value) {
        mask(&self.0, value);
    }

    pub fn row_visible(&self, key: &str, level: Level) -> bool {
        self.0
            .fields
            .get("data")
            .and_then(|data| data.nested.items.as_deref())
            .and_then(|row| row.fields.get(key))
            .and_then(|node| node.attrs.level)
            .is_none_or(|required| level >= required)
    }
}

fn strip(tree: &Tree, level: Level, value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.retain(|name, _| {
                tree.fields
                    .get(name)
                    .and_then(|node| node.attrs.level)
                    .is_none_or(|required| level >= required)
            });
            for (name, nested) in object.iter_mut() {
                if let Some(node) = tree.fields.get(name) {
                    strip(&node.nested, level, nested);
                }
            }
        }
        Value::Array(items) => {
            if let Some(nested) = &tree.items {
                for item in items {
                    strip(nested, level, item);
                }
            }
        }
        _ => {}
    }
}

fn mask(tree: &Tree, value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (name, nested) in object.iter_mut() {
                if let Some(node) = tree.fields.get(name) {
                    if node.attrs.secret && !nested.is_null() {
                        *nested = Value::String(MASK.to_owned());
                    } else {
                        mask(&node.nested, nested);
                    }
                }
            }
        }
        Value::Array(items) => {
            if let Some(nested) = &tree.items {
                for item in items {
                    mask(nested, item);
                }
            }
        }
        _ => {}
    }
}

fn collect_forbidden(
    tree: &Tree,
    level: Level,
    value: &Value,
    path: &mut String,
    into: &mut Vec<String>,
) {
    match value {
        Value::Object(object) => {
            for (name, nested) in object {
                let Some(node) = tree.fields.get(name) else {
                    continue;
                };
                let start = path.len();
                push_segment(path, name);
                if node.attrs.level.is_some_and(|required| level < required) {
                    if !nested.is_null() {
                        into.push(path.clone());
                    }
                } else {
                    collect_forbidden(&node.nested, level, nested, path, into);
                }
                path.truncate(start);
            }
        }
        Value::Array(items) => {
            if let Some(nested) = &tree.items {
                for (index, item) in items.iter().enumerate() {
                    let start = path.len();
                    push_index(path, index);
                    collect_forbidden(nested, level, item, path, into);
                    path.truncate(start);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use keel_sdk::prelude::*;
    use keel_types::ColumnType;

    use super::*;

    #[derive(Deserialize, Serialize, JsonSchema)]
    struct Edit {
        title: String,
        #[schemars(extend("x-keel" = json!({ "level": "reseller" })))]
        account: Option<Id>,
        #[schemars(extend("x-keel" = json!({ "secret": true })))]
        password: Option<Secret>,
        tags: Vec<Tag>,
    }

    #[derive(Deserialize, Serialize, JsonSchema)]
    struct Tag {
        name: String,
        #[schemars(extend("x-keel" = json!({ "level": "admin" })))]
        weight: u32,
    }

    #[derive(Deserialize, Serialize, JsonSchema)]
    struct Row {
        id: Id,
        title: String,
        account_id: String,
        #[schemars(extend("x-keel" = json!({ "level": "admin" })))]
        note: String,
    }

    fn form() -> Descriptor {
        function::<Edit, Row>("t.item.edit")
            .kind(Kind::Form)
            .level(Level::User)
            .unowned()
            .handler(|_: Call<Edit>| async { unreachable!() })
            .descriptor()
            .unwrap()
    }

    fn list() -> Descriptor {
        function::<ListQuery, Page<Row>>("t.item.list")
            .kind(Kind::List)
            .level(Level::User)
            .unowned()
            .ui(List::new("t.item.list.title")
                .column(Column::new("title", ColumnType::Text))
                .column(Column::new("account_id", ColumnType::Text).level(Level::Reseller)))
            .handler(|_: Call<ListQuery>| async { unreachable!() })
            .descriptor()
            .unwrap()
    }

    #[test]
    fn fields_above_the_level_are_refused_on_input_with_their_paths() {
        let plan = Plan::of_params(&form());
        let params = json!({
            "title": "x", "account": "01J8A0000000000000000000A1",
            "tags": [ { "name": "a" }, { "name": "b", "weight": 2 } ]
        });
        let error = plan.forbidden(Level::User, &params).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ForbiddenField);
        let wire = error.to_call_error();
        let paths: Vec<&str> = wire.fields.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["account", "tags[1].weight"]);
        plan.forbidden(Level::Admin, &params).unwrap();
        plan.forbidden(Level::User, &json!({ "title": "x", "account": null }))
            .unwrap();
        plan.forbidden(Level::User, &json!({ "title": "x", "unknown": 1 }))
            .unwrap();
    }

    #[test]
    fn fields_above_the_level_are_cut_from_results_including_list_rows_and_columns() {
        let plan = Plan::of_result(&list());
        let mut page = json!({
            "data": [ { "id": "01J8A0000000000000000000A1", "title": "x", "account_id": "a", "note": "n" } ],
            "total": 1, "totals": null
        });
        plan.strip(Level::User, &mut page);
        assert_eq!(
            page["data"][0],
            json!({ "id": "01J8A0000000000000000000A1", "title": "x" })
        );
        let mut page =
            json!({ "data": [ { "title": "x", "account_id": "a", "note": "n" } ], "total": 1 });
        plan.strip(Level::Reseller, &mut page);
        assert_eq!(page["data"][0], json!({ "title": "x", "account_id": "a" }));
        let mut page =
            json!({ "data": [ { "title": "x", "account_id": "a", "note": "n" } ], "total": 1 });
        plan.strip(Level::Admin, &mut page);
        assert_eq!(page["data"][0]["note"], json!("n"));

        let plan = Plan::of_result(&form());
        let mut row = json!({ "id": "01J8A0000000000000000000A1", "title": "x", "note": "n" });
        plan.strip(Level::User, &mut row);
        assert_eq!(
            row,
            json!({ "id": "01J8A0000000000000000000A1", "title": "x" })
        );
    }

    #[test]
    fn secret_fields_are_masked_and_empty_plans_are_cheap() {
        let plan = Plan::of_params(&form());
        let mut params = json!({ "title": "x", "password": "hunter2", "tags": [] });
        plan.mask(&mut params);
        assert_eq!(params["password"], json!(MASK));
        assert_eq!(params["title"], json!("x"));
        let mut absent = json!({ "title": "x", "password": null });
        plan.mask(&mut absent);
        assert_eq!(absent["password"], Value::Null);
        assert_eq!(Plan::of_params(&list()), Plan::default());
        let mut scalar = json!(7);
        plan.strip(Level::Guest, &mut scalar);
        assert_eq!(scalar, json!(7));
    }
}
