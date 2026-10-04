use std::any::{Any, TypeId};
use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::future::Future;
use std::sync::Arc;

use futures::future::BoxFuture;
use keel_types::{Access, Level, LogLine};
use schemars::{JsonSchema, Schema};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::call::{Call, Invoker};
use crate::context::Context;
use crate::error::Result;
use crate::function::{FunctionBuilder, FunctionDef};
use crate::guard::Guard;
use crate::host::{AgentOp, ConfigTemplate};
use crate::lifecycle::AccountLifecycle;
use crate::limit::LimitDef;
use crate::services::{ServiceKey, Services};
use crate::stream::Lines;
use crate::system::Endpoint;
use crate::ui::ViewSpec;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuNode {
    path: String,
    func: String,
    icon: Option<String>,
    order: i32,
    label: Option<String>,
    badge: Option<String>,
}

impl MenuNode {
    pub fn new(path: &str, func: &str) -> Self {
        Self {
            path: path.to_owned(),
            func: func.to_owned(),
            icon: None,
            order: 0,
            label: None,
            badge: None,
        }
    }

    pub fn icon(mut self, icon: &str) -> Self {
        self.icon = Some(icon.to_owned());
        self
    }

    pub fn order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    pub fn label(mut self, key: &str) -> Self {
        self.label = Some(key.to_owned());
        self
    }

    pub fn badge(mut self, problem: &str) -> Self {
        self.badge = Some(problem.to_owned());
        self
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn func(&self) -> &str {
        &self.func
    }

    pub fn icon_name(&self) -> Option<&str> {
        self.icon.as_deref()
    }

    pub fn order_value(&self) -> i32 {
        self.order
    }

    pub fn label_key(&self) -> Option<&str> {
        self.label.as_deref()
    }

    pub fn badge_problem(&self) -> Option<&str> {
        self.badge.as_deref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionDef {
    pub key: String,
    pub defaults: Vec<Level>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewDef {
    pub name: String,
    pub access: Access,
    pub needs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct I18nBundle {
    pub lang: String,
    pub messages: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Before {
    Proceed,
    Enrich(Value),
}

pub type BeforeHook = Arc<dyn Fn(Call<Value>) -> BoxFuture<'static, Result<Before>> + Send + Sync>;
pub type WithinHook = Arc<dyn Fn(Call<Value>) -> BoxFuture<'static, Result<()>> + Send + Sync>;
pub type StoreHook =
    Arc<dyn Fn(Call<Value>, Stored) -> BoxFuture<'static, Result<()>> + Send + Sync>;
pub type StartHook = Box<dyn FnOnce(Context) -> BoxFuture<'static, Result<()>> + Send>;
pub type StopHook = Box<dyn FnOnce() -> BoxFuture<'static, Result<()>> + Send>;

#[derive(Clone)]
pub struct HookDef<H> {
    pub function: String,
    pub hook: H,
}

impl<H> fmt::Debug for HookDef<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HookDef")
            .field("function", &self.function)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stored {
    pub field: String,
    pub value: Value,
    pub row: Value,
}

#[derive(Clone)]
pub struct DescriptorPatch {
    target: String,
    fields: Vec<AddedField>,
    access: Option<Access>,
}

#[derive(Clone)]
pub struct AddedField {
    pub name: String,
    pub schema: Schema,
    pub after: Option<String>,
    pub store: Option<StoreHook>,
}

impl fmt::Debug for AddedField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AddedField")
            .field("name", &self.name)
            .field("after", &self.after)
            .field("store", &self.store.is_some())
            .finish()
    }
}

impl fmt::Debug for DescriptorPatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DescriptorPatch")
            .field("target", &self.target)
            .field("fields", &self.fields)
            .field("access", &self.access)
            .finish()
    }
}

impl DescriptorPatch {
    pub fn new(target: &str) -> Self {
        Self {
            target: target.to_owned(),
            fields: Vec::new(),
            access: None,
        }
    }

    pub fn field(mut self, name: &str, schema: Schema) -> Self {
        self.fields.push(AddedField {
            name: name.to_owned(),
            schema,
            after: None,
            store: None,
        });
        self
    }

    pub fn after(mut self, field: &str) -> Self {
        if let Some(last) = self.fields.last_mut() {
            last.after = Some(field.to_owned());
        }
        self
    }

    pub fn store<F, Fut>(mut self, store: F) -> Self
    where
        F: Fn(Call<Value>, Stored) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        if let Some(last) = self.fields.last_mut() {
            last.store = Some(Arc::new(move |call, stored| Box::pin(store(call, stored))));
        }
        self
    }

    pub fn access(mut self, level: Level, permission: Option<&str>) -> Self {
        self.access = Some(Access {
            level,
            permission: permission.map(str::to_owned),
            feature: None,
        });
        self
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn fields(&self) -> &[AddedField] {
        &self.fields
    }

    pub fn access_change(&self) -> Option<&Access> {
        self.access.as_ref()
    }
}

#[derive(Default)]
pub struct Contributions {
    entries: HashMap<TypeId, Vec<Box<dyn Any + Send + Sync>>>,
}

impl Contributions {
    pub fn add<T: Any + Send + Sync>(&mut self, contribution: T) {
        self.entries
            .entry(TypeId::of::<T>())
            .or_default()
            .push(Box::new(contribution));
    }

    pub fn of<T: Any + Send + Sync>(&self) -> impl Iterator<Item = &T> {
        self.entries
            .get(&TypeId::of::<T>())
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.downcast_ref::<T>())
    }

    pub fn take<T: Any + Send + Sync>(&mut self) -> Vec<T> {
        self.entries
            .remove(&TypeId::of::<T>())
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.downcast::<T>().ok().map(|boxed| *boxed))
            .collect()
    }

    pub fn extend(&mut self, other: Contributions) {
        for (id, entries) in other.entries {
            self.entries.entry(id).or_default().extend(entries);
        }
    }

    pub fn len(&self) -> usize {
        self.entries.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl fmt::Debug for Contributions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Contributions({})", self.len())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RegistrationError {
    #[error("module {module}: {kind} {name:?} does not carry the module prefix")]
    Prefix {
        module: String,
        kind: &'static str,
        name: String,
    },
    #[error("module {module}: {kind} {name:?} is registered twice")]
    Duplicate {
        module: String,
        kind: &'static str,
        name: String,
    },
    #[error("module {module}: the {lang} dictionary is not a JSON object: {reason}")]
    Dictionary {
        module: String,
        lang: String,
        reason: String,
    },
    #[error("module {module}: the menu node {path} of the view {view} points at {func}")]
    ViewMenu {
        module: String,
        view: String,
        path: String,
        func: String,
    },
}

pub struct Registrar {
    module: String,
    functions: Vec<FunctionDef>,
    views: Vec<ViewDef>,
    menu: Vec<MenuNode>,
    i18n: Vec<I18nBundle>,
    permissions: Vec<PermissionDef>,
    before: Vec<HookDef<BeforeHook>>,
    within: Vec<HookDef<WithinHook>>,
    guards: Vec<Arc<dyn Guard>>,
    patches: Vec<DescriptorPatch>,
    contributions: Contributions,
    provides: Services,
    requires: Vec<ServiceKey>,
    start: Option<StartHook>,
    stop: Option<StopHook>,
    invoker: Option<Arc<dyn Invoker>>,
    errors: Vec<RegistrationError>,
}

pub struct Registrations {
    pub module: String,
    pub functions: Vec<FunctionDef>,
    pub views: Vec<ViewDef>,
    pub menu: Vec<MenuNode>,
    pub i18n: Vec<I18nBundle>,
    pub permissions: Vec<PermissionDef>,
    pub before: Vec<HookDef<BeforeHook>>,
    pub within: Vec<HookDef<WithinHook>>,
    pub guards: Vec<Arc<dyn Guard>>,
    pub patches: Vec<DescriptorPatch>,
    pub contributions: Contributions,
    pub provides: Services,
    pub requires: Vec<ServiceKey>,
    pub start: Option<StartHook>,
    pub stop: Option<StopHook>,
    pub invoker: Option<Arc<dyn Invoker>>,
}

impl fmt::Debug for Registrations {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Registrations")
            .field("module", &self.module)
            .field("functions", &self.functions)
            .field("views", &self.views)
            .field("menu", &self.menu)
            .field("permissions", &self.permissions)
            .field("before", &self.before)
            .field("within", &self.within)
            .field("guards", &self.guards.len())
            .field("patches", &self.patches)
            .field("contributions", &self.contributions)
            .field("provides", &self.provides)
            .field("requires", &self.requires)
            .field("start", &self.start.is_some())
            .field("stop", &self.stop.is_some())
            .field("invoker", &self.invoker.is_some())
            .finish()
    }
}

impl Registrar {
    pub fn new(module: &str) -> Self {
        Self {
            module: module.to_owned(),
            functions: Vec::new(),
            views: Vec::new(),
            menu: Vec::new(),
            i18n: Vec::new(),
            permissions: Vec::new(),
            before: Vec::new(),
            within: Vec::new(),
            guards: Vec::new(),
            patches: Vec::new(),
            contributions: Contributions::default(),
            provides: Services::new(),
            requires: Vec::new(),
            start: None,
            stop: None,
            invoker: None,
            errors: Vec::new(),
        }
    }

    pub fn module(&self) -> &str {
        &self.module
    }

    pub fn function(&mut self, mut def: FunctionDef) -> &mut Self {
        self.check_prefix("function", def.name());
        def.set_module(&self.module);
        if self
            .functions
            .iter()
            .any(|known| known.name() == def.name())
            || self.views.iter().any(|known| known.name == def.name())
        {
            self.errors.push(RegistrationError::Duplicate {
                module: self.module.clone(),
                kind: "function",
                name: def.name().to_owned(),
            });
        }
        self.functions.push(def);
        self
    }

    pub fn stream<P, F, Fut>(
        &mut self,
        builder: FunctionBuilder<P, LogLine>,
        handler: F,
    ) -> &mut Self
    where
        P: DeserializeOwned + JsonSchema + Send + 'static,
        F: Fn(Call<P>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Lines>> + Send + 'static,
    {
        self.function(builder.lines(handler))
    }

    pub fn view(&mut self, name: &str, spec: ViewSpec) -> &mut Self {
        self.check_prefix("view", name);
        if self.views.iter().any(|known| known.name == name)
            || self.functions.iter().any(|known| known.name() == name)
        {
            self.errors.push(RegistrationError::Duplicate {
                module: self.module.clone(),
                kind: "view",
                name: name.to_owned(),
            });
        }
        if let Some(node) = spec.menu {
            if node.func() == name {
                self.menu.push(node);
            } else {
                self.errors.push(RegistrationError::ViewMenu {
                    module: self.module.clone(),
                    view: name.to_owned(),
                    path: node.path().to_owned(),
                    func: node.func().to_owned(),
                });
            }
        }
        self.views.push(ViewDef {
            name: name.to_owned(),
            access: spec.access,
            needs: spec.needs,
        });
        self
    }

    pub fn menu(&mut self, node: MenuNode) -> &mut Self {
        self.menu.push(node);
        self
    }

    pub fn i18n(&mut self, lang: &str, json: &str) -> &mut Self {
        match serde_json::from_str::<Value>(json) {
            Ok(messages) if messages.is_object() => self.i18n.push(I18nBundle {
                lang: lang.to_owned(),
                messages,
            }),
            Ok(_) => self.errors.push(RegistrationError::Dictionary {
                module: self.module.clone(),
                lang: lang.to_owned(),
                reason: "the document is not an object".to_owned(),
            }),
            Err(error) => self.errors.push(RegistrationError::Dictionary {
                module: self.module.clone(),
                lang: lang.to_owned(),
                reason: error.to_string(),
            }),
        }
        self
    }

    pub fn permission(&mut self, key: &str, defaults: &[Level]) -> &mut Self {
        self.check_prefix("permission", key);
        if self.permissions.iter().any(|known| known.key == key) {
            self.errors.push(RegistrationError::Duplicate {
                module: self.module.clone(),
                kind: "permission",
                name: key.to_owned(),
            });
        }
        self.permissions.push(PermissionDef {
            key: key.to_owned(),
            defaults: defaults.to_vec(),
        });
        self
    }

    pub fn before<F, Fut>(&mut self, func: &str, hook: F) -> &mut Self
    where
        F: Fn(Call<Value>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Before>> + Send + 'static,
    {
        self.before.push(HookDef {
            function: func.to_owned(),
            hook: Arc::new(move |call| Box::pin(hook(call))),
        });
        self
    }

    pub fn within<F, Fut>(&mut self, func: &str, hook: F) -> &mut Self
    where
        F: Fn(Call<Value>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.within.push(HookDef {
            function: func.to_owned(),
            hook: Arc::new(move |call| Box::pin(hook(call))),
        });
        self
    }

    pub fn guard(&mut self, guard: Arc<dyn Guard>) -> &mut Self {
        self.guards.push(guard);
        self
    }

    pub fn patch(&mut self, patch: DescriptorPatch) -> &mut Self {
        self.patches.push(patch);
        self
    }

    pub fn contribute<T: Any + Send + Sync>(&mut self, mut contribution: T) -> &mut Self {
        let any: &mut dyn Any = &mut contribution;
        if let Some(lifecycle) = any.downcast_mut::<AccountLifecycle>() {
            lifecycle.stamp(&self.module);
            let taken = self.contributions.of::<AccountLifecycle>().next().is_some();
            let module = self.module.clone();
            self.check_unique("account lifecycle", &module, taken);
        }
        if let Some(endpoint) = any.downcast_mut::<Endpoint>() {
            endpoint.stamp(&self.module);
            let taken = self.contributions.of::<Endpoint>().any(|other| {
                other.name() == endpoint.name() && other.method() == endpoint.method()
            });
            let name = format!("{} {}", endpoint.method().as_str(), endpoint.name());
            self.check_unique("endpoint", &name, taken);
        }
        let any: &dyn Any = &contribution;
        if let Some(op) = any.downcast_ref::<AgentOp>() {
            self.check_prefix("agent operation", op.name());
            let taken = self
                .contributions
                .of::<AgentOp>()
                .any(|other| other.name() == op.name());
            self.check_unique("agent operation", op.name(), taken);
        }
        if let Some(limit) = any.downcast_ref::<LimitDef>() {
            self.check_prefix("limit", limit.key());
            let taken = self
                .contributions
                .of::<LimitDef>()
                .any(|other| other.key() == limit.key());
            self.check_unique("limit", limit.key(), taken);
        }
        if let Some(template) = any.downcast_ref::<ConfigTemplate>() {
            let taken = self
                .contributions
                .of::<ConfigTemplate>()
                .any(|other| other.id() == template.id());
            self.check_unique("configuration template", template.id(), taken);
        }
        self.contributions.add(contribution);
        self
    }

    pub fn provide<T: ?Sized + Send + Sync + 'static>(&mut self, service: Arc<T>) -> &mut Self {
        if !self.provides.provide(service) {
            self.errors.push(RegistrationError::Duplicate {
                module: self.module.clone(),
                kind: "service",
                name: ServiceKey::of::<T>().name().to_owned(),
            });
        }
        self
    }

    pub fn require<T: ?Sized + 'static>(&mut self) -> &mut Self {
        let key = ServiceKey::of::<T>();
        if !self.requires.contains(&key) {
            self.requires.push(key);
        }
        self
    }

    pub fn start<F, Fut>(&mut self, hook: F) -> &mut Self
    where
        F: FnOnce(Context) -> Fut + Send + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        if self.start.is_some() {
            self.errors.push(RegistrationError::Duplicate {
                module: self.module.clone(),
                kind: "start hook",
                name: "start".to_owned(),
            });
        }
        self.start = Some(Box::new(move |ctx| Box::pin(hook(ctx))));
        self
    }

    pub fn stop<F, Fut>(&mut self, hook: F) -> &mut Self
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        if self.stop.is_some() {
            self.errors.push(RegistrationError::Duplicate {
                module: self.module.clone(),
                kind: "stop hook",
                name: "stop".to_owned(),
            });
        }
        self.stop = Some(Box::new(move || Box::pin(hook())));
        self
    }

    pub fn invoker(&mut self, invoker: Arc<dyn Invoker>) -> &mut Self {
        if self.invoker.is_some() {
            self.errors.push(RegistrationError::Duplicate {
                module: self.module.clone(),
                kind: "invoker",
                name: "invoker".to_owned(),
            });
        }
        self.invoker = Some(invoker);
        self
    }

    pub fn errors(&self) -> &[RegistrationError] {
        &self.errors
    }

    pub fn function_names(&self) -> BTreeSet<&str> {
        self.functions.iter().map(FunctionDef::name).collect()
    }

    pub fn into_registrations(self) -> std::result::Result<Registrations, Vec<RegistrationError>> {
        if !self.errors.is_empty() {
            return Err(self.errors);
        }
        Ok(Registrations {
            module: self.module,
            functions: self.functions,
            views: self.views,
            menu: self.menu,
            i18n: self.i18n,
            permissions: self.permissions,
            before: self.before,
            within: self.within,
            guards: self.guards,
            patches: self.patches,
            contributions: self.contributions,
            provides: self.provides,
            requires: self.requires,
            start: self.start,
            stop: self.stop,
            invoker: self.invoker,
        })
    }

    fn check_unique(&mut self, kind: &'static str, name: &str, taken: bool) {
        if taken {
            self.errors.push(RegistrationError::Duplicate {
                module: self.module.clone(),
                kind,
                name: name.to_owned(),
            });
        }
    }

    fn check_prefix(&mut self, kind: &'static str, name: &str) {
        let dotted = self.module.replace('_', ".");
        let prefixed = [self.module.as_str(), dotted.as_str()]
            .iter()
            .any(|prefix| {
                name.strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with('.') && rest.len() > 1)
            });
        if !prefixed {
            self.errors.push(RegistrationError::Prefix {
                module: self.module.clone(),
                kind,
                name: name.to_owned(),
            });
        }
    }
}

impl fmt::Debug for Registrar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Registrar")
            .field("module", &self.module)
            .field("functions", &self.function_names())
            .field("views", &self.views)
            .field("menu", &self.menu)
            .field("permissions", &self.permissions)
            .field("errors", &self.errors)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keel_types::Kind;
    use schemars::{JsonSchema, json_schema};
    use serde::{Deserialize, Serialize};
    use serde_json::json;

    use crate::function::function;

    #[derive(Deserialize, Serialize, JsonSchema)]
    struct Nothing {}

    fn noop(name: &str) -> FunctionDef {
        function::<Nothing, Nothing>(name)
            .kind(Kind::Get)
            .handler(|_: Call<Nothing>| async { Ok(Nothing {}) })
    }

    trait Clock: Send + Sync {
        fn now(&self) -> u64;
    }

    struct Fixed;

    impl Clock for Fixed {
        fn now(&self) -> u64 {
            7
        }
    }

    struct Widget(&'static str);

    struct Open;

    #[async_trait::async_trait]
    impl Guard for Open {
        async fn check(&self, _: &str, _: &Call<Value>) -> Result<()> {
            Ok(())
        }
    }

    struct Local;

    impl Invoker for Local {
        fn invoke(
            &self,
            _: crate::call::CallHandle,
            _: &str,
            params: Value,
        ) -> BoxFuture<'static, Result<Value>> {
            Box::pin(async move { Ok(params) })
        }
    }

    #[tokio::test]
    async fn a_stream_registers_a_read_function_of_kind_stream_whose_call_attaches_its_lines() {
        use futures::StreamExt;
        use keel_types::{LogLevel, Mode};

        let mut r = Registrar::new("m");
        r.stream(
            function::<Nothing, LogLine>("m.e.tail").level(Level::User),
            |call: Call<Nothing>| async move {
                let login = call.principal().login.clone();
                Ok(Lines::new(futures::stream::iter([LogLine::new(
                    LogLevel::Info,
                    login,
                )])))
            },
        );
        let registrations = r.into_registrations().expect("registered");
        let def = &registrations.functions[0];
        assert_eq!(def.kind(), Kind::Stream);
        assert_eq!(def.mode(), Mode::Read);
        let handle = crate::tests::read_handle();
        let answer = (def.handler())(handle.call(json!({})))
            .await
            .expect("opened");
        assert_eq!(answer, Value::Null);
        let lines: Vec<LogLine> = handle.take_lines().expect("the lines").collect().await;
        assert_eq!(lines, [LogLine::new(LogLevel::Info, "alice")]);
        assert!(handle.take_lines().is_none());
    }

    #[test]
    fn a_registrar_hands_every_registration_back() {
        let mut r = Registrar::new("dns");
        r.function(noop("dns.zone.get"))
            .function(noop("dns.zone.list"))
            .menu(
                MenuNode::new("dns/zones", "dns.zone.list")
                    .icon("globe")
                    .order(10),
            )
            .i18n(
                "en",
                r#"{ "dns": { "zone": { "list": { "title": "Zones" } } } }"#,
            )
            .permission("dns.zone.read", &[Level::User])
            .permission("dns.zone.manage", &[Level::User, Level::Reseller])
            .view(
                "dns.zone-map",
                ViewSpec::new(Level::Reseller)
                    .permission("dns.zone.read")
                    .menu(MenuNode::new("dns/map", "dns.zone-map").label("dns.zone_map.title"))
                    .needs(["dns.map"]),
            )
            .view("dns.zone-health", ViewSpec::new(Level::User))
            .before("dns.zone.delete", |_: Call<Value>| async {
                Ok(Before::Proceed)
            })
            .within("keel.account.delete", |_: Call<Value>| async { Ok(()) })
            .guard(Arc::new(Open))
            .patch(
                DescriptorPatch::new("hosting.site.edit")
                    .field("dnsZone", json_schema!({ "type": "string" }))
                    .after("name")
                    .store(|_, _| async { Ok(()) })
                    .field("dnsTtl", json_schema!({ "type": "integer" })),
            )
            .contribute(Widget("zones"))
            .contribute(Widget("records"))
            .contribute(7u8)
            .provide::<dyn Clock>(Arc::new(Fixed))
            .require::<dyn Clock>()
            .require::<dyn Clock>()
            .start(|ctx| async move {
                assert_eq!(ctx.module(), "dns");
                Ok(())
            })
            .stop(|| async { Ok(()) });
        assert!(r.errors().is_empty());
        let registrations = r.into_registrations().unwrap();
        assert_eq!(registrations.module, "dns");
        assert_eq!(registrations.functions.len(), 2);
        assert_eq!(registrations.menu[0].path(), "dns/zones");
        assert_eq!(registrations.menu[0].icon_name(), Some("globe"));
        assert_eq!(registrations.menu[0].order_value(), 10);
        assert_eq!(registrations.menu[1].path(), "dns/map");
        assert_eq!(registrations.menu[1].func(), "dns.zone-map");
        assert_eq!(
            registrations.menu[1].label_key(),
            Some("dns.zone_map.title")
        );
        assert_eq!(
            registrations.views,
            [
                ViewDef {
                    name: "dns.zone-map".into(),
                    access: Access {
                        level: Level::Reseller,
                        permission: Some("dns.zone.read".into()),
                        feature: None,
                    },
                    needs: vec!["dns.map".into()],
                },
                ViewDef {
                    name: "dns.zone-health".into(),
                    access: Access {
                        level: Level::User,
                        permission: None,
                        feature: None,
                    },
                    needs: Vec::new(),
                },
            ]
        );
        assert_eq!(registrations.i18n[0].lang, "en");
        assert_eq!(
            registrations.i18n[0].messages["dns"]["zone"]["list"]["title"],
            json!("Zones")
        );
        assert_eq!(
            registrations.permissions[1].defaults,
            [Level::User, Level::Reseller]
        );
        assert_eq!(registrations.before[0].function, "dns.zone.delete");
        assert_eq!(registrations.within[0].function, "keel.account.delete");
        assert_eq!(registrations.guards.len(), 1);
        assert_eq!(registrations.patches[0].target(), "hosting.site.edit");
        assert_eq!(
            registrations.patches[0].fields()[0].after.as_deref(),
            Some("name")
        );
        assert!(registrations.patches[0].fields()[0].store.is_some());
        assert!(registrations.patches[0].fields()[1].store.is_none());
        assert_eq!(
            format!("{:?}", registrations.patches[0]),
            "DescriptorPatch { target: \"hosting.site.edit\", fields: [AddedField { name: \"dnsZone\", after: Some(\"name\"), store: true }, AddedField { name: \"dnsTtl\", after: None, store: false }], access: None }"
        );
        assert!(registrations.start.is_some());
        assert!(registrations.stop.is_some());
        assert!(registrations.invoker.is_none());
        let widgets: Vec<&str> = registrations
            .contributions
            .of::<Widget>()
            .map(|widget| widget.0)
            .collect();
        assert_eq!(widgets, ["zones", "records"]);
        assert_eq!(registrations.contributions.of::<u8>().count(), 1);
        assert_eq!(registrations.contributions.len(), 3);
        assert_eq!(registrations.provides.get::<dyn Clock>().unwrap().now(), 7);
        assert_eq!(registrations.requires, [ServiceKey::of::<dyn Clock>()]);
    }

    #[test]
    fn a_module_id_with_an_underscore_prefixes_its_names_with_the_underscore_read_as_a_dot() {
        use crate::limit::Unit;
        let mut r = Registrar::new("dns_record");
        r.function(noop("dns.record.list"))
            .function(noop("dns_record.list"))
            .function(noop("dns.zone.list"))
            .function(noop("dns.records.list"))
            .function(noop("dns.record."))
            .permission("dns.record.manage", &[])
            .contribute(LimitDef::new("dns.record.max", Unit::Count))
            .contribute(LimitDef::new("dns.records", Unit::Count));
        assert!(r.functions.iter().all(|def| def.module() == "dns_record"));
        let errors = r.into_registrations().unwrap_err();
        let listed: Vec<String> = errors.iter().map(ToString::to_string).collect();
        assert_eq!(
            listed,
            [
                "module dns_record: function \"dns.zone.list\" does not carry the module prefix",
                "module dns_record: function \"dns.records.list\" does not carry the module prefix",
                "module dns_record: function \"dns.record.\" does not carry the module prefix",
                "module dns_record: limit \"dns.records\" does not carry the module prefix",
            ]
        );
    }

    #[test]
    fn names_outside_the_module_prefix_and_duplicates_are_reported() {
        let mut r = Registrar::new("dns");
        r.function(noop("dnsx.zone.get"))
            .function(noop("dns.zone.get"))
            .function(noop("dns.zone.get"))
            .function(noop("dns."))
            .permission("auth.zone.read", &[])
            .view("dnsx.zone-map", ViewSpec::new(Level::User))
            .view("dns.zone.get", ViewSpec::new(Level::User))
            .view(
                "dns.zone-map",
                ViewSpec::new(Level::User).menu(MenuNode::new("dns/map", "dns.zone.list")),
            )
            .view("dns.zone-map", ViewSpec::new(Level::User))
            .function(noop("dns.zone-map"))
            .i18n("ru", "[1, 2]")
            .i18n("de", "{ not json")
            .provide::<dyn Clock>(Arc::new(Fixed))
            .provide::<dyn Clock>(Arc::new(Fixed))
            .start(|_| async { Ok(()) })
            .start(|_| async { Ok(()) })
            .stop(|| async { Ok(()) })
            .stop(|| async { Ok(()) })
            .invoker(Arc::new(Local))
            .invoker(Arc::new(Local));
        let errors = r.into_registrations().unwrap_err();
        let kinds: Vec<String> = errors.iter().map(ToString::to_string).collect();
        assert_eq!(
            kinds,
            [
                "module dns: function \"dnsx.zone.get\" does not carry the module prefix",
                "module dns: function \"dns.zone.get\" is registered twice",
                "module dns: function \"dns.\" does not carry the module prefix",
                "module dns: permission \"auth.zone.read\" does not carry the module prefix",
                "module dns: view \"dnsx.zone-map\" does not carry the module prefix",
                "module dns: view \"dns.zone.get\" is registered twice",
                "module dns: the menu node dns/map of the view dns.zone-map points at dns.zone.list",
                "module dns: view \"dns.zone-map\" is registered twice",
                "module dns: function \"dns.zone-map\" is registered twice",
                "module dns: the ru dictionary is not a JSON object: the document is not an object",
                "module dns: the de dictionary is not a JSON object: key must be a string at line 1 column 3",
                format!(
                    "module dns: service {:?} is registered twice",
                    ServiceKey::of::<dyn Clock>().name()
                )
                .as_str(),
                "module dns: start hook \"start\" is registered twice",
                "module dns: stop hook \"stop\" is registered twice",
                "module dns: invoker \"invoker\" is registered twice",
            ]
        );
    }

    #[test]
    fn agent_operations_carry_the_module_prefix_and_ids_are_contributed_once_per_module() {
        fn stat(
            _: &dyn crate::host::AgentScope,
            name: String,
        ) -> std::result::Result<String, crate::host::AgentOpError> {
            Ok(name)
        }
        let mut r = Registrar::new("vm");
        r.contribute(AgentOp::new::<String, String>("vm.libvirt.create", stat))
            .contribute(AgentOp::new::<String, String>("vm.libvirt.create", stat))
            .contribute(AgentOp::new::<String, String>("host.fs.write", stat))
            .contribute(ConfigTemplate::new("libvirt/domain.xml.j2", "x"))
            .contribute(ConfigTemplate::new("libvirt/domain.xml.j2", "y"))
            .contribute(ConfigTemplate::new("bind/zone.j2", "z"));
        let errors = r.into_registrations().unwrap_err();
        let listed: Vec<String> = errors.iter().map(ToString::to_string).collect();
        assert_eq!(
            listed,
            [
                "module vm: agent operation \"vm.libvirt.create\" is registered twice",
                "module vm: agent operation \"host.fs.write\" does not carry the module prefix",
                "module vm: configuration template \"libvirt/domain.xml.j2\" is registered twice",
            ]
        );
    }

    #[test]
    fn a_limit_carries_the_module_prefix_and_is_contributed_once() {
        use crate::limit::Unit;
        let mut r = Registrar::new("dns");
        r.contribute(LimitDef::new("dns.zones", Unit::Count).default(50))
            .contribute(LimitDef::new("dns.zones", Unit::Count))
            .contribute(LimitDef::new("mail.boxes", Unit::Count));
        let errors = r.into_registrations().unwrap_err();
        let listed: Vec<String> = errors.iter().map(ToString::to_string).collect();
        assert_eq!(
            listed,
            [
                "module dns: limit \"dns.zones\" is registered twice",
                "module dns: limit \"mail.boxes\" does not carry the module prefix",
            ]
        );
    }

    #[test]
    fn an_account_lifecycle_carries_the_contributing_module_and_is_contributed_once() {
        let mut r = Registrar::new("zones");
        r.contribute(AccountLifecycle::new().on_delete(|_| async { Ok(()) }));
        let registrations = r.into_registrations().expect("one lifecycle is valid");
        let lifecycles: Vec<&AccountLifecycle> = registrations
            .contributions
            .of::<AccountLifecycle>()
            .collect();
        assert_eq!(lifecycles.len(), 1);
        assert_eq!(lifecycles[0].module(), "zones");
        let mut twice = Registrar::new("zones");
        twice
            .contribute(AccountLifecycle::new())
            .contribute(AccountLifecycle::new());
        let errors = twice.into_registrations().unwrap_err();
        let listed: Vec<String> = errors.iter().map(ToString::to_string).collect();
        assert_eq!(
            listed,
            ["module zones: account lifecycle \"zones\" is registered twice"]
        );
    }

    #[test]
    fn an_endpoint_carries_the_contributing_module_and_one_name_takes_each_method_once() {
        let mut r = Registrar::new("relay");
        r.contribute(Endpoint::get("feed", |_| async { Ok(json!([])) }))
            .contribute(Endpoint::post("feed", |_| async { Ok(json!({})) }));
        let registrations = r.into_registrations().expect("two methods are valid");
        let endpoints: Vec<(&str, &str, &str)> = registrations
            .contributions
            .of::<Endpoint>()
            .map(|endpoint| {
                (
                    endpoint.module(),
                    endpoint.method().as_str(),
                    endpoint.name(),
                )
            })
            .collect();
        assert_eq!(
            endpoints,
            [("relay", "GET", "feed"), ("relay", "POST", "feed")]
        );
        let mut twice = Registrar::new("relay");
        twice
            .contribute(Endpoint::post("hook", |_| async { Ok(json!({})) }))
            .contribute(Endpoint::post("hook", |_| async { Ok(json!({})) }));
        let errors = twice.into_registrations().unwrap_err();
        let listed: Vec<String> = errors.iter().map(ToString::to_string).collect();
        assert_eq!(
            listed,
            ["module relay: endpoint \"POST hook\" is registered twice"]
        );
    }

    #[test]
    fn contributions_can_be_taken_out_by_type() {
        let mut contributions = Contributions::default();
        contributions.add(Widget("a"));
        contributions.add(1u8);
        let mut other = Contributions::default();
        other.add(Widget("b"));
        contributions.extend(other);
        let widgets = contributions.take::<Widget>();
        assert_eq!(widgets.len(), 2);
        assert!(contributions.take::<Widget>().is_empty());
        assert_eq!(contributions.len(), 1);
        assert_eq!(format!("{contributions:?}"), "Contributions(1)");
    }
}
