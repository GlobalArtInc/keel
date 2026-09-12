use std::any::TypeId;
use std::collections::BTreeSet;
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use keel_types::descriptor::{Access, DESCRIPTOR_VERSION, Descriptor, Kind, Mode};
use keel_types::{Level, ListQuery, Page, Sort, SortDir};
use schemars::{JsonSchema, Schema, json_schema, schema_for};
use sea_orm::{IdenStatic, Iterable, PrimaryKeyToColumn};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::access::Owned;
use crate::call::Call;
use crate::error::{KeelError, Result};
use crate::list::{ListSource, ListSpec};
use crate::ui::{UiDef, UiError};

pub type ErasedHandler =
    Arc<dyn Fn(Call<Value>) -> BoxFuture<'static, Result<Value>> + Send + Sync>;

pub const LIST_QUERY_REF: &str = "keel://ListQuery";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnedDef {
    pub table: String,
    pub account_column: String,
    pub primary_key: Vec<String>,
}

impl OwnedDef {
    pub fn of<E: Owned>() -> Self {
        Self {
            table: E::default().table_name().to_owned(),
            account_column: E::account_column().as_str().to_owned(),
            primary_key: E::PrimaryKey::iter()
                .map(|key| key.into_column().as_str().to_owned())
                .collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ownership {
    Undeclared,
    Owned(OwnedDef),
    Unowned,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Limits {
    pub timeout: Option<Duration>,
    pub max_page_size: Option<u32>,
    pub default_sort: Option<Sort>,
    pub batch_threshold: Option<u32>,
}

struct Definition {
    name: String,
    kind: Kind,
    mode: Option<Mode>,
    access: Access,
    key: Option<String>,
    ownership: Ownership,
    ui: Option<UiDef>,
    limits: Limits,
}

pub struct FunctionBuilder<P, R> {
    def: Definition,
    types: PhantomData<fn(P) -> R>,
}

pub fn function<P, R>(name: &str) -> FunctionBuilder<P, R> {
    FunctionBuilder {
        def: Definition {
            name: name.to_owned(),
            kind: Kind::Action,
            mode: None,
            access: Access {
                level: Level::Owner,
                permission: None,
                feature: None,
            },
            key: None,
            ownership: Ownership::Undeclared,
            ui: None,
            limits: Limits::default(),
        },
        types: PhantomData,
    }
}

pub fn natural_mode(kind: Kind) -> Mode {
    match kind {
        Kind::List | Kind::Get | Kind::Report | Kind::Options | Kind::Stream | Kind::Download => {
            Mode::Read
        }
        Kind::Form | Kind::Action | Kind::Wizard | Kind::Upload => Mode::Write,
    }
}

impl<P, R> FunctionBuilder<P, R>
where
    P: DeserializeOwned + JsonSchema + Send + 'static,
    R: Serialize + JsonSchema + Send + 'static,
{
    pub fn kind(mut self, kind: Kind) -> Self {
        self.def.kind = kind;
        self
    }

    pub fn mode(mut self, mode: Mode) -> Self {
        self.def.mode = Some(mode);
        self
    }

    pub fn access(mut self, level: Level, permission: &str) -> Self {
        self.def.access.level = level;
        self.def.access.permission = Some(permission.to_owned());
        self
    }

    pub fn level(mut self, level: Level) -> Self {
        self.def.access.level = level;
        self.def.access.permission = None;
        self
    }

    pub fn feature(mut self, feature: &str) -> Self {
        self.def.access.feature = Some(feature.to_owned());
        self
    }

    pub fn key(mut self, key: &str) -> Self {
        self.def.key = Some(key.to_owned());
        self
    }

    pub fn owned<E: Owned>(mut self) -> Self {
        self.def.ownership = Ownership::Owned(OwnedDef::of::<E>());
        if self.def.key.is_none() {
            self.def.key = Some("id".to_owned());
        }
        self
    }

    pub fn unowned(mut self) -> Self {
        self.def.ownership = Ownership::Unowned;
        self
    }

    pub fn ui(mut self, ui: impl Into<UiDef>) -> Self {
        self.def.ui = Some(ui.into());
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.def.limits.timeout = Some(timeout);
        self
    }

    pub fn max_page_size(mut self, size: u32) -> Self {
        self.def.limits.max_page_size = Some(size);
        self
    }

    pub fn default_sort(mut self, key: &str, dir: SortDir) -> Self {
        self.def.limits.default_sort = Some(Sort {
            key: key.to_owned(),
            dir,
        });
        self
    }

    pub fn batch_threshold(mut self, threshold: u32) -> Self {
        self.def.limits.batch_threshold = Some(threshold);
        self
    }

    pub fn handler<F, Fut>(self, handler: F) -> FunctionDef
    where
        F: Fn(Call<P>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<R>> + Send + 'static,
    {
        let handler = Arc::new(handler);
        let erased: ErasedHandler = Arc::new(move |call: Call<Value>| {
            let handler = handler.clone();
            Box::pin(async move {
                let (state, raw) = call.into_parts();
                let params: P = serde_json::from_value(raw).map_err(|error| {
                    KeelError::validation("keel.error.validation").source(error)
                })?;
                let result = handler(Call::from_parts(state, params)).await?;
                Ok(serde_json::to_value(result)?)
            })
        });
        self.finish(erased, None)
    }

    fn finish(self, handler: ErasedHandler, list: Option<Arc<ListSpec>>) -> FunctionDef {
        let def = self.def;
        let params = if TypeId::of::<P>() == TypeId::of::<ListQuery>() {
            json_schema!({ "$ref": LIST_QUERY_REF })
        } else {
            embedded(schema_for!(P))
        };
        FunctionDef {
            module: def.name.split('.').next().unwrap_or_default().to_owned(),
            mode: def.mode.unwrap_or_else(|| natural_mode(def.kind)),
            name: def.name,
            kind: def.kind,
            access: def.access,
            key: def.key,
            ownership: def.ownership,
            params,
            result: embedded(schema_for!(R)),
            ui: def.ui,
            limits: def.limits,
            list,
            handler,
        }
    }
}

impl<Row> FunctionBuilder<ListQuery, Page<Row>>
where
    Row: Serialize + JsonSchema + Send + 'static,
{
    pub fn source<S>(self, source: S) -> FunctionDef
    where
        S: ListSource<Row = Row> + 'static,
    {
        let source = Arc::new(source);
        let spec = Arc::new(ListSpec::new(self.def.ui.as_ref(), &self.def.limits));
        let enforced = spec.clone();
        let erased: ErasedHandler = Arc::new(move |call: Call<Value>| {
            let source = source.clone();
            let spec = enforced.clone();
            Box::pin(async move {
                let (state, raw) = call.into_parts();
                let query: ListQuery = serde_json::from_value(raw).map_err(|error| {
                    KeelError::validation("keel.error.validation").source(error)
                })?;
                let query = spec.check(query)?;
                let page = source.fetch(&Call::from_parts(state, query), &spec).await?;
                Ok(serde_json::to_value(page)?)
            })
        });
        self.finish(erased, Some(spec))
    }
}

pub struct FunctionDef {
    name: String,
    module: String,
    kind: Kind,
    mode: Mode,
    access: Access,
    key: Option<String>,
    ownership: Ownership,
    params: Schema,
    result: Schema,
    ui: Option<UiDef>,
    limits: Limits,
    list: Option<Arc<ListSpec>>,
    handler: ErasedHandler,
}

impl FunctionDef {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn module(&self) -> &str {
        &self.module
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn access(&self) -> &Access {
        &self.access
    }

    pub fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }

    pub fn ownership(&self) -> &Ownership {
        &self.ownership
    }

    pub fn owned(&self) -> Option<&OwnedDef> {
        match &self.ownership {
            Ownership::Owned(owned) => Some(owned),
            _ => None,
        }
    }

    pub fn params(&self) -> &Schema {
        &self.params
    }

    pub fn result(&self) -> &Schema {
        &self.result
    }

    pub fn ui(&self) -> Option<&UiDef> {
        self.ui.as_ref()
    }

    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    pub fn handler(&self) -> &ErasedHandler {
        &self.handler
    }

    pub fn known_fields(&self) -> BTreeSet<String> {
        let mut known = property_names(&self.params);
        if let Some(ui) = &self.ui {
            known.extend(ui.column_keys());
        }
        known
    }

    pub fn descriptor(&self) -> Result<Descriptor, UiError> {
        if let Some(list) = &self.list {
            list.validate()?;
        }
        let ui = self
            .ui
            .as_ref()
            .map(|ui| ui.compile(&self.known_fields()))
            .transpose()?;
        Ok(Descriptor {
            keel: DESCRIPTOR_VERSION,
            name: self.name.clone(),
            module: self.module.clone(),
            kind: self.kind,
            mode: self.mode,
            access: self.access.clone(),
            key: self.key.clone(),
            params: self.params.clone(),
            result: self.result.clone(),
            ui,
        })
    }
}

fn embedded(mut schema: Schema) -> Schema {
    schema.remove("$schema");
    schema
}

pub fn property_names(schema: &Schema) -> BTreeSet<String> {
    schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| properties.keys().cloned().collect())
        .unwrap_or_default()
}

impl fmt::Debug for FunctionDef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FunctionDef")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("mode", &self.mode)
            .field("access", &self.access)
            .field("key", &self.key)
            .field("ownership", &self.ownership)
            .field("limits", &self.limits)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::json;

    #[derive(Deserialize, JsonSchema)]
    struct Ping {
        text: String,
    }

    #[derive(Serialize, JsonSchema)]
    struct Pong {
        text: String,
    }

    #[test]
    fn the_mode_defaults_to_the_natural_mode_of_the_kind() {
        let def = function::<Ping, Pong>("m.e.get").kind(Kind::Get).handler(
            |call: Call<Ping>| async move {
                Ok(Pong {
                    text: call.params().text.clone(),
                })
            },
        );
        assert_eq!(def.mode(), Mode::Read);
        assert_eq!(def.module(), "m");
        assert_eq!(def.kind(), Kind::Get);
        let def = function::<Ping, Pong>("m.e.edit")
            .kind(Kind::Form)
            .handler(|_: Call<Ping>| async { Ok(Pong { text: "x".into() }) });
        assert_eq!(def.mode(), Mode::Write);
        let def = function::<Ping, Pong>("m.e.probe")
            .kind(Kind::Form)
            .mode(Mode::Read)
            .handler(|_: Call<Ping>| async { Ok(Pong { text: "x".into() }) });
        assert_eq!(def.mode(), Mode::Read);
        for kind in [
            Kind::List,
            Kind::Get,
            Kind::Report,
            Kind::Options,
            Kind::Stream,
            Kind::Download,
        ] {
            assert_eq!(natural_mode(kind), Mode::Read);
        }
        for kind in [Kind::Form, Kind::Action, Kind::Wizard, Kind::Upload] {
            assert_eq!(natural_mode(kind), Mode::Write);
        }
    }

    #[test]
    fn the_descriptor_carries_schemas_access_and_limits_as_declared() {
        let def = function::<Ping, Pong>("m.e.edit")
            .kind(Kind::Form)
            .access(Level::User, "m.e.manage")
            .feature("m.things")
            .key("id")
            .unowned()
            .timeout(Duration::from_secs(5))
            .max_page_size(100)
            .default_sort("name", SortDir::Desc)
            .batch_threshold(20)
            .handler(|_: Call<Ping>| async { Ok(Pong { text: "x".into() }) });
        assert_eq!(*def.ownership(), Ownership::Unowned);
        assert_eq!(
            *def.limits(),
            Limits {
                timeout: Some(Duration::from_secs(5)),
                max_page_size: Some(100),
                default_sort: Some(Sort {
                    key: "name".into(),
                    dir: SortDir::Desc
                }),
                batch_threshold: Some(20),
            }
        );
        let descriptor = serde_json::to_value(def.descriptor().unwrap()).unwrap();
        assert_eq!(descriptor["keel"], json!(1));
        assert_eq!(descriptor["name"], json!("m.e.edit"));
        assert_eq!(descriptor["module"], json!("m"));
        assert_eq!(descriptor["kind"], json!("form"));
        assert_eq!(descriptor["mode"], json!("write"));
        assert_eq!(
            descriptor["access"],
            json!({ "level": "user", "permission": "m.e.manage", "feature": "m.things" })
        );
        assert_eq!(descriptor["key"], json!("id"));
        assert_eq!(
            descriptor["params"]["properties"]["text"]["type"],
            json!("string")
        );
        assert_eq!(descriptor["params"].get("$schema"), None);
        assert_eq!(descriptor["result"].get("$schema"), None);
        assert_eq!(
            descriptor["result"]["properties"]["text"]["type"],
            json!("string")
        );
        assert_eq!(descriptor.get("ui"), None);
        assert_eq!(def.known_fields(), BTreeSet::from(["text".to_owned()]));
    }

    #[test]
    fn a_guest_function_has_a_level_and_no_permission() {
        let def = function::<Ping, Pong>("m.login")
            .level(Level::Guest)
            .handler(|_: Call<Ping>| async { Ok(Pong { text: "x".into() }) });
        assert_eq!(def.access().level, Level::Guest);
        assert_eq!(def.access().permission, None);
    }

    #[test]
    fn a_list_points_its_params_at_the_shared_list_query() {
        let def = function::<ListQuery, Page<Pong>>("m.e.list")
            .kind(Kind::List)
            .handler(|_: Call<ListQuery>| async { Ok(Page::new(Vec::new(), 0)) });
        assert_eq!(
            serde_json::to_value(def.params()).unwrap(),
            json!({ "$ref": "keel://ListQuery" })
        );
        assert_eq!(def.result().get("title"), Some(&json!("Page_Pong")));
        assert_eq!(
            def.result().as_value()["properties"]["data"]["items"],
            json!({ "$ref": "#/$defs/Pong" })
        );
    }

    struct Nothing;

    #[async_trait::async_trait]
    impl ListSource for Nothing {
        type Row = Pong;

        async fn fetch(&self, _: &Call<ListQuery>, _: &ListSpec) -> Result<Page<Pong>> {
            Ok(Page::new(Vec::new(), 0))
        }
    }

    #[test]
    fn a_default_sort_on_a_column_not_marked_sortable_fails_the_descriptor() {
        let list = || {
            function::<ListQuery, Page<Pong>>("m.e.list")
                .kind(Kind::List)
                .ui(crate::ui::List::new("m.e.list.title")
                    .column(crate::ui::Column::new("text", keel_types::ColumnType::Text)))
                .default_sort("text", SortDir::Asc)
        };
        let error = list()
            .source(Nothing)
            .descriptor()
            .expect_err("text is not sortable");
        assert!(error.to_string().contains("text"), "{error}");
        let sortable = function::<ListQuery, Page<Pong>>("m.e.list")
            .kind(Kind::List)
            .ui(crate::ui::List::new("m.e.list.title")
                .column(crate::ui::Column::new("text", keel_types::ColumnType::Text).sortable()))
            .default_sort("text", SortDir::Asc)
            .source(Nothing);
        sortable
            .descriptor()
            .expect("a sortable default sort compiles");
        let bare = function::<ListQuery, Page<Pong>>("m.e.list")
            .kind(Kind::List)
            .default_sort("text", SortDir::Asc)
            .source(Nothing);
        bare.descriptor()
            .expect_err("a list without a screen has no sortable column");
        list()
            .handler(|_: Call<ListQuery>| async { Ok(Page::new(Vec::new(), 0)) })
            .descriptor()
            .expect("a handler list enforces no spec");
    }

    #[tokio::test]
    async fn the_erased_handler_rejects_params_that_do_not_deserialise() {
        let def = function::<Ping, Pong>("m.e.edit")
            .kind(Kind::Form)
            .handler(|_: Call<Ping>| async { Ok(Pong { text: "x".into() }) });
        let handle = crate::tests::read_handle();
        let error = (def.handler())(handle.call(json!({ "text": 1 })))
            .await
            .unwrap_err();
        assert_eq!(error.code(), keel_types::ErrorCode::Validation);
    }
}
