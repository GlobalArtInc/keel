use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use keel_sdk::runtime::{
    BeforeHook, Contributions, Guard, Invoker, PermissionDef, Services, StoreHook, ViewDef,
    WithinHook,
};
use keel_sdk::{FunctionDef, MenuNode, ProductManifest};
use keel_types::{Access, Descriptor, Platform};
use serde_json::{Map, Value};

pub struct Registry {
    pub(crate) platform: Platform,
    pub(crate) modules: Vec<ModuleInfo>,
    pub(crate) manifests: Vec<&'static ProductManifest>,
    pub(crate) products: Vec<&'static ProductManifest>,
    pub(crate) functions: BTreeMap<String, Function>,
    pub(crate) views: BTreeMap<String, View>,
    pub(crate) groups: Vec<Group>,
    pub(crate) menu: Vec<MenuNode>,
    pub(crate) permissions: BTreeMap<String, PermissionDef>,
    pub(crate) guards: Vec<Hook<Arc<dyn Guard>>>,
    pub(crate) i18n: BTreeMap<String, Map<String, Value>>,
    pub(crate) services: Arc<Services>,
    pub(crate) contributions: Arc<Contributions>,
    pub(crate) invoker: Option<Arc<dyn Invoker>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleInfo {
    pub id: String,
    pub version: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub id: String,
    pub label: String,
    pub icon: String,
    pub product: Option<String>,
}

pub struct Function {
    pub(crate) def: FunctionDef,
    pub(crate) descriptor: Descriptor,
    pub(crate) before: Vec<Hook<BeforeHook>>,
    pub(crate) external: Vec<Hook<BeforeHook>>,
    pub(crate) within: Vec<Hook<WithinHook>>,
    pub(crate) patched: Vec<PatchedField>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    pub(crate) module: String,
    pub(crate) def: ViewDef,
}

#[derive(Clone)]
pub struct Hook<H> {
    pub module: String,
    pub hook: H,
}

#[derive(Clone)]
pub struct PatchedField {
    pub name: String,
    pub module: String,
    pub store: StoreHook,
}

impl Registry {
    pub fn platform(&self) -> &Platform {
        &self.platform
    }

    pub fn modules(&self) -> &[ModuleInfo] {
        &self.modules
    }

    pub fn manifests(&self) -> &[&'static ProductManifest] {
        &self.manifests
    }

    pub fn products(&self) -> &[&'static ProductManifest] {
        &self.products
    }

    pub fn product_ids(&self) -> Vec<&'static str> {
        self.products.iter().map(|bundle| bundle.id).collect()
    }

    pub fn groups(&self) -> &[Group] {
        &self.groups
    }

    pub fn functions(&self) -> impl Iterator<Item = &Function> {
        self.functions.values()
    }

    pub fn function(&self, name: &str) -> Option<&Function> {
        self.functions.get(name)
    }

    pub fn views(&self) -> impl Iterator<Item = &View> {
        self.views.values()
    }

    pub fn view(&self, name: &str) -> Option<&View> {
        self.views.get(name)
    }

    pub fn menu(&self) -> &[MenuNode] {
        &self.menu
    }

    pub fn permissions(&self) -> &BTreeMap<String, PermissionDef> {
        &self.permissions
    }

    pub fn permission(&self, key: &str) -> Option<&PermissionDef> {
        self.permissions.get(key)
    }

    pub fn guards(&self) -> &[Hook<Arc<dyn Guard>>] {
        &self.guards
    }

    pub fn languages(&self) -> impl Iterator<Item = &str> {
        self.i18n.keys().map(String::as_str)
    }

    pub fn i18n(&self, lang: &str) -> Option<&Map<String, Value>> {
        self.i18n.get(lang)
    }

    pub fn services(&self) -> &Arc<Services> {
        &self.services
    }

    pub fn contributions(&self) -> &Arc<Contributions> {
        &self.contributions
    }

    pub fn invoker(&self) -> Option<&Arc<dyn Invoker>> {
        self.invoker.as_ref()
    }
}

impl Function {
    pub fn name(&self) -> &str {
        self.def.name()
    }

    pub fn module(&self) -> &str {
        self.def.module()
    }

    pub fn def(&self) -> &FunctionDef {
        &self.def
    }

    pub fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }

    pub fn before(&self) -> &[Hook<BeforeHook>] {
        &self.before
    }

    pub fn within(&self) -> &[Hook<WithinHook>] {
        &self.within
    }

    pub fn patched(&self) -> &[PatchedField] {
        &self.patched
    }
}

impl View {
    pub fn name(&self) -> &str {
        &self.def.name
    }

    pub fn module(&self) -> &str {
        &self.module
    }

    pub fn access(&self) -> &Access {
        &self.def.access
    }

    pub fn needs(&self) -> &[String] {
        &self.def.needs
    }
}

impl fmt::Debug for Registry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Registry")
            .field("platform", &self.platform)
            .field("modules", &self.modules)
            .field(
                "manifests",
                &self
                    .manifests
                    .iter()
                    .map(|manifest| manifest.id)
                    .collect::<Vec<_>>(),
            )
            .field("products", &self.product_ids())
            .field("functions", &self.functions.keys().collect::<Vec<_>>())
            .field("views", &self.views.keys().collect::<Vec<_>>())
            .field("groups", &self.groups)
            .field("menu", &self.menu)
            .field("permissions", &self.permissions.keys().collect::<Vec<_>>())
            .field("guards", &self.guards)
            .field("languages", &self.i18n.keys().collect::<Vec<_>>())
            .field("services", &self.services)
            .field("contributions", &self.contributions)
            .field("invoker", &self.invoker.is_some())
            .finish()
    }
}

impl fmt::Debug for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Function")
            .field("def", &self.def)
            .field("before", &self.before.len())
            .field("external", &self.external.len())
            .field("within", &self.within.len())
            .field("patched", &self.patched)
            .finish()
    }
}

impl<H> fmt::Debug for Hook<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hook")
            .field("module", &self.module)
            .finish()
    }
}

impl fmt::Debug for PatchedField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PatchedField")
            .field("name", &self.name)
            .field("module", &self.module)
            .finish()
    }
}
