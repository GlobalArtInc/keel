use std::collections::{BTreeMap, BTreeSet};

use keel_types::descriptor::UiAttrs;
use keel_types::ui::ColumnFilter;
use keel_types::{Descriptor, X_UI};
use schemars::Schema;
use serde_json::{Map, Value};

pub const SHARED_PREFIX: &str = "keel";

const MAX_DEPTH: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivedKey {
    pub member: String,
    pub key: String,
    pub shared: Option<String>,
}

pub fn of(descriptor: &Descriptor, patched: &BTreeMap<String, String>) -> Vec<DerivedKey> {
    let Some(ui) = &descriptor.ui else {
        return Vec::new();
    };
    let prefix = entity_prefix(&descriptor.name);
    let mut keys = Keys {
        prefix: prefix.clone(),
        seen: BTreeSet::new(),
        derived: Vec::new(),
    };
    let properties = properties(&descriptor.params);
    let defs = descriptor.params.get("$defs").and_then(Value::as_object);
    for field in fields(descriptor, properties) {
        let owner = patched
            .get(&field)
            .map(|module| patched_prefix(&prefix, module));
        keys.push_under(owner.as_deref(), &format!("field {field}"), &field);
        let Some(schema) = properties.and_then(|properties| properties.get(&field)) else {
            continue;
        };
        for value in enum_values(schema, defs, 0) {
            keys.push_under(
                owner.as_deref(),
                &format!("field {field} value {value}"),
                &format!("{field}.{value}"),
            );
        }
        if let Some(attrs) = schema
            .get(X_UI)
            .and_then(|value| serde_json::from_value::<UiAttrs>(value.clone()).ok())
        {
            if let Some(placeholder) = attrs.placeholder {
                keys.explicit(&format!("field {field} {X_UI}.placeholder"), &placeholder);
            }
            if let Some(help) = attrs.help {
                keys.explicit(&format!("field {field} {X_UI}.help"), &help);
            }
        }
    }
    for page in &ui.pages {
        keys.push(&format!("page {}", page.id), &format!("page.{}", page.id));
    }
    for action in &ui.actions {
        keys.push(
            &format!("action {}", action.id),
            &format!("action.{}", action.id),
        );
    }
    for column in &ui.columns {
        keys.column(column);
    }
    for band in &ui.bands {
        keys.push(&format!("band {}", band.id), &format!("band.{}", band.id));
        for column in &band.columns {
            keys.column(column);
        }
    }
    for chart in &ui.charts {
        keys.push(
            &format!("chart {}", chart.id),
            &format!("chart.{}", chart.id),
        );
    }
    keys.derived
}

struct Keys {
    prefix: String,
    seen: BTreeSet<String>,
    derived: Vec<DerivedKey>,
}

impl Keys {
    fn column(&mut self, column: &keel_types::Column) {
        self.push(
            &format!("column {}", column.key),
            &format!("column.{}", column.key),
        );
        let mut values: Vec<&str> = Vec::new();
        if let Some(ColumnFilter::Options { options }) = &column.filter {
            values.extend(options.iter().map(String::as_str));
        }
        if let Some(map) = &column.map {
            values.extend(map.keys().map(String::as_str));
        }
        for value in values {
            self.push(
                &format!("column {} value {value}", column.key),
                &format!("{}.{value}", column.key),
            );
        }
    }

    fn push(&mut self, member: &str, tail: &str) {
        self.push_under(None, member, tail);
    }

    fn push_under(&mut self, prefix: Option<&str>, member: &str, tail: &str) {
        let key = format!("{}.{tail}", prefix.unwrap_or(&self.prefix));
        if !self.seen.insert(key.clone()) {
            return;
        }
        self.derived.push(DerivedKey {
            member: member.to_owned(),
            key,
            shared: Some(format!("{SHARED_PREFIX}.{tail}")),
        });
    }

    fn explicit(&mut self, member: &str, key: &str) {
        if !self.seen.insert(key.to_owned()) {
            return;
        }
        self.derived.push(DerivedKey {
            member: member.to_owned(),
            key: key.to_owned(),
            shared: None,
        });
    }
}

pub fn entity_prefix(function: &str) -> String {
    function
        .rsplit_once('.')
        .map_or(function, |(prefix, _)| prefix)
        .to_owned()
}

fn patched_prefix(prefix: &str, module: &str) -> String {
    match prefix.split_once('.') {
        Some((_, entity)) => format!("{module}.{entity}"),
        None => module.to_owned(),
    }
}

fn properties(schema: &Schema) -> Option<&Map<String, Value>> {
    schema.get("properties").and_then(Value::as_object)
}

fn fields(descriptor: &Descriptor, properties: Option<&Map<String, Value>>) -> Vec<String> {
    let ui = descriptor.ui.as_ref();
    let paged: Vec<String> = ui
        .map(|ui| {
            ui.pages
                .iter()
                .flat_map(|page| page.fields.iter().cloned())
                .collect()
        })
        .unwrap_or_default();
    let mut fields = Vec::new();
    let from_pages = !paged.is_empty();
    let candidates = if from_pages {
        paged
    } else {
        properties
            .map(|properties| properties.keys().cloned().collect())
            .unwrap_or_default()
    };
    for field in candidates {
        if !from_pages && descriptor.key.as_deref() == Some(field.as_str()) {
            continue;
        }
        if !fields.contains(&field) {
            fields.push(field);
        }
    }
    fields
}

fn enum_values(schema: &Value, defs: Option<&Map<String, Value>>, depth: usize) -> Vec<String> {
    let mut values = Vec::new();
    if depth > MAX_DEPTH {
        return values;
    }
    let Some(object) = schema.as_object() else {
        return values;
    };
    if let Some(Value::Array(listed)) = object.get("enum") {
        values.extend(listed.iter().filter_map(Value::as_str).map(str::to_owned));
    }
    if let Some(reference) = object.get("$ref").and_then(Value::as_str)
        && let Some(name) = reference.strip_prefix("#/$defs/")
        && let Some(target) = defs.and_then(|defs| defs.get(name))
    {
        values.extend(enum_values(target, defs, depth + 1));
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(Value::Array(branches)) = object.get(key) {
            for branch in branches {
                values.extend(enum_values(branch, defs, depth + 1));
            }
        }
    }
    if let Some(items) = object.get("items") {
        values.extend(enum_values(items, defs, depth + 1));
    }
    let mut unique = Vec::with_capacity(values.len());
    for value in values {
        if !unique.contains(&value) {
            unique.push(value);
        }
    }
    unique
}

#[cfg(test)]
mod tests {
    use keel_sdk::prelude::*;
    use keel_sdk::ui::{Band, Chart, Report};
    use keel_types::ColumnType;

    use super::*;
    use crate::loader::testing::{Item, Nothing};

    #[derive(Deserialize, Serialize, JsonSchema)]
    #[serde(rename_all = "lowercase")]
    enum Flavour {
        Sweet,
        Sour,
    }

    #[derive(Deserialize, Serialize, JsonSchema)]
    struct Recipe {
        id: Option<Id>,
        name: String,
        flavour: Flavour,
        second: Option<Flavour>,
        tags: Vec<Flavour>,
        #[schemars(extend("x-ui" = json!({ "placeholder": "cook.recipe.note.placeholder", "help": "cook.recipe.note.hint" })))]
        note: Option<String>,
    }

    fn pairs(keys: &[DerivedKey]) -> Vec<(&str, Option<&str>)> {
        keys.iter()
            .map(|key| (key.key.as_str(), key.shared.as_deref()))
            .collect()
    }

    #[test]
    fn a_form_derives_field_enum_page_and_explicit_ui_keys() {
        let descriptor = function::<Recipe, Nothing>("cook.recipe.edit")
            .kind(Kind::Form)
            .unowned()
            .ui(Form::new("cook.recipe.edit.title")
                .page("main", ["name", "flavour", "second"])
                .page("more", ["tags", "note", "name"]))
            .handler(|_: Call<Recipe>| async { Ok(Nothing {}) })
            .descriptor()
            .unwrap();
        let keys = of(&descriptor, &BTreeMap::new());
        assert_eq!(
            pairs(&keys),
            [
                ("cook.recipe.name", Some("keel.name")),
                ("cook.recipe.flavour", Some("keel.flavour")),
                ("cook.recipe.flavour.sweet", Some("keel.flavour.sweet")),
                ("cook.recipe.flavour.sour", Some("keel.flavour.sour")),
                ("cook.recipe.second", Some("keel.second")),
                ("cook.recipe.second.sweet", Some("keel.second.sweet")),
                ("cook.recipe.second.sour", Some("keel.second.sour")),
                ("cook.recipe.tags", Some("keel.tags")),
                ("cook.recipe.tags.sweet", Some("keel.tags.sweet")),
                ("cook.recipe.tags.sour", Some("keel.tags.sour")),
                ("cook.recipe.note", Some("keel.note")),
                ("cook.recipe.note.placeholder", None),
                ("cook.recipe.note.hint", None),
                ("cook.recipe.page.main", Some("keel.page.main")),
                ("cook.recipe.page.more", Some("keel.page.more")),
            ]
        );
        assert_eq!(keys[0].member, "field name");
        assert_eq!(keys[2].member, "field flavour value sweet");
        assert_eq!(keys[11].member, "field note x-ui.placeholder");
        assert_eq!(keys[13].member, "page main");
    }

    #[test]
    fn a_form_without_pages_derives_every_property_but_the_key() {
        let descriptor = function::<Item, Nothing>("base.item.edit")
            .kind(Kind::Form)
            .key("id")
            .unowned()
            .ui(Form::new("base.item.edit.title"))
            .handler(|_: Call<Item>| async { Ok(Nothing {}) })
            .descriptor()
            .unwrap();
        let listed: Vec<String> = of(&descriptor, &BTreeMap::new())
            .into_iter()
            .map(|key| key.key)
            .collect();
        assert_eq!(
            listed,
            ["base.item.title", "base.item.kind", "base.item.weight"]
        );
    }

    #[test]
    fn a_list_derives_column_action_and_value_keys_and_a_screenless_function_none() {
        let descriptor = function::<ListQuery, Page<Item>>("base.item.list")
            .kind(Kind::List)
            .unowned()
            .ui(List::new("base.item.list.title")
                .column(Column::new("title", ColumnType::Text))
                .column(
                    Column::new("kind", ColumnType::Status)
                        .map([("heavy", Tone::Warning), ("light", Tone::Info)])
                        .filter_options(["light", "feather"]),
                )
                .action(Action::new("new", ActionRole::New).screen("base.item.edit"))
                .action(Action::new("drop", ActionRole::Delete).function("base.item.delete")))
            .handler(|_: Call<ListQuery>| async { Ok(Page::new(Vec::new(), 0)) })
            .descriptor()
            .unwrap();
        let listed: Vec<String> = of(&descriptor, &BTreeMap::new())
            .into_iter()
            .map(|key| key.key)
            .collect();
        assert_eq!(
            listed,
            [
                "base.item.action.new",
                "base.item.action.drop",
                "base.item.column.title",
                "base.item.column.kind",
                "base.item.kind.light",
                "base.item.kind.feather",
                "base.item.kind.heavy",
            ]
        );
        let bare = function::<Nothing, Nothing>("base.item.get")
            .kind(Kind::Get)
            .handler(|_: Call<Nothing>| async { Ok(Nothing {}) })
            .descriptor()
            .unwrap();
        assert!(of(&bare, &BTreeMap::new()).is_empty());
        assert_eq!(entity_prefix("base.item.get"), "base.item");
        assert_eq!(entity_prefix("bare"), "bare");
    }

    #[test]
    fn a_report_derives_band_chart_and_band_column_keys() {
        let descriptor = function::<Nothing, Nothing>("base.report.items")
            .kind(Kind::Report)
            .unowned()
            .ui(Report::new("base.report.items.title")
                .band(
                    Band::new("kinds")
                        .column(
                            Column::new("kind", ColumnType::Status).map([("heavy", Tone::Warning)]),
                        )
                        .column(Column::new("count", ColumnType::Number).total(Total::Sum)),
                )
                .chart(Chart::new(
                    "trend",
                    keel_types::ChartType::Line,
                    "base.report.series",
                )))
            .handler(|_: Call<Nothing>| async { Ok(Nothing {}) })
            .descriptor()
            .unwrap();
        let listed: Vec<String> = of(&descriptor, &BTreeMap::new())
            .into_iter()
            .map(|key| key.key)
            .collect();
        assert_eq!(
            listed,
            [
                "base.report.band.kinds",
                "base.report.column.kind",
                "base.report.kind.heavy",
                "base.report.column.count",
                "base.report.chart.trend",
            ]
        );
    }

    #[test]
    fn a_patched_field_derives_its_key_under_the_patching_module_with_the_shared_fallback() {
        let mut descriptor = function::<Recipe, Nothing>("cook.recipe.edit")
            .kind(Kind::Form)
            .unowned()
            .ui(Form::new("cook.recipe.edit.title").page("main", ["name", "flavour"]))
            .handler(|_: Call<Recipe>| async { Ok(Nothing {}) })
            .descriptor()
            .unwrap();
        descriptor.ui.as_mut().unwrap().pages[0]
            .fields
            .push("rating".to_owned());
        let patched = BTreeMap::from([("rating".to_owned(), "critic".to_owned())]);
        let keys = of(&descriptor, &patched);
        assert_eq!(
            pairs(&keys),
            [
                ("cook.recipe.name", Some("keel.name")),
                ("cook.recipe.flavour", Some("keel.flavour")),
                ("cook.recipe.flavour.sweet", Some("keel.flavour.sweet")),
                ("cook.recipe.flavour.sour", Some("keel.flavour.sour")),
                ("critic.recipe.rating", Some("keel.rating")),
                ("cook.recipe.page.main", Some("keel.page.main")),
            ]
        );
        assert_eq!(keys[4].member, "field rating");
        assert_eq!(patched_prefix("cook.recipe", "critic"), "critic.recipe");
        assert_eq!(patched_prefix("cook", "critic"), "critic");
    }
}
