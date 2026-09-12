use std::collections::{BTreeMap, BTreeSet};

use keel_types::expr::ParseError;
use keel_types::ui::{
    Action as WireAction, Column as WireColumn, ColumnFilter, ColumnType, Confirm, ConfirmDetails,
    Empty, ExportFormat, FieldUi, InlineEdit, Link, ScreenPage, ScreenResult, ScreenUi, Tone,
    Total,
};
use keel_types::{Expr, Level, MemberAccess};
use serde_json::{Map, Value};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiDef {
    screen: ScreenUi,
    conditions: Vec<ConditionSource>,
    mistakes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConditionSource {
    pub target: ConditionTarget,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConditionTarget {
    Page(String),
    Field(String, FieldCondition),
    Action(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldCondition {
    ShowIf,
    RequiredIf,
    ReadonlyIf,
}

#[derive(Debug, thiserror::Error)]
pub enum UiError {
    #[error("{target}: {source_text:?} does not parse: {error}")]
    Condition {
        target: String,
        source_text: String,
        #[source]
        error: ParseError,
    },
    #[error("{target}: {source_text:?} refers to an unknown field {field:?}")]
    UnknownField {
        target: String,
        source_text: String,
        field: String,
    },
    #[error(
        "{target}: {source_text:?} refers to an unknown variable {variable:?}; the system variables are $mode, $principal.level and $screen.params.<name>"
    )]
    UnknownVariable {
        target: String,
        source_text: String,
        variable: String,
    },
    #[error("{target}: {source_text:?} has no {target_kind} to attach to")]
    Detached {
        target: String,
        source_text: String,
        target_kind: &'static str,
    },
    #[error("{0}")]
    Misuse(String),
}

impl std::fmt::Display for ConditionTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConditionTarget::Page(id) => write!(f, "page {id}"),
            ConditionTarget::Field(name, slot) => write!(f, "field {name} {slot}"),
            ConditionTarget::Action(id) => write!(f, "action {id}"),
        }
    }
}

impl std::fmt::Display for FieldCondition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            FieldCondition::ShowIf => "show_if",
            FieldCondition::RequiredIf => "required_if",
            FieldCondition::ReadonlyIf => "readonly_if",
        })
    }
}

impl UiDef {
    pub fn screen(&self) -> &ScreenUi {
        &self.screen
    }

    pub fn conditions(&self) -> &[ConditionSource] {
        &self.conditions
    }

    pub fn field_names(&self) -> BTreeSet<String> {
        let mut names: BTreeSet<String> = self
            .screen
            .pages
            .iter()
            .flat_map(|page| page.fields.iter().cloned())
            .collect();
        names.extend(self.screen.fields.keys().cloned());
        names.extend(self.screen.multi.iter().cloned());
        names.extend(self.screen.search.iter().cloned());
        names
    }

    pub fn column_keys(&self) -> BTreeSet<String> {
        self.screen
            .columns
            .iter()
            .map(|column| column.key.clone())
            .collect()
    }

    pub fn compile(&self, known: &BTreeSet<String>) -> Result<ScreenUi, UiError> {
        if let Some(mistake) = self.mistakes.first() {
            return Err(UiError::Misuse(mistake.clone()));
        }
        let mut screen = self.screen.clone();
        for condition in &self.conditions {
            let target = condition.target.to_string();
            let expr = Expr::parse(&condition.source).map_err(|error| UiError::Condition {
                target: target.clone(),
                source_text: condition.source.clone(),
                error,
            })?;
            for variable in variables(&expr) {
                if variable.starts_with('$') {
                    if !is_system_variable(variable) {
                        return Err(UiError::UnknownVariable {
                            target,
                            source_text: condition.source.clone(),
                            variable: variable.to_owned(),
                        });
                    }
                } else if !known.contains(variable) {
                    return Err(UiError::UnknownField {
                        target,
                        source_text: condition.source.clone(),
                        field: variable.to_owned(),
                    });
                }
            }
            attach(&mut screen, &condition.target, expr).map_err(|target_kind| {
                UiError::Detached {
                    target: target.clone(),
                    source_text: condition.source.clone(),
                    target_kind,
                }
            })?;
        }
        Ok(screen)
    }

    fn condition(&mut self, target: ConditionTarget, source: &str) {
        self.conditions.push(ConditionSource {
            target,
            source: source.to_owned(),
        });
    }
}

fn attach(screen: &mut ScreenUi, target: &ConditionTarget, expr: Expr) -> Result<(), &'static str> {
    match target {
        ConditionTarget::Page(id) => {
            let page = screen
                .pages
                .iter_mut()
                .find(|page| &page.id == id)
                .ok_or("page")?;
            page.show_if = Some(expr);
        }
        ConditionTarget::Field(name, slot) => {
            let field = screen.fields.entry(name.clone()).or_default();
            match slot {
                FieldCondition::ShowIf => field.show_if = Some(expr),
                FieldCondition::RequiredIf => field.required_if = Some(expr),
                FieldCondition::ReadonlyIf => field.readonly_if = Some(expr),
            }
        }
        ConditionTarget::Action(id) => {
            let action = screen
                .actions
                .iter_mut()
                .find(|action| &action.id == id)
                .ok_or("action")?;
            action.hidden_if = Some(expr);
        }
    }
    Ok(())
}

pub fn variables(expr: &Expr) -> BTreeSet<&str> {
    let mut found = BTreeSet::new();
    collect(expr, &mut found);
    found
}

fn collect<'a>(expr: &'a Expr, into: &mut BTreeSet<&'a str>) {
    match expr {
        Expr::Eq(variable, _)
        | Expr::Ne(variable, _)
        | Expr::Gt(variable, _)
        | Expr::Lt(variable, _)
        | Expr::In(variable, _)
        | Expr::Matches(variable, _)
        | Expr::Truthy(variable)
        | Expr::Empty(variable) => {
            into.insert(root_segment(variable));
        }
        Expr::And(items) | Expr::Or(items) => items.iter().for_each(|item| collect(item, into)),
        Expr::Not(inner) => collect(inner, into),
    }
}

fn is_system_variable(variable: &str) -> bool {
    match variable {
        "$mode" | "$principal.level" => true,
        _ => variable
            .strip_prefix("$screen.params.")
            .is_some_and(|name| !name.is_empty()),
    }
}

fn root_segment(variable: &str) -> &str {
    if variable.starts_with('$') {
        variable
    } else {
        variable.split('.').next().unwrap_or(variable)
    }
}

#[derive(Clone, Debug)]
pub struct Form {
    def: UiDef,
    cursor: Cursor,
}

#[derive(Clone, Debug)]
enum Cursor {
    Nothing,
    Page(String),
    Field(String),
}

impl Form {
    pub fn new(title: &str) -> Self {
        Self {
            def: UiDef {
                screen: ScreenUi {
                    title: title.to_owned(),
                    ..Default::default()
                },
                ..Default::default()
            },
            cursor: Cursor::Nothing,
        }
    }

    pub fn read(mut self, func: &str) -> Self {
        self.def.screen.read = Some(func.to_owned());
        self
    }

    pub fn page<I, S>(mut self, id: &str, fields: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.def.screen.pages.push(ScreenPage {
            id: id.to_owned(),
            fields: fields.into_iter().map(Into::into).collect(),
            show_if: None,
            validate: None,
        });
        self.cursor = Cursor::Page(id.to_owned());
        self
    }

    pub fn validate(mut self, func: &str) -> Self {
        match &self.cursor {
            Cursor::Page(id) => {
                let id = id.clone();
                if let Some(page) = self.def.screen.pages.iter_mut().find(|page| page.id == id) {
                    page.validate = Some(func.to_owned());
                }
            }
            _ => self
                .def
                .mistakes
                .push(format!("validate({func:?}) follows no page")),
        }
        self
    }

    pub fn field(mut self, name: &str) -> Self {
        self.def.screen.fields.entry(name.to_owned()).or_default();
        self.cursor = Cursor::Field(name.to_owned());
        self
    }

    pub fn show_if(mut self, source: &str) -> Self {
        match &self.cursor {
            Cursor::Page(id) => {
                let target = ConditionTarget::Page(id.clone());
                self.def.condition(target, source);
            }
            Cursor::Field(name) => {
                let target = ConditionTarget::Field(name.clone(), FieldCondition::ShowIf);
                self.def.condition(target, source);
            }
            Cursor::Nothing => self.def.mistakes.push(format!(
                "show_if({source:?}) follows neither a page nor a field"
            )),
        }
        self
    }

    pub fn required_if(self, source: &str) -> Self {
        self.field_condition(FieldCondition::RequiredIf, source)
    }

    pub fn readonly_if(self, source: &str) -> Self {
        self.field_condition(FieldCondition::ReadonlyIf, source)
    }

    fn field_condition(mut self, slot: FieldCondition, source: &str) -> Self {
        match &self.cursor {
            Cursor::Field(name) => {
                let target = ConditionTarget::Field(name.clone(), slot);
                self.def.condition(target, source);
            }
            _ => self
                .def
                .mistakes
                .push(format!("{slot}({source:?}) follows no field")),
        }
        self
    }

    pub fn recompute(mut self, func: &str) -> Self {
        match &self.cursor {
            Cursor::Field(name) => {
                let name = name.clone();
                self.def.screen.fields.entry(name).or_default().recompute = Some(func.to_owned());
            }
            _ => self
                .def
                .mistakes
                .push(format!("recompute({func:?}) follows no field")),
        }
        self
    }

    pub fn multi<I, S>(mut self, fields: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.def.screen.multi = fields.into_iter().map(Into::into).collect();
        self
    }

    pub fn result(mut self, result: ScreenResult) -> Self {
        self.def.screen.result = Some(result);
        self
    }
}

impl From<Form> for UiDef {
    fn from(form: Form) -> Self {
        let mut def = form.def;
        def.screen
            .fields
            .retain(|_, field| *field != FieldUi::default());
        def
    }
}

#[derive(Clone, Debug)]
pub struct List {
    def: UiDef,
}

impl List {
    pub fn new(title: &str) -> Self {
        Self {
            def: UiDef {
                screen: ScreenUi {
                    title: title.to_owned(),
                    ..Default::default()
                },
                ..Default::default()
            },
        }
    }

    pub fn search<I, S>(mut self, fields: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.def.screen.search = fields.into_iter().map(Into::into).collect();
        self
    }

    pub fn column(mut self, column: Column) -> Self {
        self.def.screen.columns.push(column.inner);
        self
    }

    pub fn action(mut self, action: Action) -> Self {
        if let Some(source) = action.hidden_if {
            let target = ConditionTarget::Action(action.inner.id.clone());
            self.def.condition(target, &source);
        }
        self.def.screen.actions.push(action.inner);
        self
    }

    pub fn row_actions<I, S>(mut self, ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.def.screen.row_actions = ids.into_iter().map(Into::into).collect();
        self
    }

    pub fn empty(mut self, title: &str, action: Option<&str>) -> Self {
        self.def.screen.empty = Some(Empty {
            title: title.to_owned(),
            action: action.map(str::to_owned),
        });
        self
    }

    pub fn invalidate_on<I, S>(mut self, patterns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.def.screen.invalidate_on = patterns.into_iter().map(Into::into).collect();
        self
    }
}

impl From<List> for UiDef {
    fn from(list: List) -> Self {
        list.def
    }
}

#[derive(Clone, Debug)]
pub struct Column {
    inner: WireColumn,
}

impl Column {
    pub fn new(key: &str, kind: ColumnType) -> Self {
        Self {
            inner: WireColumn {
                key: key.to_owned(),
                kind,
                to: None,
                sortable: false,
                primary: false,
                total: None,
                map: None,
                filter: None,
                edit: None,
                access: None,
            },
        }
    }

    pub fn to<'a, I>(mut self, screen: &str, params: I) -> Self
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        self.inner.to = Some(Link {
            screen: screen.to_owned(),
            params: string_map(params),
        });
        self
    }

    pub fn sortable(mut self) -> Self {
        self.inner.sortable = true;
        self
    }

    pub fn primary(mut self) -> Self {
        self.inner.primary = true;
        self
    }

    pub fn total(mut self, total: Total) -> Self {
        self.inner.total = Some(total);
        self
    }

    pub fn map<'a, I>(mut self, tones: I) -> Self
    where
        I: IntoIterator<Item = (&'a str, Tone)>,
    {
        self.inner.map = Some(
            tones
                .into_iter()
                .map(|(value, tone)| (value.to_owned(), tone))
                .collect::<BTreeMap<_, _>>(),
        );
        self
    }

    pub fn filter_options<I, S>(mut self, options: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.inner.filter = Some(ColumnFilter::Options {
            options: options.into_iter().map(Into::into).collect(),
        });
        self
    }

    pub fn filter_remote(mut self, func: &str) -> Self {
        self.inner.filter = Some(ColumnFilter::Remote {
            remote: func.to_owned(),
        });
        self
    }

    pub fn edit(mut self, func: &str) -> Self {
        self.inner.edit = Some(InlineEdit {
            function: func.to_owned(),
        });
        self
    }

    pub fn level(mut self, level: Level) -> Self {
        self.inner.access.get_or_insert_with(Default::default).level = Some(level);
        self
    }

    pub fn permission(mut self, permission: &str) -> Self {
        self.inner
            .access
            .get_or_insert_with(MemberAccess::default)
            .permission = Some(permission.to_owned());
        self
    }
}

#[derive(Clone, Debug)]
pub struct Action {
    inner: WireAction,
    hidden_if: Option<String>,
}

impl Action {
    pub fn new(id: &str, role: keel_types::ActionRole) -> Self {
        Self {
            inner: WireAction {
                id: id.to_owned(),
                role,
                screen: None,
                function: None,
                icon: None,
                default: false,
                params: Map::new(),
                confirm: None,
                hidden_if: None,
                formats: Vec::new(),
            },
            hidden_if: None,
        }
    }

    pub fn screen(mut self, screen: &str) -> Self {
        self.inner.screen = Some(screen.to_owned());
        self
    }

    pub fn function(mut self, func: &str) -> Self {
        self.inner.function = Some(func.to_owned());
        self
    }

    pub fn icon(mut self, icon: &str) -> Self {
        self.inner.icon = Some(icon.to_owned());
        self
    }

    pub fn default(mut self) -> Self {
        self.inner.default = true;
        self
    }

    pub fn params<'a, I>(mut self, params: I) -> Self
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        self.inner.params = string_map(params);
        self
    }

    pub fn confirm(mut self) -> Self {
        self.inner.confirm = Some(Confirm::Simple(true));
        self
    }

    pub fn confirm_with(mut self, details: ConfirmDetails) -> Self {
        self.inner.confirm = Some(Confirm::Detailed(details));
        self
    }

    pub fn hidden_if(mut self, source: &str) -> Self {
        self.hidden_if = Some(source.to_owned());
        self
    }

    pub fn formats<I>(mut self, formats: I) -> Self
    where
        I: IntoIterator<Item = ExportFormat>,
    {
        self.inner.formats = formats.into_iter().collect();
        self
    }
}

fn string_map<'a, I>(pairs: I) -> Map<String, Value>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    pairs
        .into_iter()
        .map(|(key, value)| (key.to_owned(), Value::String(value.to_owned())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use keel_types::{ActionRole, ConfirmInput};
    use serde_json::json;

    fn known<const N: usize>(names: [&str; N]) -> BTreeSet<String> {
        names.into_iter().map(str::to_owned).collect()
    }

    fn zone_edit() -> UiDef {
        Form::new("dns.zone.edit.title")
            .read("dns.zone.get")
            .page("main", ["name", "kind", "ttl", "account"])
            .page("transfer", ["master_ips", "tsig_key"])
            .show_if("kind == 'slave'")
            .field("name")
            .readonly_if("$mode == 'edit'")
            .field("master_ips")
            .required_if("kind == 'slave'")
            .field("ttl")
            .recompute("dns.zone.recompute")
            .multi(["ttl"])
            .into()
    }

    #[test]
    fn form_conditions_are_kept_as_sources_until_compiled() {
        let def = zone_edit();
        assert_eq!(
            def.conditions(),
            &[
                ConditionSource {
                    target: ConditionTarget::Page("transfer".into()),
                    source: "kind == 'slave'".into()
                },
                ConditionSource {
                    target: ConditionTarget::Field("name".into(), FieldCondition::ReadonlyIf),
                    source: "$mode == 'edit'".into()
                },
                ConditionSource {
                    target: ConditionTarget::Field("master_ips".into(), FieldCondition::RequiredIf),
                    source: "kind == 'slave'".into()
                },
            ]
        );
        assert!(def.screen().pages[1].show_if.is_none());
        assert_eq!(
            def.field_names(),
            known(["name", "kind", "ttl", "account", "master_ips", "tsig_key"])
        );
    }

    #[test]
    fn a_compiled_form_carries_the_ast_of_every_condition() {
        let def = zone_edit();
        let screen = def
            .compile(&known([
                "id",
                "name",
                "kind",
                "ttl",
                "account",
                "master_ips",
                "tsig_key",
            ]))
            .unwrap();
        assert_eq!(
            serde_json::to_value(&screen).unwrap(),
            json!({
                "title": "dns.zone.edit.title",
                "read": "dns.zone.get",
                "pages": [
                    { "id": "main", "fields": ["name", "kind", "ttl", "account"] },
                    { "id": "transfer", "fields": ["master_ips", "tsig_key"], "show_if": { "eq": ["kind", "slave"] } }
                ],
                "fields": {
                    "master_ips": { "required_if": { "eq": ["kind", "slave"] } },
                    "name": { "readonly_if": { "eq": ["$mode", "edit"] } },
                    "ttl": { "recompute": "dns.zone.recompute" }
                },
                "multi": ["ttl"]
            })
        );
    }

    #[test]
    fn a_condition_naming_an_unknown_field_fails_to_compile() {
        let error = zone_edit()
            .compile(&known(["name", "ttl", "account", "master_ips", "tsig_key"]))
            .unwrap_err();
        match error {
            UiError::UnknownField { field, target, .. } => {
                assert_eq!(field, "kind");
                assert_eq!(target, "page transfer");
            }
            other => panic!("{other}"),
        }
    }

    #[test]
    fn a_condition_that_does_not_parse_fails_to_compile() {
        let def: UiDef = Form::new("t").page("p", ["a"]).show_if("a ==").into();
        assert!(matches!(
            def.compile(&known(["a"])),
            Err(UiError::Condition { .. })
        ));
    }

    #[test]
    fn a_condition_without_a_target_is_a_misuse() {
        let def: UiDef = Form::new("t").readonly_if("a").into();
        assert!(matches!(
            def.compile(&known(["a"])),
            Err(UiError::Misuse(_))
        ));
        let def: UiDef = Form::new("t").show_if("a").into();
        assert!(matches!(
            def.compile(&known(["a"])),
            Err(UiError::Misuse(_))
        ));
        let def: UiDef = Form::new("t").recompute("f").into();
        assert!(matches!(def.compile(&known([])), Err(UiError::Misuse(_))));
        let def: UiDef = Form::new("t").validate("f").into();
        assert!(matches!(def.compile(&known([])), Err(UiError::Misuse(_))));
    }

    #[test]
    fn nested_field_paths_are_checked_by_their_root() {
        let def: UiDef = Form::new("t").page("p", ["a"]).show_if("a.b").into();
        assert!(def.compile(&known(["a"])).is_ok());
        assert!(matches!(
            def.compile(&known([])),
            Err(UiError::UnknownField { field, .. }) if field == "a"
        ));
    }

    #[test]
    fn system_variables_come_from_a_closed_set() {
        let def: UiDef = Form::new("t")
            .page("p", ["a"])
            .show_if(
                "$mode == 'edit' && $principal.level in ['admin'] && $screen.params.zone_id == 'x'",
            )
            .into();
        assert!(def.compile(&known([])).is_ok());
        for source in [
            "$modee == 'edit'",
            "$principal.role == 'x'",
            "$principal",
            "$screen.params",
            "$screen.zone_id == 'x'",
            "$other",
        ] {
            let def: UiDef = Form::new("t").page("p", ["a"]).show_if(source).into();
            let error = def.compile(&known(["a"])).unwrap_err();
            match error {
                UiError::UnknownVariable {
                    variable, target, ..
                } => {
                    assert_eq!(variable, source.split(' ').next().unwrap());
                    assert_eq!(target, "page p");
                }
                other => panic!("{source}: {other}"),
            }
        }
    }

    #[test]
    fn a_wizard_page_carries_its_validate_function_and_result() {
        let def: UiDef = Form::new("t")
            .page("one", ["a"])
            .validate("m.w.check")
            .page("two", ["b"])
            .result(ScreenResult::Task)
            .into();
        let screen = def.compile(&known(["a", "b"])).unwrap();
        assert_eq!(screen.pages[0].validate.as_deref(), Some("m.w.check"));
        assert_eq!(screen.pages[1].validate, None);
        assert_eq!(screen.result, Some(ScreenResult::Task));
    }

    #[test]
    fn the_list_builder_reproduces_the_zone_list_fixture() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../contracts/fixtures/descriptor-dns-zone-list.json"
        ))
        .unwrap();
        let def: UiDef = List::new("dns.zone.list.title")
            .search(["name"])
            .column(
                Column::new("name", ColumnType::Link)
                    .to("dns.zone.edit", [("id", "$id")])
                    .sortable()
                    .primary(),
            )
            .column(
                Column::new("records", ColumnType::Number)
                    .sortable()
                    .total(Total::Sum),
            )
            .column(
                Column::new("status", ColumnType::Status)
                    .map([("active", Tone::Success), ("suspended", Tone::Warning)])
                    .filter_options(["active", "suspended"]),
            )
            .column(
                Column::new("dnssec", ColumnType::Toggle)
                    .edit("dns.zone.update")
                    .permission("dns.zone.manage"),
            )
            .column(
                Column::new("account", ColumnType::Text)
                    .level(Level::Reseller)
                    .filter_remote("keel.account.options"),
            )
            .action(
                Action::new("create", ActionRole::New)
                    .screen("dns.zone.edit")
                    .icon("plus"),
            )
            .action(
                Action::new("edit", ActionRole::Edit)
                    .screen("dns.zone.edit")
                    .default(),
            )
            .action(
                Action::new("records", ActionRole::Child)
                    .screen("dns.record.list")
                    .params([("zone_id", "$id")]),
            )
            .action(
                Action::new("suspend", ActionRole::Group)
                    .function("dns.zone.suspend")
                    .confirm()
                    .hidden_if("status == 'suspended'"),
            )
            .action(
                Action::new("delete", ActionRole::Delete)
                    .function("dns.zone.delete")
                    .confirm_with(ConfirmDetails {
                        destructive: true,
                        items: Some("name".into()),
                        input: Some(ConfirmInput::Count),
                    }),
            )
            .action(
                Action::new("export", ActionRole::Export)
                    .formats([ExportFormat::Csv, ExportFormat::Json]),
            )
            .row_actions(["edit", "records", "suspend", "delete"])
            .empty("dns.zone.empty", Some("create"))
            .invalidate_on(["dns.zone.*"])
            .into();
        let screen = def.compile(&def.column_keys()).unwrap();
        assert_eq!(serde_json::to_value(&screen).unwrap(), fixture["ui"]);
    }

    #[test]
    fn an_action_condition_must_name_a_column() {
        let def: UiDef = List::new("t")
            .column(Column::new("status", ColumnType::Status))
            .action(
                Action::new("suspend", ActionRole::Group)
                    .function("m.e.suspend")
                    .hidden_if("state == 'suspended'"),
            )
            .into();
        assert!(matches!(
            def.compile(&def.column_keys()),
            Err(UiError::UnknownField { .. })
        ));
    }
}
