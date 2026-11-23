use std::any::TypeId;
use std::collections::BTreeSet;
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use keel_types::descriptor::{Access, DESCRIPTOR_VERSION, Descriptor, Idempotency, Kind, Mode};
use keel_types::{DownloadFile, Level, ListQuery, LogLine, Page, Sort, SortDir};
use schemars::{JsonSchema, Schema, json_schema, schema_for};
use sea_orm::{IdenStatic, Iterable, PrimaryKeyToColumn};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::access::Owned;
use crate::call::Call;
use crate::download::DownloadOutcome;
use crate::error::{KeelError, Result};
use crate::list::{ListSource, ListSpec};
use crate::stream::Lines;
use crate::ui::{UiDef, UiError};

pub type ErasedHandler =
    Arc<dyn Fn(Call<Value>) -> BoxFuture<'static, Result<Value>> + Send + Sync>;

pub const LIST_QUERY_REF: &str = "keel://ListQuery";
pub const DEFAULT_CLASS: &str = "default";
pub const STORED_INLINE_KEY: &str = "keel.error.download_stored_inline";

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
    class: String,
    resumable: bool,
    internal: bool,
    exempt_from_read_only: bool,
    idempotency: Idempotency,
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
            class: DEFAULT_CLASS.to_owned(),
            resumable: false,
            internal: false,
            exempt_from_read_only: false,
            idempotency: Idempotency::Optional,
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

    pub fn class(mut self, class: &str) -> Self {
        self.def.class = class.to_owned();
        self
    }

    pub fn resumable(mut self) -> Self {
        self.def.resumable = true;
        self
    }

    pub fn internal(mut self) -> Self {
        self.def.internal = true;
        self
    }

    pub fn exempt_from_read_only(mut self) -> Self {
        self.def.exempt_from_read_only = true;
        self
    }

    pub fn idempotent(mut self, idempotency: Idempotency) -> Self {
        self.def.idempotency = idempotency;
        self
    }

    pub fn upload<F, Fut>(mut self, handler: F) -> FunctionDef
    where
        F: Fn(Call<P>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<R>> + Send + 'static,
    {
        self.def.kind = Kind::Upload;
        self.handler(handler)
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
            class: def.class,
            resumable: def.resumable,
            internal: def.internal,
            exempt_from_read_only: def.exempt_from_read_only,
            idempotency: def.idempotency,
            list,
            handler,
        }
    }
}

impl<P> FunctionBuilder<P, DownloadFile>
where
    P: DeserializeOwned + JsonSchema + Send + 'static,
{
    pub fn download<F, Fut>(mut self, handler: F) -> FunctionDef
    where
        F: Fn(Call<P>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<DownloadOutcome>> + Send + 'static,
    {
        self.def.kind = Kind::Download;
        let handler = Arc::new(handler);
        let erased: ErasedHandler = Arc::new(move |call: Call<Value>| {
            let handler = handler.clone();
            Box::pin(async move {
                let (state, raw) = call.into_parts();
                let params: P = serde_json::from_value(raw).map_err(|error| {
                    KeelError::validation("keel.error.validation").source(error)
                })?;
                let call = Call::from_parts(state, params);
                let slot = Call::from_parts(call.state_clone(), ());
                let outcome = handler(call).await?;
                let answer = match outcome {
                    DownloadOutcome::Body(download) => {
                        let info = download.info();
                        slot.attach_download(download);
                        serde_json::to_value(info)?
                    }
                    DownloadOutcome::Stored {
                        filename,
                        content_type,
                    } => {
                        let file = DownloadFile::new(filename, content_type);
                        slot.task()
                            .ok_or_else(|| KeelError::internal(STORED_INLINE_KEY))?
                            .stored(file.clone());
                        serde_json::to_value(file)?
                    }
                    DownloadOutcome::Task(reference) => serde_json::to_value(reference)?,
                };
                Ok(answer)
            })
        });
        self.finish(erased, None)
    }
}

impl<P> FunctionBuilder<P, LogLine>
where
    P: DeserializeOwned + JsonSchema + Send + 'static,
{
    pub(crate) fn lines<F, Fut>(mut self, handler: F) -> FunctionDef
    where
        F: Fn(Call<P>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Lines>> + Send + 'static,
    {
        self.def.kind = Kind::Stream;
        let handler = Arc::new(handler);
        let erased: ErasedHandler = Arc::new(move |call: Call<Value>| {
            let handler = handler.clone();
            Box::pin(async move {
                let (state, raw) = call.into_parts();
                let params: P = serde_json::from_value(raw).map_err(|error| {
                    KeelError::validation("keel.error.validation").source(error)
                })?;
                let call = Call::from_parts(state, params);
                let slot = Call::from_parts(call.state_clone(), ());
                slot.attach_lines(handler(call).await?);
                Ok(Value::Null)
            })
        });
        self.finish(erased, None)
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
    class: String,
    resumable: bool,
    internal: bool,
    exempt_from_read_only: bool,
    idempotency: Idempotency,
    list: Option<Arc<ListSpec>>,
    handler: ErasedHandler,
}

impl FunctionDef {
    pub(crate) fn described(descriptor: Descriptor, handler: ErasedHandler) -> Self {
        FunctionDef {
            name: descriptor.name,
            module: descriptor.module,
            kind: descriptor.kind,
            mode: descriptor.mode,
            access: descriptor.access,
            key: descriptor.key,
            ownership: Ownership::Unowned,
            params: embedded(descriptor.params),
            result: embedded(descriptor.result),
            ui: descriptor.ui.map(UiDef::compiled),
            limits: Limits::default(),
            class: DEFAULT_CLASS.to_owned(),
            resumable: false,
            internal: false,
            exempt_from_read_only: false,
            idempotency: descriptor.idempotency,
            list: None,
            handler,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn module(&self) -> &str {
        &self.module
    }

    pub(crate) fn set_module(&mut self, module: &str) {
        module.clone_into(&mut self.module);
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

    #[doc(hidden)]
    pub fn with_params(mut self, schema: Schema) -> Self {
        self.params = embedded(schema);
        self
    }

    #[doc(hidden)]
    pub fn with_result(mut self, schema: Schema) -> Self {
        self.result = embedded(schema);
        self
    }

    pub fn ui(&self) -> Option<&UiDef> {
        self.ui.as_ref()
    }

    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    pub fn class(&self) -> &str {
        &self.class
    }

    pub fn is_resumable(&self) -> bool {
        self.resumable
    }

    pub fn is_internal(&self) -> bool {
        self.internal
    }

    pub fn is_exempt_from_read_only(&self) -> bool {
        self.exempt_from_read_only
    }

    pub fn idempotency(&self) -> Idempotency {
        self.idempotency
    }

    pub fn handler(&self) -> &ErasedHandler {
        &self.handler
    }

    pub fn list_spec(&self) -> Option<&Arc<ListSpec>> {
        self.list.as_ref()
    }

    pub fn known_fields(&self) -> BTreeSet<String> {
        let mut known = property_names(&self.params);
        if self.kind == Kind::Get {
            known.extend(property_names(&self.result));
        }
        if let Some(ui) = &self.ui {
            known.extend(ui.column_keys());
        }
        known
    }

    fn row_fields(&self) -> BTreeSet<String> {
        if self.kind != Kind::List {
            return BTreeSet::new();
        }
        row_fields_of(&self.result)
    }

    pub fn descriptor(&self) -> Result<Descriptor, UiError> {
        if let Some(list) = &self.list {
            list.validate()?;
        }
        let ui = self
            .ui
            .as_ref()
            .map(|ui| ui.compile_in(&self.known_fields(), &self.row_fields()))
            .transpose()?;
        Ok(Descriptor {
            keel: DESCRIPTOR_VERSION,
            name: self.name.clone(),
            module: self.module.clone(),
            kind: self.kind,
            mode: self.mode,
            access: self.access.clone(),
            key: self.key.clone(),
            idempotency: self.idempotency,
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

fn row_fields_of(result: &Schema) -> BTreeSet<String> {
    let Some(items) = result
        .get("properties")
        .and_then(|properties| properties.get("data"))
        .and_then(|data| data.get("items"))
    else {
        return BTreeSet::new();
    };
    let row = match items.get("$ref").and_then(Value::as_str) {
        Some(reference) => reference
            .strip_prefix("#/$defs/")
            .and_then(|name| result.get("$defs")?.get(name)),
        None => Some(items),
    };
    row.and_then(|row| row.get("properties"))
        .and_then(Value::as_object)
        .map(|properties| properties.keys().cloned().collect())
        .unwrap_or_default()
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
            .field("class", &self.class)
            .field("resumable", &self.resumable)
            .field("internal", &self.internal)
            .field("exempt_from_read_only", &self.exempt_from_read_only)
            .field("idempotency", &self.idempotency)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{Action, Column, List};
    use keel_types::{ActionRole, ColumnType};
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

    #[derive(Serialize, JsonSchema)]
    struct Answer {
        allowed: bool,
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
    fn a_function_over_values_carries_the_schemas_it_is_given_without_their_dialect() {
        let def = function::<Value, Value>("m.e.open")
            .kind(Kind::Action)
            .handler(|call: Call<Value>| async move { Ok(call.params().clone()) })
            .with_params(schema_for!(Ping))
            .with_result(schema_for!(Pong));
        let descriptor = serde_json::to_value(def.descriptor().unwrap()).unwrap();
        assert_eq!(
            descriptor["params"]["properties"]["text"]["type"],
            json!("string")
        );
        assert_eq!(descriptor["params"]["required"], json!(["text"]));
        assert_eq!(
            descriptor["result"]["properties"]["text"]["type"],
            json!("string")
        );
        assert_eq!(descriptor["params"].get("$schema"), None);
        assert_eq!(descriptor["result"].get("$schema"), None);
        assert_eq!(def.known_fields(), BTreeSet::from(["text".to_owned()]));
    }

    #[test]
    fn a_get_knows_the_fields_of_its_result_so_its_actions_can_read_them() {
        let get = function::<Ping, Answer>("m.e.get")
            .kind(Kind::Get)
            .ui(crate::ui::ObjectScreen::new("m.e.get.title").action(
                crate::ui::Action::new("run", keel_types::ActionRole::Button)
                    .screen("m.e.run")
                    .hidden_if("!allowed"),
            ))
            .handler(|_: Call<Ping>| async { Ok(Answer { allowed: true }) });
        assert_eq!(
            get.known_fields(),
            BTreeSet::from(["allowed".to_owned(), "text".to_owned()])
        );
        let descriptor = get.descriptor().expect("the condition reads the result");
        assert_eq!(
            descriptor.ui.unwrap().actions[0].screen.as_deref(),
            Some("m.e.run")
        );
        let action = function::<Ping, Answer>("m.e.run")
            .kind(Kind::Action)
            .ui(crate::ui::List::new("t").action(
                crate::ui::Action::new("again", keel_types::ActionRole::Button)
                    .hidden_if("!allowed"),
            ))
            .handler(|_: Call<Ping>| async { Ok(Answer { allowed: true }) });
        assert_eq!(action.known_fields(), BTreeSet::from(["text".to_owned()]));
        assert!(matches!(
            action.descriptor(),
            Err(UiError::UnknownField { field, .. }) if field == "allowed"
        ));
    }

    #[test]
    fn a_function_names_the_class_of_its_jobs_and_whether_they_resume() {
        let plain = function::<Ping, Pong>("m.e.run")
            .kind(Kind::Action)
            .handler(|_: Call<Ping>| async { Ok(Pong { text: "x".into() }) });
        assert_eq!(plain.class(), DEFAULT_CLASS);
        assert!(!plain.is_resumable());
        assert!(!plain.is_internal());
        let heavy = function::<Ping, Pong>("m.e.export")
            .kind(Kind::Action)
            .class("export")
            .resumable()
            .handler(|_: Call<Ping>| async { Ok(Pong { text: "x".into() }) });
        assert_eq!(heavy.class(), "export");
        assert!(heavy.is_resumable());
        let descriptor = serde_json::to_value(heavy.descriptor().unwrap()).unwrap();
        assert!(descriptor.get("class").is_none());
        assert!(descriptor.get("resumable").is_none());
        assert!(format!("{heavy:?}").contains("class: \"export\", resumable: true"));
        let hidden = function::<Ping, Pong>("m.e.raise")
            .kind(Kind::Action)
            .internal()
            .handler(|_: Call<Ping>| async { Ok(Pong { text: "x".into() }) });
        assert!(hidden.is_internal());
        let descriptor = serde_json::to_value(hidden.descriptor().unwrap()).unwrap();
        assert!(descriptor.get("internal").is_none());
        assert!(format!("{hidden:?}").contains("internal: true"));
        assert!(!hidden.is_exempt_from_read_only());
        let session = function::<Ping, Pong>("m.e.enter")
            .kind(Kind::Form)
            .exempt_from_read_only()
            .handler(|_: Call<Ping>| async { Ok(Pong { text: "x".into() }) });
        assert!(session.is_exempt_from_read_only());
        let descriptor = serde_json::to_value(session.descriptor().unwrap()).unwrap();
        assert!(descriptor.get("exempt_from_read_only").is_none());
    }

    #[test]
    fn a_function_may_require_an_idempotency_key_which_its_descriptor_names() {
        let plain = function::<Ping, Pong>("m.e.open")
            .kind(Kind::Action)
            .handler(|_: Call<Ping>| async { Ok(Pong { text: "x".into() }) });
        assert_eq!(plain.idempotency(), Idempotency::Optional);
        let descriptor = serde_json::to_value(plain.descriptor().unwrap()).unwrap();
        assert!(descriptor.get("idempotency").is_none());
        let keyed = function::<Ping, Pong>("m.e.open")
            .kind(Kind::Action)
            .idempotent(Idempotency::Required)
            .handler(|_: Call<Ping>| async { Ok(Pong { text: "x".into() }) });
        assert_eq!(keyed.idempotency(), Idempotency::Required);
        assert_eq!(
            keyed.descriptor().unwrap().idempotency,
            Idempotency::Required
        );
        let descriptor = serde_json::to_value(keyed.descriptor().unwrap()).unwrap();
        assert_eq!(descriptor["idempotency"], json!("required"));
        assert!(format!("{keyed:?}").contains("idempotency: Required"));
        let relaxed = function::<Ping, Pong>("m.e.open")
            .kind(Kind::Action)
            .idempotent(Idempotency::Required)
            .idempotent(Idempotency::Optional)
            .handler(|_: Call<Ping>| async { Ok(Pong { text: "x".into() }) });
        assert_eq!(relaxed.idempotency(), Idempotency::Optional);
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

    #[test]
    fn a_list_action_may_take_a_row_field_that_is_no_column() {
        let list = |token: &str| {
            function::<ListQuery, Page<Pong>>("m.e.list")
                .kind(Kind::List)
                .ui(List::new("m.e.list.title")
                    .column(Column::new("id", ColumnType::Text))
                    .action(
                        Action::new("again", ActionRole::Button)
                            .function("m.e.again")
                            .params([("text", token)]),
                    ))
                .handler(|_: Call<ListQuery>| async { Ok(Page::new(Vec::new(), 0)) })
        };
        assert!(list("$text").descriptor().is_ok());
        assert!(matches!(
            list("$missing").descriptor(),
            Err(UiError::UnknownVariable { .. })
        ));
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
    async fn a_download_builder_answers_the_file_and_hands_the_body_to_the_call() {
        let def = function::<Ping, DownloadFile>("m.e.fetch")
            .level(Level::User)
            .unowned()
            .download(|call: Call<Ping>| async move {
                Ok(match call.params().text.as_str() {
                    "stored" => DownloadOutcome::stored("kept.csv", "text/csv"),
                    "task" => keel_types::TaskRef::new(
                        "01J9Q3W6B1R8N5X2K4T7M0V9Z3".parse().expect("an id"),
                    )
                    .into(),
                    text => {
                        crate::download::Download::bytes("out.txt", "text/plain", text.to_owned())
                            .into()
                    }
                })
            });
        assert_eq!(def.kind(), Kind::Download);
        assert_eq!(def.mode(), Mode::Read);
        let descriptor = serde_json::to_value(def.descriptor().unwrap()).unwrap();
        assert_eq!(descriptor["kind"], json!("download"));
        assert_eq!(descriptor["result"]["title"], json!("DownloadFile"));
        assert_eq!(
            descriptor["result"]["required"],
            json!(["filename", "contentType"])
        );

        let handle = crate::tests::read_handle();
        let answer = (def.handler())(handle.call(json!({ "text": "hello" })))
            .await
            .unwrap();
        assert_eq!(
            answer,
            json!({ "filename": "out.txt", "contentType": "text/plain" })
        );
        let download = handle.take_download().expect("the body is attached");
        assert_eq!(download.collect().await.unwrap(), b"hello");
        assert!(handle.take_download().is_none());

        let error = (def.handler())(handle.call(json!({ "text": "stored" })))
            .await
            .unwrap_err();
        assert_eq!(error.key(), STORED_INLINE_KEY);
        assert!(handle.take_download().is_none());

        let answer = (def.handler())(handle.call(json!({ "text": "task" })))
            .await
            .unwrap();
        assert_eq!(
            answer,
            json!({ "task": { "id": "01J9Q3W6B1R8N5X2K4T7M0V9Z3" } })
        );
        assert!(handle.take_download().is_none());
    }

    #[tokio::test]
    async fn an_upload_builder_sets_the_kind_and_keeps_the_write_mode() {
        let def = function::<Ping, Pong>("m.e.put")
            .level(Level::Admin)
            .unowned()
            .upload(|call: Call<Ping>| async move {
                Ok(Pong {
                    text: call.params().text.to_uppercase(),
                })
            });
        assert_eq!(def.kind(), Kind::Upload);
        assert_eq!(def.mode(), Mode::Write);
        let handle = crate::tests::read_handle();
        let answer = (def.handler())(handle.call(json!({ "text": "hello" })))
            .await
            .unwrap();
        assert_eq!(answer, json!({ "text": "HELLO" }));
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
