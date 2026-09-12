use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::sync::Arc;

use keel_sdk::runtime::{
    Contributions, Guard, HookDef, Invoker, PermissionDef, ServiceKey, Services, StartHook,
    StopHook, UiError,
};
use keel_sdk::{FunctionDef, MenuNode};
use keel_types::descriptor::{KeelAttrs, UiAttrs, X_KEEL, X_UI};
use keel_types::ui::{ColumnFilter, ScreenUi};
use keel_types::{Access, Descriptor, Kind};
use schemars::Schema;
use serde_json::Value;

use super::composition::Graph;
use super::i18n::{Catalogue, REQUIRED_LANGUAGE};
use super::init::Initialised;
use super::patch;
use crate::registry::{Function, Hook, ModuleInfo, Registry};

pub struct Frozen {
    pub(super) graph: Graph,
    pub(super) registry: Arc<Registry>,
    pub(super) lifecycle: Vec<Lifecycle>,
}

pub(super) struct Lifecycle {
    pub module: String,
    pub start: Option<StartHook>,
    pub stop: Option<StopHook>,
}

#[derive(Debug, thiserror::Error)]
pub struct FreezeError {
    problems: Vec<FreezeProblem>,
}

#[derive(Debug, thiserror::Error)]
pub enum FreezeProblem {
    #[error("module {module} requires the service {service}, which no module provides")]
    ServiceMissing { module: String, service: String },
    #[error("the service {service} is provided by both {first} and {second}")]
    ServiceProvidedTwice {
        service: String,
        first: String,
        second: String,
    },
    #[error(
        "an invoker is registered by both {first} and {second}; the kernel has one invoker slot"
    )]
    InvokerProvidedTwice { first: String, second: String },
    #[error("the function {function} is registered by both {first} and {second}")]
    FunctionRegisteredTwice {
        function: String,
        first: String,
        second: String,
    },
    #[error("the permission {permission} is registered by both {first} and {second}")]
    PermissionRegisteredTwice {
        permission: String,
        first: String,
        second: String,
    },
    #[error("module {module}: {kind} hook on {function}, which is not registered")]
    HookTarget {
        module: String,
        kind: &'static str,
        function: String,
    },
    #[error("module {module}: patch on {target}, which is not registered")]
    PatchTarget { module: String, target: String },
    #[error(
        "module {module}: patch on {target}, whose parameters have no properties to add a field to"
    )]
    PatchNotPatchable { module: String, target: String },
    #[error(
        "module {module}: patch on {target} weakens access from {} to {}",
        access(.from),
        access(.to)
    )]
    PatchWeakensAccess {
        module: String,
        target: String,
        from: Access,
        to: Access,
    },
    #[error("module {module}: patch on {target} adds the field {field} without a store hook")]
    PatchFieldWithoutStore {
        module: String,
        target: String,
        field: String,
    },
    #[error(
        "module {module}: patch on {target} adds the field {field}, which the function already has"
    )]
    PatchFieldExists {
        module: String,
        target: String,
        field: String,
    },
    #[error(
        "module {module}: patch on {target} places {field} after {after}, which the function does not have"
    )]
    PatchAfterUnknown {
        module: String,
        target: String,
        field: String,
        after: String,
    },
    #[error("function {function}: {source}")]
    Descriptor {
        function: String,
        #[source]
        source: UiError,
    },
    #[error(
        "function {function}: field {field} carries an invalid {namespace} attribute: {reason}"
    )]
    Attrs {
        function: String,
        field: String,
        namespace: &'static str,
        reason: String,
    },
    #[error("function {function}: {member} names {target}, which is not registered")]
    Reference {
        function: String,
        member: String,
        target: String,
    },
    #[error(
        "function {function}: {member} names {target}, which is a {} function, not {}",
        kind(*.found),
        kind(*.expected)
    )]
    ReferenceKind {
        function: String,
        member: String,
        target: String,
        expected: Kind,
        found: Kind,
    },
    #[error(
        "function {function}: {member} names the action {action}, which the screen does not have"
    )]
    ActionUnknown {
        function: String,
        member: String,
        action: String,
    },
    #[error(
        "function {function}: {member} names the permission {permission}, which no module registers"
    )]
    PermissionUnknown {
        function: String,
        member: String,
        permission: String,
    },
    #[error("module {module}: menu node {path} points at {function}, which is not registered")]
    MenuFunction {
        module: String,
        path: String,
        function: String,
    },
    #[error("the menu path {path} is registered by both {first} and {second}")]
    MenuPathTwice {
        path: String,
        first: String,
        second: String,
    },
    #[error("{owner}: the {REQUIRED_LANGUAGE} dictionary has no entry for {key}")]
    Translation { owner: String, key: String },
    #[error("the {lang} dictionaries of {first} and {second} both define {key}")]
    TranslationConflict {
        lang: String,
        key: String,
        first: String,
        second: String,
    },
}

fn access(access: &Access) -> String {
    match &access.permission {
        Some(permission) => format!("level {}, permission {permission}", access.level),
        None => format!("level {}", access.level),
    }
}

fn kind(kind: Kind) -> &'static str {
    match kind {
        Kind::List => "list",
        Kind::Get => "get",
        Kind::Form => "form",
        Kind::Action => "action",
        Kind::Wizard => "wizard",
        Kind::Report => "report",
        Kind::Options => "options",
        Kind::Stream => "stream",
        Kind::Download => "download",
        Kind::Upload => "upload",
    }
}

struct Freezer {
    problems: Vec<FreezeProblem>,
    services: Services,
    providers: HashMap<ServiceKey, String>,
    invoker: Option<(String, Arc<dyn Invoker>)>,
    known: BTreeMap<String, Kind>,
    functions: BTreeMap<String, Function>,
    owners: BTreeMap<String, String>,
    permissions: BTreeMap<String, PermissionDef>,
    permission_owners: BTreeMap<String, String>,
    guards: Vec<Hook<Arc<dyn Guard>>>,
    menu: Vec<MenuNode>,
    menu_owners: BTreeMap<String, String>,
    catalogue: Catalogue,
    dictionary_owners: BTreeMap<(String, String), String>,
    contributions: Contributions,
    lifecycle: Vec<Lifecycle>,
}

impl Initialised {
    pub fn freeze(self) -> Result<Frozen, FreezeError> {
        let mut freezer = Freezer {
            problems: Vec::new(),
            services: Services::new(),
            providers: HashMap::new(),
            invoker: None,
            known: BTreeMap::new(),
            functions: BTreeMap::new(),
            owners: BTreeMap::new(),
            permissions: BTreeMap::new(),
            permission_owners: BTreeMap::new(),
            guards: Vec::new(),
            menu: Vec::new(),
            menu_owners: BTreeMap::new(),
            catalogue: Catalogue::default(),
            dictionary_owners: BTreeMap::new(),
            contributions: Contributions::default(),
            lifecycle: Vec::new(),
        };
        let mut requirements = Vec::new();
        let mut hooks = Vec::new();
        let mut patches = Vec::new();
        for registrations in self.registrations {
            let module = registrations.module;
            freezer.provide(&module, registrations.provides);
            requirements.push((module.clone(), registrations.requires));
            if let Some(invoker) = registrations.invoker {
                freezer.invoker(&module, invoker);
            }
            for def in registrations.functions {
                freezer.function(&module, def);
            }
            for permission in registrations.permissions {
                freezer.permission(&module, permission);
            }
            for guard in registrations.guards {
                freezer.guards.push(Hook {
                    module: module.clone(),
                    hook: guard,
                });
            }
            for node in registrations.menu {
                freezer.menu(&module, node);
            }
            for bundle in registrations.i18n {
                freezer.dictionary(&module, &bundle.lang, &bundle.messages);
            }
            hooks.push((module.clone(), registrations.before, registrations.within));
            patches.push((module.clone(), registrations.patches));
            freezer.contributions.extend(registrations.contributions);
            freezer.lifecycle.push(Lifecycle {
                module,
                start: registrations.start,
                stop: registrations.stop,
            });
        }
        for (module, required) in requirements {
            freezer.require(&module, &required);
        }
        for (module, before, within) in hooks {
            freezer.hooks(&module, before, within);
        }
        for (module, patches) in patches {
            for patch in patches {
                freezer.patch(&module, &patch);
            }
        }
        freezer.references();
        freezer.translations();
        freezer.finish(self.graph)
    }
}

impl Freezer {
    fn provide(&mut self, module: &str, provides: Services) {
        for key in provides.keys() {
            match self.providers.get(&key) {
                Some(first) => self.problems.push(FreezeProblem::ServiceProvidedTwice {
                    service: key.name().to_owned(),
                    first: first.clone(),
                    second: module.to_owned(),
                }),
                None => {
                    self.providers.insert(key, module.to_owned());
                }
            }
        }
        self.services.merge(provides);
    }

    fn require(&mut self, module: &str, required: &[ServiceKey]) {
        for key in required {
            if !self.services.contains(key) {
                self.problems.push(FreezeProblem::ServiceMissing {
                    module: module.to_owned(),
                    service: key.name().to_owned(),
                });
            }
        }
    }

    fn invoker(&mut self, module: &str, invoker: Arc<dyn Invoker>) {
        match &self.invoker {
            Some((first, _)) => self.problems.push(FreezeProblem::InvokerProvidedTwice {
                first: first.clone(),
                second: module.to_owned(),
            }),
            None => self.invoker = Some((module.to_owned(), invoker)),
        }
    }

    fn function(&mut self, module: &str, def: FunctionDef) {
        let name = def.name().to_owned();
        if let Some(first) = self.owners.get(&name) {
            self.problems.push(FreezeProblem::FunctionRegisteredTwice {
                function: name,
                first: first.clone(),
                second: module.to_owned(),
            });
            return;
        }
        self.owners.insert(name.clone(), module.to_owned());
        self.known.insert(name.clone(), def.kind());
        match def.descriptor() {
            Ok(descriptor) => {
                self.functions.insert(
                    name,
                    Function {
                        def,
                        descriptor,
                        before: Vec::new(),
                        within: Vec::new(),
                        patched: Vec::new(),
                    },
                );
            }
            Err(source) => self.problems.push(FreezeProblem::Descriptor {
                function: name,
                source,
            }),
        }
    }

    fn permission(&mut self, module: &str, permission: PermissionDef) {
        if let Some(first) = self.permission_owners.get(&permission.key) {
            self.problems
                .push(FreezeProblem::PermissionRegisteredTwice {
                    permission: permission.key,
                    first: first.clone(),
                    second: module.to_owned(),
                });
            return;
        }
        self.permission_owners
            .insert(permission.key.clone(), module.to_owned());
        self.permissions.insert(permission.key.clone(), permission);
    }

    fn menu(&mut self, module: &str, node: MenuNode) {
        if let Some(first) = self.menu_owners.get(node.path()) {
            self.problems.push(FreezeProblem::MenuPathTwice {
                path: node.path().to_owned(),
                first: first.clone(),
                second: module.to_owned(),
            });
            return;
        }
        self.menu_owners
            .insert(node.path().to_owned(), module.to_owned());
        self.menu.push(node);
    }

    fn dictionary(&mut self, module: &str, lang: &str, messages: &Value) {
        for conflict in self.catalogue.merge(lang, messages) {
            let first = self
                .dictionary_owners
                .get(&(conflict.lang.clone(), conflict.key.clone()))
                .cloned()
                .unwrap_or_else(|| "an earlier module".to_owned());
            self.problems.push(FreezeProblem::TranslationConflict {
                lang: conflict.lang,
                key: conflict.key,
                first,
                second: module.to_owned(),
            });
        }
        if let Value::Object(messages) = messages {
            let mut keys = Vec::new();
            leaves(messages, &mut Vec::new(), &mut keys);
            for key in keys {
                self.dictionary_owners
                    .entry((lang.to_owned(), key))
                    .or_insert_with(|| module.to_owned());
            }
        }
    }

    fn hooks(
        &mut self,
        module: &str,
        before: Vec<HookDef<keel_sdk::runtime::BeforeHook>>,
        within: Vec<HookDef<keel_sdk::runtime::WithinHook>>,
    ) {
        for hook in before {
            match self.functions.get_mut(&hook.function) {
                Some(function) => function.before.push(Hook {
                    module: module.to_owned(),
                    hook: hook.hook,
                }),
                None if self.known.contains_key(&hook.function) => {}
                None => self.problems.push(FreezeProblem::HookTarget {
                    module: module.to_owned(),
                    kind: "before",
                    function: hook.function,
                }),
            }
        }
        for hook in within {
            match self.functions.get_mut(&hook.function) {
                Some(function) => function.within.push(Hook {
                    module: module.to_owned(),
                    hook: hook.hook,
                }),
                None if self.known.contains_key(&hook.function) => {}
                None => self.problems.push(FreezeProblem::HookTarget {
                    module: module.to_owned(),
                    kind: "within",
                    function: hook.function,
                }),
            }
        }
    }

    fn patch(&mut self, module: &str, patch: &keel_sdk::DescriptorPatch) {
        match self.functions.get_mut(patch.target()) {
            Some(function) => {
                let patched =
                    patch::apply(module, patch, &mut function.descriptor, &mut self.problems);
                function.patched.extend(patched);
            }
            None if self.known.contains_key(patch.target()) => {}
            None => self.problems.push(FreezeProblem::PatchTarget {
                module: module.to_owned(),
                target: patch.target().to_owned(),
            }),
        }
    }

    fn references(&mut self) {
        let mut problems = Vec::new();
        for function in self.functions.values() {
            let mut checker = References {
                function: function.name(),
                known: &self.known,
                permissions: &self.permissions,
                problems: &mut problems,
            };
            checker.descriptor(&function.descriptor);
        }
        for node in &self.menu {
            if !self.known.contains_key(node.func()) {
                problems.push(FreezeProblem::MenuFunction {
                    module: self
                        .menu_owners
                        .get(node.path())
                        .cloned()
                        .unwrap_or_default(),
                    path: node.path().to_owned(),
                    function: node.func().to_owned(),
                });
            }
        }
        self.problems.extend(problems);
    }

    fn translations(&mut self) {
        let mut required: Vec<(String, String)> = Vec::new();
        for function in self.functions.values() {
            let owner = format!("function {}", function.name());
            if let Some(ui) = &function.descriptor.ui {
                required.push((owner.clone(), ui.title.clone()));
                if let Some(empty) = &ui.empty {
                    required.push((owner.clone(), empty.title.clone()));
                }
            }
        }
        for node in &self.menu {
            if let Some(label) = node.label_key() {
                required.push((format!("menu node {}", node.path()), label.to_owned()));
            }
        }
        for (owner, key) in required {
            if !self.catalogue.has(REQUIRED_LANGUAGE, &key) {
                self.problems
                    .push(FreezeProblem::Translation { owner, key });
            }
        }
    }

    fn finish(self, graph: Graph) -> Result<Frozen, FreezeError> {
        if !self.problems.is_empty() {
            return Err(FreezeError {
                problems: self.problems,
            });
        }
        let modules = graph
            .modules()
            .iter()
            .map(|def| ModuleInfo {
                id: def.id.to_owned(),
                version: def.version.to_owned(),
            })
            .collect();
        let registry = Registry {
            modules,
            functions: self.functions,
            menu: self.menu,
            permissions: self.permissions,
            guards: self.guards,
            i18n: self.catalogue.into_languages(),
            services: Arc::new(self.services),
            contributions: Arc::new(self.contributions),
            invoker: self.invoker.map(|(_, invoker)| invoker),
        };
        tracing::info!(
            modules = ?graph.ids(),
            functions = registry.functions.len(),
            "registries frozen"
        );
        Ok(Frozen {
            graph,
            registry: Arc::new(registry),
            lifecycle: self.lifecycle,
        })
    }
}

fn leaves(
    messages: &serde_json::Map<String, Value>,
    path: &mut Vec<String>,
    into: &mut Vec<String>,
) {
    for (key, value) in messages {
        path.push(key.clone());
        match value {
            Value::Object(nested) => leaves(nested, path, into),
            _ => into.push(path.join(".")),
        }
        path.pop();
    }
}

struct References<'a> {
    function: &'a str,
    known: &'a BTreeMap<String, Kind>,
    permissions: &'a BTreeMap<String, PermissionDef>,
    problems: &'a mut Vec<FreezeProblem>,
}

impl References<'_> {
    fn descriptor(&mut self, descriptor: &Descriptor) {
        if let Some(permission) = &descriptor.access.permission {
            self.permission("access.permission", permission);
        }
        self.params(&descriptor.params);
        if let Some(ui) = &descriptor.ui {
            self.screen(ui);
        }
    }

    fn params(&mut self, params: &Schema) {
        let properties = params
            .as_object()
            .and_then(|schema| schema.get("properties"))
            .and_then(Value::as_object);
        for (field, schema) in properties.into_iter().flatten() {
            let Ok(schema) = Schema::try_from(schema.clone()) else {
                continue;
            };
            match KeelAttrs::of(&schema) {
                Ok(Some(attrs)) => {
                    if let Some(options) = &attrs.options {
                        self.function(
                            &format!("field {field} {X_KEEL}.options"),
                            options,
                            Some(Kind::Options),
                        );
                    }
                }
                Ok(None) => {}
                Err(error) => self.attrs(field, X_KEEL, error),
            }
            if let Err(error) = UiAttrs::of(&schema) {
                self.attrs(field, X_UI, error);
            }
        }
    }

    fn screen(&mut self, ui: &ScreenUi) {
        if let Some(read) = &ui.read {
            self.function("ui.read", read, Some(Kind::Get));
        }
        for page in &ui.pages {
            if let Some(validate) = &page.validate {
                self.function(&format!("page {} validate", page.id), validate, None);
            }
        }
        for (name, field) in &ui.fields {
            if let Some(recompute) = &field.recompute {
                self.function(&format!("field {name} recompute"), recompute, None);
            }
        }
        let actions: BTreeSet<&str> = ui.actions.iter().map(|action| action.id.as_str()).collect();
        for action in &ui.actions {
            if let Some(function) = &action.function {
                self.function(&format!("action {} function", action.id), function, None);
            }
            if let Some(screen) = &action.screen {
                self.function(&format!("action {} screen", action.id), screen, None);
            }
        }
        for column in &ui.columns {
            if let Some(edit) = &column.edit {
                self.function(&format!("column {} edit", column.key), &edit.function, None);
            }
            if let Some(ColumnFilter::Remote { remote }) = &column.filter {
                self.function(
                    &format!("column {} filter", column.key),
                    remote,
                    Some(Kind::Options),
                );
            }
            if let Some(link) = &column.to {
                self.function(&format!("column {} to", column.key), &link.screen, None);
            }
            if let Some(permission) = column
                .access
                .as_ref()
                .and_then(|access| access.permission.as_ref())
            {
                self.permission(
                    &format!("column {} access.permission", column.key),
                    permission,
                );
            }
        }
        if let Some(action) = ui.empty.as_ref().and_then(|empty| empty.action.as_ref()) {
            self.action("ui.empty.action", action, &actions);
        }
        for action in &ui.row_actions {
            self.action("ui.row_actions", action, &actions);
        }
    }

    fn function(&mut self, member: &str, target: &str, expected: Option<Kind>) {
        match (self.known.get(target), expected) {
            (None, _) => self.problems.push(FreezeProblem::Reference {
                function: self.function.to_owned(),
                member: member.to_owned(),
                target: target.to_owned(),
            }),
            (Some(found), Some(expected)) if *found != expected => {
                self.problems.push(FreezeProblem::ReferenceKind {
                    function: self.function.to_owned(),
                    member: member.to_owned(),
                    target: target.to_owned(),
                    expected,
                    found: *found,
                })
            }
            _ => {}
        }
    }

    fn action(&mut self, member: &str, action: &str, actions: &BTreeSet<&str>) {
        if !actions.contains(action) {
            self.problems.push(FreezeProblem::ActionUnknown {
                function: self.function.to_owned(),
                member: member.to_owned(),
                action: action.to_owned(),
            });
        }
    }

    fn permission(&mut self, member: &str, permission: &str) {
        if !self.permissions.contains_key(permission) {
            self.problems.push(FreezeProblem::PermissionUnknown {
                function: self.function.to_owned(),
                member: member.to_owned(),
                permission: permission.to_owned(),
            });
        }
    }

    fn attrs(&mut self, field: &str, namespace: &'static str, error: serde_json::Error) {
        self.problems.push(FreezeProblem::Attrs {
            function: self.function.to_owned(),
            field: field.to_owned(),
            namespace,
            reason: error.to_string(),
        });
    }
}

impl Frozen {
    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    pub fn registry(&self) -> &Arc<Registry> {
        &self.registry
    }
}

impl fmt::Debug for Frozen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Frozen")
            .field("graph", &self.graph)
            .field("registry", &self.registry)
            .finish()
    }
}

impl FreezeError {
    pub fn problems(&self) -> &[FreezeProblem] {
        &self.problems
    }
}

impl fmt::Display for FreezeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the registries cannot be frozen:")?;
        for problem in &self.problems {
            write!(f, "\n  - {problem}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use futures::future::BoxFuture;
    use keel_sdk::prelude::*;
    use keel_sdk::runtime::{CallHandle, Invoker, UiError};
    use keel_sdk::{DescriptorPatch, Registrar};
    use keel_types::ColumnType;
    use schemars::json_schema;

    use super::*;
    use crate::loader::Composition;
    use crate::loader::testing::{Item, Nothing, form, get, module, options};

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

    struct Local;

    impl Invoker for Local {
        fn invoke(
            &self,
            _: CallHandle,
            _: &str,
            params: Value,
        ) -> BoxFuture<'static, Result<Value>> {
            Box::pin(async move { Ok(params) })
        }
    }

    const EN: &str = r#"{
        "base": { "item": { "edit": { "title": "Item" }, "list": { "title": "Items" }, "empty": "Nothing" } },
        "menu": { "base": "Base" }
    }"#;

    fn base_init(r: &mut Registrar) -> Result<()> {
        r.function(form(
            "base.item.edit",
            Form::new("base.item.edit.title")
                .read("base.item.get")
                .page("main", ["title", "kind"])
                .page("more", ["weight"])
                .show_if("kind == 'heavy'")
                .field("title")
                .readonly_if("$mode == 'edit'"),
        ))
        .function(get("base.item.get"))
        .function(options("base.item.options"))
        .function(
            function::<ListQuery, Page<Item>>("base.item.list")
                .kind(Kind::List)
                .access(Level::User, "base.item.read")
                .unowned()
                .ui(List::new("base.item.list.title")
                    .column(Column::new("title", ColumnType::Text).permission("base.item.read"))
                    .action(Action::new("edit", ActionRole::Edit).screen("base.item.edit"))
                    .row_actions(["edit"])
                    .empty("base.item.empty", Some("edit")))
                .handler(|_: Call<ListQuery>| async { Ok(Page::new(Vec::new(), 0)) }),
        )
        .permission("base.item.read", &[Level::User])
        .permission("base.item.manage", &[Level::User])
        .menu(MenuNode::new("base/items", "base.item.list").icon("box"))
        .i18n("en", EN)
        .provide::<dyn Clock>(Arc::new(Fixed))
        .contribute(Widget("base"))
        .invoker(Arc::new(Local));
        Ok(())
    }

    module!(BASE, "base", [], base_init);

    fn freeze(modules: &[&'static keel_sdk::ModuleDef]) -> Result<Frozen, FreezeError> {
        let mut composition = Composition::new();
        for module in modules {
            composition = composition.module(module);
        }
        composition
            .resolve()
            .expect("the composition resolves")
            .init()
            .expect("init registers")
            .freeze()
    }

    fn problems(modules: &[&'static keel_sdk::ModuleDef]) -> Vec<String> {
        freeze(modules)
            .expect_err("the freeze fails")
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn init_and_freeze_run_without_a_database_so_init_cannot_do_io() {
        let frozen = freeze(&[&BASE]).unwrap();
        let registry = frozen.registry();
        assert_eq!(registry.modules().len(), 1);
        assert_eq!(registry.modules()[0].id, "base");
        assert_eq!(registry.functions().count(), 4);
        assert!(registry.function("base.item.edit").is_some());
        assert!(registry.function("base.item.delete").is_none());
        assert_eq!(registry.menu().len(), 1);
        assert_eq!(registry.permissions().len(), 2);
        assert!(registry.permission("base.item.read").is_some());
        assert_eq!(registry.languages().collect::<Vec<_>>(), ["en"]);
        assert!(registry.i18n("en").is_some());
        assert!(registry.i18n("ru").is_none());
        assert_eq!(registry.services().get::<dyn Clock>().unwrap().now(), 7);
        assert_eq!(registry.contributions().of::<Widget>().count(), 1);
        assert!(registry.invoker().is_some());
        assert_eq!(frozen.graph().ids(), ["base"]);
        assert!(format!("{frozen:?}").contains("Registry"));
        assert!(format!("{registry:?}").contains("\"base.item.edit\""));
    }

    #[test]
    fn a_compiled_descriptor_carries_the_conditions_as_ast() {
        let frozen = freeze(&[&BASE]).unwrap();
        let function = frozen.registry().function("base.item.edit").unwrap();
        assert_eq!(function.name(), "base.item.edit");
        assert_eq!(function.module(), "base");
        assert_eq!(function.def().kind(), Kind::Form);
        let ui = function.descriptor().ui.as_ref().unwrap();
        assert_eq!(
            serde_json::to_value(&ui.pages[1].show_if).unwrap(),
            json!({ "eq": ["kind", "heavy"] })
        );
        assert!(function.before().is_empty());
        assert!(function.within().is_empty());
        assert!(function.patched().is_empty());
        assert!(format!("{function:?}").starts_with("Function"));
    }

    fn needy_init(r: &mut Registrar) -> Result<()> {
        r.require::<dyn Clock>();
        Ok(())
    }

    module!(NEEDY, "needy", [], needy_init);

    #[test]
    fn require_of_a_service_nobody_provides_fails_at_freeze_not_at_the_first_call() {
        let listed = problems(&[&NEEDY]);
        assert_eq!(listed.len(), 1);
        assert!(
            listed[0].starts_with("module needy requires the service "),
            "{listed:?}"
        );
        assert!(
            listed[0].ends_with("Clock, which no module provides"),
            "{listed:?}"
        );
        let frozen = freeze(&[&NEEDY, &BASE]).unwrap();
        assert!(frozen.registry().services().get::<dyn Clock>().is_some());
    }

    fn second_clock_init(r: &mut Registrar) -> Result<()> {
        r.provide::<dyn Clock>(Arc::new(Fixed))
            .invoker(Arc::new(Local));
        Ok(())
    }

    module!(SECOND_CLOCK, "second_clock", [], second_clock_init);

    #[test]
    fn a_service_and_the_invoker_can_have_one_provider_only() {
        let listed = problems(&[&BASE, &SECOND_CLOCK]);
        assert_eq!(listed.len(), 2, "{listed:?}");
        assert!(listed[0].starts_with("the service "), "{listed:?}");
        assert!(
            listed[0].ends_with("Clock is provided by both base and second_clock"),
            "{listed:?}"
        );
        assert_eq!(
            listed[1],
            "an invoker is registered by both base and second_clock; the kernel has one invoker slot"
        );
    }

    fn typo_init(r: &mut Registrar) -> Result<()> {
        r.function(form(
            "typo.item.edit",
            Form::new("typo.item.edit.title")
                .page("main", ["title", "kind"])
                .field("title")
                .readonly_if("kindd == 'x'"),
        ))
        .i18n(
            "en",
            r#"{ "typo": { "item": { "edit": { "title": "Item" } } } }"#,
        );
        Ok(())
    }

    module!(TYPO, "typo", [], typo_init);

    #[test]
    fn a_condition_naming_an_unknown_field_fails_start_with_the_function_and_field_named() {
        let error = freeze(&[&TYPO]).unwrap_err();
        assert!(matches!(
            error.problems(),
            [FreezeProblem::Descriptor {
                function,
                source: UiError::UnknownField { field, .. }
            }] if function == "typo.item.edit" && field == "kindd"
        ));
        assert_eq!(
            error.to_string(),
            "the registries cannot be frozen:\n  - function typo.item.edit: field title readonly_if: \"kindd == 'x'\" refers to an unknown field \"kindd\""
        );
    }

    fn untitled_init(r: &mut Registrar) -> Result<()> {
        r.function(form(
            "untitled.item.edit",
            Form::new("untitled.item.edit.title").page("main", ["title"]),
        ))
        .i18n(
            "en",
            r#"{ "untitled": { "item": { "edit": { "hint": "no title here" } } } }"#,
        )
        .i18n(
            "ru",
            r#"{ "untitled": { "item": { "edit": { "title": "Item in ru" } } } }"#,
        );
        Ok(())
    }

    module!(UNTITLED, "untitled", [], untitled_init);

    #[test]
    fn a_function_whose_title_key_has_no_en_entry_fails_start() {
        assert_eq!(
            problems(&[&UNTITLED]),
            [
                "function untitled.item.edit: the en dictionary has no entry for untitled.item.edit.title"
            ]
        );
    }

    fn lowering_init(r: &mut Registrar) -> Result<()> {
        r.patch(DescriptorPatch::new("base.item.list").access(Level::Guest, None))
            .patch(
                DescriptorPatch::new("base.item.list")
                    .access(Level::Admin, Some("base.item.manage")),
            )
            .patch(DescriptorPatch::new("base.item.list").access(Level::Admin, None));
        Ok(())
    }

    module!(LOWERING, "lowering", ["base"], lowering_init);

    #[test]
    fn a_patch_that_lowers_access_is_rejected() {
        assert_eq!(
            problems(&[&BASE, &LOWERING]),
            [
                "module lowering: patch on base.item.list weakens access from level user, permission base.item.read to level guest",
                "module lowering: patch on base.item.list weakens access from level user, permission base.item.read to level admin, permission base.item.manage",
            ]
        );
    }

    fn raising_init(r: &mut Registrar) -> Result<()> {
        r.patch(DescriptorPatch::new("base.item.list").access(Level::Admin, None))
            .patch(
                DescriptorPatch::new("base.item.edit")
                    .access(Level::User, Some("base.item.manage")),
            );
        Ok(())
    }

    module!(RAISING, "raising", ["base"], raising_init);

    #[test]
    fn a_patch_may_raise_the_level_and_add_a_permission_where_there_was_none() {
        let frozen = freeze(&[&BASE, &RAISING]).unwrap();
        let list = frozen.registry().function("base.item.list").unwrap();
        assert_eq!(
            list.descriptor().access,
            Access {
                level: Level::Admin,
                permission: Some("base.item.read".into()),
                feature: None
            }
        );
        let edit = frozen.registry().function("base.item.edit").unwrap();
        assert_eq!(
            edit.descriptor().access,
            Access {
                level: Level::User,
                permission: Some("base.item.manage".into()),
                feature: None
            }
        );
    }

    fn storeless_init(r: &mut Registrar) -> Result<()> {
        r.patch(
            DescriptorPatch::new("base.item.edit")
                .field("extra", json_schema!({ "type": "string" })),
        );
        Ok(())
    }

    module!(STORELESS, "storeless", ["base"], storeless_init);

    #[test]
    fn a_patch_that_adds_a_field_without_store_is_rejected() {
        assert_eq!(
            problems(&[&BASE, &STORELESS]),
            ["module storeless: patch on base.item.edit adds the field extra without a store hook"]
        );
    }

    fn storing_init(r: &mut Registrar) -> Result<()> {
        r.patch(
            DescriptorPatch::new("base.item.edit")
                .field("sync", json_schema!({ "type": "boolean", "default": true }))
                .after("title")
                .store(|_, _| async { Ok(()) })
                .field("note", json_schema!({ "type": "string" }))
                .store(|_, _| async { Ok(()) }),
        );
        Ok(())
    }

    module!(STORING, "storing", ["base"], storing_init);

    #[test]
    fn a_patch_inserts_its_field_after_the_named_one_and_records_the_store_hook() {
        let frozen = freeze(&[&BASE, &STORING]).unwrap();
        let edit = frozen.registry().function("base.item.edit").unwrap();
        let properties: Vec<&String> = edit.descriptor().params.as_value()["properties"]
            .as_object()
            .unwrap()
            .keys()
            .collect();
        assert_eq!(
            properties,
            ["id", "title", "sync", "kind", "weight", "note"]
        );
        assert_eq!(
            edit.descriptor().params.as_value()["properties"]["sync"],
            json!({ "type": "boolean", "default": true })
        );
        let ui = edit.descriptor().ui.as_ref().unwrap();
        assert_eq!(ui.pages[0].fields, ["title", "sync", "kind"]);
        assert_eq!(ui.pages[1].fields, ["weight", "note"]);
        let patched: Vec<(&str, &str)> = edit
            .patched()
            .iter()
            .map(|field| (field.name.as_str(), field.module.as_str()))
            .collect();
        assert_eq!(patched, [("sync", "storing"), ("note", "storing")]);
        assert!(
            edit.def().params().as_value()["properties"]
                .get("sync")
                .is_none()
        );
        assert_eq!(
            format!("{:?}", edit.patched()[0]),
            "PatchedField { name: \"sync\", module: \"storing\" }"
        );
    }

    fn misplaced_init(r: &mut Registrar) -> Result<()> {
        r.patch(
            DescriptorPatch::new("base.item.edit")
                .field("title", json_schema!({ "type": "string" }))
                .store(|_, _| async { Ok(()) })
                .field("late", json_schema!({ "type": "string" }))
                .after("nowhere")
                .store(|_, _| async { Ok(()) }),
        )
        .patch(
            DescriptorPatch::new("base.item.list")
                .field("x", json_schema!({ "type": "string" }))
                .store(|_, _| async { Ok(()) }),
        )
        .patch(DescriptorPatch::new("base.item.drop").access(Level::Admin, None));
        Ok(())
    }

    module!(MISPLACED, "misplaced", ["base"], misplaced_init);

    #[test]
    fn a_patch_must_name_a_registered_function_a_new_field_and_an_existing_anchor() {
        assert_eq!(
            problems(&[&BASE, &MISPLACED]),
            [
                "module misplaced: patch on base.item.edit adds the field title, which the function already has",
                "module misplaced: patch on base.item.edit places late after nowhere, which the function does not have",
                "module misplaced: patch on base.item.list, whose parameters have no properties to add a field to",
                "module misplaced: patch on base.item.drop, which is not registered",
            ]
        );
    }

    fn hooked_init(r: &mut Registrar) -> Result<()> {
        r.before("base.item.edit", |_: Call<Value>| async {
            Ok(Before::Proceed)
        })
        .within("base.item.edit", |_: Call<Value>| async { Ok(()) })
        .before("base.item.vanish", |_: Call<Value>| async {
            Ok(Before::Proceed)
        })
        .within("base.item.vanish", |_: Call<Value>| async { Ok(()) });
        Ok(())
    }

    module!(HOOKED, "hooked", ["base"], hooked_init);

    #[test]
    fn hooks_attach_to_registered_functions_and_fail_on_unknown_ones() {
        assert_eq!(
            problems(&[&BASE, &HOOKED]),
            [
                "module hooked: before hook on base.item.vanish, which is not registered",
                "module hooked: within hook on base.item.vanish, which is not registered",
            ]
        );
    }

    fn well_hooked_init(r: &mut Registrar) -> Result<()> {
        r.before("base.item.edit", |_: Call<Value>| async {
            Ok(Before::Proceed)
        })
        .within("base.item.edit", |_: Call<Value>| async { Ok(()) });
        Ok(())
    }

    module!(WELL_HOOKED, "well_hooked", ["base"], well_hooked_init);

    #[test]
    fn attached_hooks_remember_the_module_that_registered_them() {
        let frozen = freeze(&[&BASE, &WELL_HOOKED]).unwrap();
        let edit = frozen.registry().function("base.item.edit").unwrap();
        assert_eq!(edit.before().len(), 1);
        assert_eq!(edit.before()[0].module, "well_hooked");
        assert_eq!(edit.within().len(), 1);
        assert_eq!(
            format!("{:?}", edit.within()[0]),
            "Hook { module: \"well_hooked\" }"
        );
    }

    fn dangling_init(r: &mut Registrar) -> Result<()> {
        r.function(form(
            "dangling.item.edit",
            Form::new("dangling.item.edit.title")
                .read("dangling.item.fetch")
                .page("main", ["title"])
                .validate("dangling.item.check")
                .field("title")
                .recompute("dangling.item.recompute"),
        ))
        .function(
            function::<ListQuery, Page<Item>>("dangling.item.list")
                .kind(Kind::List)
                .access(Level::User, "dangling.item.read")
                .unowned()
                .ui(List::new("dangling.item.list.title")
                    .column(Column::new("title", ColumnType::Link).to("dangling.item.open", []))
                    .column(Column::new("kind", ColumnType::Text).filter_remote("dangling.item.edit"))
                    .column(Column::new("weight", ColumnType::Number).edit("dangling.item.bump").permission("dangling.item.write"))
                    .action(Action::new("new", ActionRole::New).screen("dangling.item.create"))
                    .action(Action::new("drop", ActionRole::Delete).function("dangling.item.delete"))
                    .row_actions(["drop", "ghost"])
                    .empty("dangling.item.empty", Some("phantom")))
                .handler(|_: Call<ListQuery>| async { Ok(Page::new(Vec::new(), 0)) }),
        )
        .function(
            function::<Nothing, Nothing>("dangling.item.wrong")
                .kind(Kind::Form)
                .ui(Form::new("dangling.item.wrong.title").read("dangling.item.list"))
                .handler(|_: Call<Nothing>| async { Ok(Nothing {}) }),
        )
        .menu(MenuNode::new("dangling/items", "dangling.item.nowhere"))
        .i18n("en", r#"{ "dangling": { "item": { "edit": { "title": "E" }, "list": { "title": "L" }, "wrong": { "title": "W" }, "empty": "N" } } }"#);
        Ok(())
    }

    module!(DANGLING, "dangling", [], dangling_init);

    #[test]
    fn every_function_action_and_permission_a_descriptor_names_must_exist() {
        assert_eq!(
            problems(&[&DANGLING]),
            [
                "function dangling.item.edit: ui.read names dangling.item.fetch, which is not registered",
                "function dangling.item.edit: page main validate names dangling.item.check, which is not registered",
                "function dangling.item.edit: field title recompute names dangling.item.recompute, which is not registered",
                "function dangling.item.list: access.permission names the permission dangling.item.read, which no module registers",
                "function dangling.item.list: action new screen names dangling.item.create, which is not registered",
                "function dangling.item.list: action drop function names dangling.item.delete, which is not registered",
                "function dangling.item.list: column title to names dangling.item.open, which is not registered",
                "function dangling.item.list: column kind filter names dangling.item.edit, which is a form function, not options",
                "function dangling.item.list: column weight edit names dangling.item.bump, which is not registered",
                "function dangling.item.list: column weight access.permission names the permission dangling.item.write, which no module registers",
                "function dangling.item.list: ui.empty.action names the action phantom, which the screen does not have",
                "function dangling.item.list: ui.row_actions names the action ghost, which the screen does not have",
                "function dangling.item.wrong: ui.read names dangling.item.list, which is a list function, not get",
                "module dangling: menu node dangling/items points at dangling.item.nowhere, which is not registered",
            ]
        );
    }

    #[derive(Deserialize, Serialize, JsonSchema)]
    struct Attributed {
        #[schemars(extend("x-keel" = json!({ "options": "base.item.options" })))]
        account: Option<Id>,
        #[schemars(extend("x-keel" = json!({ "level": "boss" })))]
        broken: String,
        #[schemars(extend("x-ui" = json!({ "control": "knob" })))]
        knob: String,
        #[schemars(extend("x-keel" = json!({ "options": "base.item.get" })))]
        wrong: String,
    }

    fn attributed_init(r: &mut Registrar) -> Result<()> {
        r.function(
            function::<Attributed, Nothing>("attributed.item.edit")
                .kind(Kind::Form)
                .handler(|_: Call<Attributed>| async { Ok(Nothing {}) }),
        );
        Ok(())
    }

    module!(ATTRIBUTED, "attributed", ["base"], attributed_init);

    #[test]
    fn field_attributes_are_checked_for_membership_and_the_options_function_for_its_kind() {
        let listed = problems(&[&BASE, &ATTRIBUTED]);
        assert_eq!(listed.len(), 3, "{listed:?}");
        assert!(listed[0].starts_with("function attributed.item.edit: field broken carries an invalid x-keel attribute: unknown variant `boss`"), "{listed:?}");
        assert!(listed[1].starts_with("function attributed.item.edit: field knob carries an invalid x-ui attribute: unknown variant `knob`"), "{listed:?}");
        assert_eq!(
            listed[2],
            "function attributed.item.edit: field wrong x-keel.options names base.item.get, which is a get function, not options"
        );
    }

    fn clashing_init(r: &mut Registrar) -> Result<()> {
        r.menu(MenuNode::new("base/items", "base.item.list"))
            .menu(MenuNode::new("clashing/items", "base.item.list").label("clashing.menu"))
            .i18n(
                "en",
                r#"{ "menu": { "base": "Again" }, "clashing": { "other": "x" } }"#,
            );
        Ok(())
    }

    module!(CLASHING, "clashing", ["base"], clashing_init);

    #[test]
    fn menu_paths_and_dictionary_leaves_are_owned_by_one_module_and_labels_need_en() {
        assert_eq!(
            problems(&[&BASE, &CLASHING]),
            [
                "the menu path base/items is registered by both base and clashing",
                "the en dictionaries of base and clashing both define menu.base",
                "menu node clashing/items: the en dictionary has no entry for clashing.menu",
            ]
        );
    }

    fn contributing_init(r: &mut Registrar) -> Result<()> {
        r.contribute(Widget("contributing")).contribute(3u8);
        Ok(())
    }

    module!(CONTRIBUTING, "contributing", ["base"], contributing_init);

    #[test]
    fn contributions_of_every_module_land_in_one_typed_bag_in_graph_order() {
        let frozen = freeze(&[&CONTRIBUTING, &BASE]).unwrap();
        let widgets: Vec<&str> = frozen
            .registry()
            .contributions()
            .of::<Widget>()
            .map(|widget| widget.0)
            .collect();
        assert_eq!(widgets, ["base", "contributing"]);
        assert_eq!(frozen.registry().contributions().of::<u8>().count(), 1);
        assert_eq!(frozen.registry().contributions().len(), 3);
    }

    fn twice_init(r: &mut Registrar) -> Result<()> {
        r.function(get("twice.item.get"))
            .function(get("twice.item.get"));
        Ok(())
    }

    module!(TWICE, "twice", [], twice_init);

    fn failing_init(_: &mut Registrar) -> Result<()> {
        Err(KeelError::internal("failing.error.boom"))
    }

    module!(FAILING, "failing", [], failing_init);

    #[test]
    fn init_errors_and_invalid_registrations_name_the_module() {
        let error = Composition::new()
            .module(&FAILING)
            .resolve()
            .unwrap()
            .init()
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "module failing: init failed: internal: failing.error.boom"
        );
        let error = Composition::new()
            .module(&TWICE)
            .resolve()
            .unwrap()
            .init()
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "module twice: init registered invalid entries:\n  - module twice: function \"twice.item.get\" is registered twice"
        );
        let initialised = Composition::new()
            .module(&BASE)
            .resolve()
            .unwrap()
            .init()
            .unwrap();
        assert_eq!(initialised.graph().ids(), ["base"]);
        assert!(format!("{initialised:?}").contains("base"));
    }
}
