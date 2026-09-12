use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use keel_sdk::runtime::{
    BeforeHook, Contributions, Invoker, PermissionDef, Services, StoreHook, WithinHook,
};
use keel_sdk::{FunctionDef, MenuNode};
use keel_types::Descriptor;
use serde_json::{Map, Value};

pub struct Registry {
    pub(crate) modules: Vec<ModuleInfo>,
    pub(crate) functions: BTreeMap<String, Function>,
    pub(crate) menu: Vec<MenuNode>,
    pub(crate) permissions: BTreeMap<String, PermissionDef>,
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

pub struct Function {
    pub(crate) def: FunctionDef,
    pub(crate) descriptor: Descriptor,
    pub(crate) before: Vec<Hook<BeforeHook>>,
    pub(crate) within: Vec<Hook<WithinHook>>,
    pub(crate) patched: Vec<PatchedField>,
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
    pub fn modules(&self) -> &[ModuleInfo] {
        &self.modules
    }

    pub fn functions(&self) -> impl Iterator<Item = &Function> {
        self.functions.values()
    }

    pub fn function(&self, name: &str) -> Option<&Function> {
        self.functions.get(name)
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

impl fmt::Debug for Registry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Registry")
            .field("modules", &self.modules)
            .field("functions", &self.functions.keys().collect::<Vec<_>>())
            .field("menu", &self.menu)
            .field("permissions", &self.permissions.keys().collect::<Vec<_>>())
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
