use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::sync::Arc;

use keel_sdk::runtime::{
    Contributions, Guard, HookDef, Invoker, PermissionDef, ServiceKey, Services, StartHook,
    StopHook, UiError, ViewDef,
};
use keel_sdk::system::{
    AccountSource, AdvisoryLock, Backups, Caller, Features, Installation, LanguageSource,
    LicenseKey, LimitSource, LiveEvents, OutboxSubscriber, ParamMasker, PermissionCatalogue,
    PrincipalSource, SecretCipher, TaskRunner,
};
use keel_sdk::{
    BackupSource, ConfigSection, ConfigTemplate, DashboardWidget, FunctionDef, MenuNode,
    ProductManifest, SettingsSource,
};
use keel_types::descriptor::{Control, KeelAttrs, UiAttrs, X_KEEL, X_UI};
use keel_types::ui::{ColumnFilter, ScreenResult, ScreenUi};
use keel_types::{Access, Descriptor, Kind, Platform, WidgetKind};
use schemars::Schema;
use serde_json::Value;

use super::composition::Sections;
use super::composition::{API, Graph};
use super::i18n::{Catalogue, REQUIRED_LANGUAGE, leaves, placeholder_fault};
use super::icons::{ICONS_FILE, IconSet};
use super::init::Initialised;
use super::settings::DefaultSettings;
use super::{keys, patch, section};
use crate::backup;
use crate::clock::Clock;
use crate::dispatcher::guards::RegisteredPermissions;
use crate::dispatcher::strip::Plan;
use crate::dispatcher::{CallerHandle, LiveBus, MaskHandle, OutboxFeed};
use crate::doctor;
use crate::features::ProductFeatures;
use crate::keel::{SetupHandle, StoredAccounts, StoredLimits, StoredPrincipals};
use crate::menu::{SYSTEM_GROUP, group_of, label_of};
use crate::products::Composed;
use crate::registry::{Function, Group, Hook, ModuleInfo, Registry, View};
use crate::restart;
use crate::secret::DataKeyCipher;
use crate::storage::SessionLocks;

const PLATFORM: &str = "the platform";

pub struct Frozen {
    pub(super) graph: Graph,
    pub(super) registry: Arc<Registry>,
    pub(super) lifecycle: Vec<Lifecycle>,
    pub(super) locks: Arc<SessionLocks>,
    pub(super) backups: Arc<backup::Handle>,
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
    #[error("the section {section} of {file} is claimed by both {first} and {second}")]
    SectionClaimedTwice {
        file: String,
        section: String,
        first: String,
        second: String,
    },
    #[error("module {module}: the backup source {name} does not carry the module prefix")]
    BackupSourceName { module: String, name: String },
    #[error("the backup source {name} is contributed by both {first} and {second}")]
    BackupSourceClaimedTwice {
        name: String,
        first: String,
        second: String,
    },
    #[error("the configuration template {id} is contributed by both {first} and {second}")]
    TemplateClaimedTwice {
        id: String,
        first: String,
        second: String,
    },
    #[error("module {module}: the section {section} of {file} does not fit at {path}: {reason}")]
    SectionInvalid {
        module: String,
        file: String,
        section: String,
        path: String,
        reason: String,
    },
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
    #[error("the view {view} is registered by both {first} and {second}")]
    ViewRegisteredTwice {
        view: String,
        first: String,
        second: String,
    },
    #[error(
        "module {module}: view {view} names the permission {permission}, which no module registers"
    )]
    ViewPermissionUnknown {
        module: String,
        view: String,
        permission: String,
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
        "module {module}: function {function} answers a task, and no module provides a task runner"
    )]
    TaskRunnerMissing { module: String, function: String },
    #[error(
        "module {module}: function {function} can become a job, whose stored parameters would hold the secret field {field}"
    )]
    TaskStoresSecret {
        module: String,
        function: String,
        field: String,
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
    #[error(
        "function {function}: field {field} has the file control without {X_KEEL}.file naming maxBytes and types"
    )]
    FileAttrsMissing { function: String, field: String },
    #[error(
        "function {function}: field {field} chooses its check by {sibling}, which is not a parameter beside it"
    )]
    CheckBySibling {
        function: String,
        field: String,
        sibling: String,
    },
    #[error("function {function}: the upload carries no form block to render its fields")]
    UploadWithoutForm { function: String },
    #[error("function {function}: ui.multi names {field}, which is {problem}")]
    Multi {
        function: String,
        field: String,
        problem: &'static str,
    },
    #[error("function {function}: ui.multi needs {missing} to apply a group edit row by row")]
    MultiWithout {
        function: String,
        missing: &'static str,
    },
    #[error("{ICONS_FILE} embedded in the kernel is not a JSON array of icon names: {reason}")]
    IconSet { reason: String },
    #[error(
        "function {function}: {member} names the icon {icon}, which is outside the phoenix icon set"
    )]
    Icon {
        function: String,
        member: String,
        icon: String,
    },
    #[error(
        "module {module}: menu node {path} names the icon {icon}, which is outside the phoenix icon set"
    )]
    MenuIcon {
        module: String,
        path: String,
        icon: String,
    },
    #[error(
        "{owner}: the menu group {group} names the icon {icon}, which is outside the phoenix icon set"
    )]
    GroupIcon {
        owner: String,
        group: String,
        icon: String,
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
    #[error("a {widget} dashboard widget points at {function}, which is {problem}")]
    WidgetFunction {
        widget: &'static str,
        function: String,
        problem: &'static str,
    },
    #[error("the menu path {path} is registered by both {first} and {second}")]
    MenuPathTwice {
        path: String,
        first: String,
        second: String,
    },
    #[error(
        "module {module}: menu node {path} names the group {group}, which no enabled product declares"
    )]
    MenuGroupUnknown {
        module: String,
        path: String,
        group: String,
    },
    #[error("the menu group {group} is declared by both {first} and {second}")]
    MenuGroupTwice {
        group: String,
        first: String,
        second: String,
    },
    #[error(
        "module {module}: menu node {path} has no label and {function} has no screen title to use instead"
    )]
    MenuLabel {
        module: String,
        path: String,
        function: String,
    },
    #[error("product {product}: the {lang} dictionary is not a JSON object: {reason}")]
    BundleDictionary {
        product: String,
        lang: String,
        reason: String,
    },
    #[error("product {product}: the asset {asset} is shipped twice")]
    AssetTwice { product: String, asset: String },
    #[error("{owner}: the {REQUIRED_LANGUAGE} dictionary has no entry for {key}")]
    Translation { owner: String, key: String },
    #[error(
        "{owner}: the {REQUIRED_LANGUAGE} dictionary has no entry for {key} nor for the shared {shared}"
    )]
    TranslationInherited {
        owner: String,
        key: String,
        shared: String,
    },
    #[error("the {lang} dictionaries of {first} and {second} both define {key}")]
    TranslationConflict {
        lang: String,
        key: String,
        first: String,
        second: String,
    },
    #[error(
        "{owner}: the {lang} entry {key} carries the placeholder {found:?}; placeholders are written {{name}}"
    )]
    Placeholder {
        owner: String,
        lang: String,
        key: String,
        found: String,
    },
    #[error("function {function}: {problem}")]
    Layout { function: String, problem: String },
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
    icons: IconSet,
    services: Services,
    providers: HashMap<ServiceKey, String>,
    invoker: Option<(String, Arc<dyn Invoker>)>,
    known: BTreeMap<String, Kind>,
    functions: BTreeMap<String, Function>,
    owners: BTreeMap<String, String>,
    views: BTreeMap<String, View>,
    permissions: BTreeMap<String, PermissionDef>,
    permission_owners: BTreeMap<String, String>,
    guards: Vec<Hook<Arc<dyn Guard>>>,
    menu: Vec<MenuNode>,
    menu_owners: BTreeMap<String, String>,
    groups: Vec<Group>,
    group_owners: BTreeMap<String, String>,
    catalogue: Catalogue,
    dictionary_owners: BTreeMap<(String, String), String>,
    contributions: Contributions,
    lifecycle: Vec<Lifecycle>,
    sections: Sections,
    claimed: BTreeMap<String, String>,
    source_owners: BTreeMap<String, String>,
    template_owners: BTreeMap<String, String>,
    locks: Arc<SessionLocks>,
    masks: Arc<MaskHandle>,
    backups: Arc<backup::Handle>,
}

impl Initialised {
    pub fn freeze(self) -> Result<Frozen, FreezeError> {
        let mut problems = Vec::new();
        let icons = IconSet::embedded().unwrap_or_else(|error| {
            problems.push(FreezeProblem::IconSet {
                reason: error.to_string(),
            });
            IconSet::any()
        });
        let mut freezer = Freezer {
            problems,
            icons,
            services: Services::new(),
            providers: HashMap::new(),
            invoker: None,
            known: BTreeMap::new(),
            functions: BTreeMap::new(),
            owners: BTreeMap::new(),
            views: BTreeMap::new(),
            permissions: BTreeMap::new(),
            permission_owners: BTreeMap::new(),
            guards: Vec::new(),
            menu: Vec::new(),
            menu_owners: BTreeMap::new(),
            groups: Vec::new(),
            group_owners: BTreeMap::new(),
            catalogue: Catalogue::default(),
            dictionary_owners: BTreeMap::new(),
            contributions: Contributions::default(),
            lifecycle: Vec::new(),
            sections: self.graph.sections().clone(),
            claimed: BTreeMap::new(),
            source_owners: BTreeMap::new(),
            template_owners: BTreeMap::new(),
            locks: Arc::new(SessionLocks::new()),
            masks: Arc::new(MaskHandle::new()),
            backups: Arc::new(backup::Handle::new()),
        };
        let mut requirements = Vec::new();
        let mut hooks = Vec::new();
        let mut patches = Vec::new();
        let mut views = Vec::new();
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
            views.push((module.clone(), registrations.views));
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
                freezer.dictionary(&format!("module {module}"), &bundle.lang, &bundle.messages);
            }
            for section in registrations.contributions.of::<ConfigSection>() {
                freezer.section(&module, section);
            }
            for source in registrations.contributions.of::<BackupSource>() {
                freezer.backup_source(&module, source);
            }
            for template in registrations.contributions.of::<ConfigTemplate>() {
                freezer.template(&module, template);
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
        for bundle in self.graph.products() {
            freezer.bundle(bundle);
        }
        freezer.platform(&self.graph);
        freezer.unclaimed();
        for (module, required) in requirements {
            freezer.require(&module, &required);
        }
        for (module, views) in views {
            for def in views {
                freezer.view(&module, def);
            }
        }
        for (module, before, within) in hooks {
            freezer.hooks(&module, before, within);
        }
        for (module, patches) in patches {
            for patch in patches {
                freezer.patch(&module, &patch);
            }
        }
        freezer.group(PLATFORM, None, &SYSTEM_GROUP);
        freezer.references();
        freezer.tasks();
        freezer.translations(self.graph.products());
        freezer.finish(self.graph)
    }
}

impl Freezer {
    fn platform(&mut self, graph: &Graph) {
        let mut services = Services::new();
        services.provide(Arc::new(Composed::new(graph.manifests())));
        services.provide(Arc::new(Platform {
            version: graph.version().to_owned(),
            api: API,
        }));
        services.provide::<dyn Clock>(graph.clock().clone());
        services.provide(Arc::new(Installation {
            instance: graph.instance(),
            products: graph.products().to_vec(),
        }));
        if let Some(key) = graph.license_key() {
            services.provide::<dyn LicenseKey>(key.clone());
        }
        services.provide::<dyn PrincipalSource>(Arc::new(StoredPrincipals));
        services.provide::<dyn AccountSource>(Arc::new(StoredAccounts));
        services.provide::<dyn LimitSource>(Arc::new(StoredLimits::new(&self.contributions)));
        services.provide::<dyn LanguageSource>(Arc::new(Languages(self.catalogue.languages())));
        services.provide::<dyn PermissionCatalogue>(Arc::new(RegisteredPermissions::new(
            self.permissions.clone(),
        )));
        if let Some(key) = graph.key() {
            services.provide::<dyn SecretCipher>(Arc::new(DataKeyCipher::new(key.clone())));
        }
        let caller = Arc::new(CallerHandle::new());
        services.provide::<dyn Caller>(caller.clone());
        services.provide::<CallerHandle>(caller);
        let live = Arc::new(LiveBus::new());
        services.provide::<dyn LiveEvents>(live.clone());
        services.provide::<LiveBus>(live);
        let outbox = Arc::new(OutboxFeed::new());
        services.provide::<dyn OutboxSubscriber>(outbox.clone());
        services.provide::<OutboxFeed>(outbox);
        services.provide::<dyn AdvisoryLock>(self.locks.clone());
        services.provide::<dyn ParamMasker>(self.masks.clone());
        services.provide::<dyn Backups>(self.backups.clone());
        services.provide::<backup::Handle>(self.backups.clone());
        services.provide(Arc::new(doctor::Handle::new()));
        services.provide(Arc::new(restart::Handle::new(graph.clock().now())));
        services.provide(Arc::new(SetupHandle::new()));
        if !self.services.contains(&ServiceKey::of::<dyn Features>()) {
            services.provide::<dyn Features>(Arc::new(ProductFeatures::new(graph.products())));
        }
        if !self
            .services
            .contains(&ServiceKey::of::<dyn SettingsSource>())
        {
            services
                .provide::<dyn SettingsSource>(Arc::new(DefaultSettings::new(&self.contributions)));
        }
        self.provide(PLATFORM, services);
    }

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

    fn section(&mut self, module: &str, section: &ConfigSection) {
        let file = self.sections.file().display().to_string();
        if let Some(first) = self.claimed.get(section.key()) {
            self.problems.push(FreezeProblem::SectionClaimedTwice {
                file,
                section: section.key().to_owned(),
                first: first.clone(),
                second: module.to_owned(),
            });
            return;
        }
        self.claimed
            .insert(section.key().to_owned(), module.to_owned());
        let parsed = match self.sections.get(section.key()) {
            Some(raw) => match section::load(section, raw) {
                Ok(parsed) => parsed,
                Err(fault) => {
                    self.problems.push(FreezeProblem::SectionInvalid {
                        module: module.to_owned(),
                        file,
                        section: section.key().to_owned(),
                        path: fault.path,
                        reason: fault.reason,
                    });
                    return;
                }
            },
            None => section.fallback(),
        };
        let mut services = Services::new();
        services.provide_erased(section.service(), parsed);
        self.provide(module, services);
    }

    fn template(&mut self, module: &str, template: &ConfigTemplate) {
        if let Some(first) = self.template_owners.get(template.id()) {
            self.problems.push(FreezeProblem::TemplateClaimedTwice {
                id: template.id().to_owned(),
                first: first.clone(),
                second: module.to_owned(),
            });
            return;
        }
        self.template_owners
            .insert(template.id().to_owned(), module.to_owned());
    }

    fn backup_source(&mut self, module: &str, source: &BackupSource) {
        if source.module() != module {
            self.problems.push(FreezeProblem::BackupSourceName {
                module: module.to_owned(),
                name: source.name().to_owned(),
            });
        }
        if let Some(first) = self.source_owners.get(source.name()) {
            self.problems.push(FreezeProblem::BackupSourceClaimedTwice {
                name: source.name().to_owned(),
                first: first.clone(),
                second: module.to_owned(),
            });
            return;
        }
        self.source_owners
            .insert(source.name().to_owned(), module.to_owned());
    }

    fn unclaimed(&self) {
        for key in self.sections.raw().keys() {
            if !self.claimed.contains_key(key) {
                tracing::warn!(
                    file = %self.sections.file().display(),
                    section = key,
                    "the section is claimed by no loaded module and is ignored"
                );
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

    fn view(&mut self, module: &str, def: ViewDef) {
        if let Some(first) = self.owners.get(&def.name) {
            self.problems.push(FreezeProblem::ViewRegisteredTwice {
                view: def.name,
                first: first.clone(),
                second: module.to_owned(),
            });
            return;
        }
        self.owners.insert(def.name.clone(), module.to_owned());
        if let Some(permission) = &def.access.permission
            && !self.permissions.contains_key(permission)
        {
            self.problems.push(FreezeProblem::ViewPermissionUnknown {
                module: module.to_owned(),
                view: def.name.clone(),
                permission: permission.clone(),
            });
        }
        self.views.insert(
            def.name.clone(),
            View {
                module: module.to_owned(),
                def,
            },
        );
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
        if let Some(icon) = node.icon_name()
            && !self.icons.admits(icon)
        {
            self.problems.push(FreezeProblem::MenuIcon {
                module: module.to_owned(),
                path: node.path().to_owned(),
                icon: icon.to_owned(),
            });
        }
        self.menu.push(node);
    }

    fn dictionary(&mut self, owner: &str, lang: &str, messages: &Value) {
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
                second: owner.to_owned(),
            });
        }
        if let Value::Object(messages) = messages {
            for (key, value) in leaves(messages) {
                if let Some(found) = value.as_str().and_then(placeholder_fault) {
                    self.problems.push(FreezeProblem::Placeholder {
                        owner: owner.to_owned(),
                        lang: lang.to_owned(),
                        key: key.clone(),
                        found,
                    });
                }
                self.dictionary_owners
                    .entry((lang.to_owned(), key))
                    .or_insert_with(|| owner.to_owned());
            }
        }
    }

    fn bundle(&mut self, bundle: &'static ProductManifest) {
        let owner = format!("product {}", bundle.id);
        for (lang, json) in bundle.i18n {
            match serde_json::from_str::<Value>(json) {
                Ok(messages) if messages.is_object() => self.dictionary(&owner, lang, &messages),
                Ok(_) => self.problems.push(FreezeProblem::BundleDictionary {
                    product: bundle.id.to_owned(),
                    lang: (*lang).to_owned(),
                    reason: "the document is not an object".to_owned(),
                }),
                Err(error) => self.problems.push(FreezeProblem::BundleDictionary {
                    product: bundle.id.to_owned(),
                    lang: (*lang).to_owned(),
                    reason: error.to_string(),
                }),
            }
        }
        let mut groups: Vec<&keel_sdk::MenuGroup> = bundle.menu.iter().collect();
        groups.sort_by_key(|group| group.order);
        for group in groups {
            self.group(&owner, Some(bundle.id), group);
        }
        let mut assets = BTreeSet::new();
        for asset in bundle.assets {
            if !assets.insert(asset.id) {
                self.problems.push(FreezeProblem::AssetTwice {
                    product: bundle.id.to_owned(),
                    asset: asset.id.to_owned(),
                });
            }
        }
    }

    fn group(&mut self, owner: &str, product: Option<&str>, group: &keel_sdk::MenuGroup) {
        if let Some(first) = self.group_owners.get(group.id) {
            self.problems.push(FreezeProblem::MenuGroupTwice {
                group: group.id.to_owned(),
                first: first.clone(),
                second: owner.to_owned(),
            });
            return;
        }
        self.group_owners
            .insert(group.id.to_owned(), owner.to_owned());
        if !self.icons.admits(group.icon) {
            self.problems.push(FreezeProblem::GroupIcon {
                owner: owner.to_owned(),
                group: group.id.to_owned(),
                icon: group.icon.to_owned(),
            });
        }
        self.groups.push(Group {
            id: group.id.to_owned(),
            label: group.label.to_owned(),
            icon: group.icon.to_owned(),
            product: product.map(str::to_owned),
        });
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
                icons: &self.icons,
                problems: &mut problems,
            };
            checker.descriptor(&function.descriptor);
        }
        let mut placed = BTreeSet::new();
        for widget in self.contributions.of::<DashboardWidget>() {
            let problem = match (self.known.get(widget.func()), widget.kind()) {
                (None, _) => Some("not registered"),
                (Some(_), _) if !placed.insert(widget.func()) => Some("already on the dashboard"),
                (Some(Kind::List), WidgetKind::List) | (Some(Kind::Get), WidgetKind::Stat) => None,
                (Some(_), WidgetKind::List) => Some("not a list function"),
                (Some(_), WidgetKind::Stat) => Some("not a get function"),
            };
            if let Some(problem) = problem {
                problems.push(FreezeProblem::WidgetFunction {
                    widget: match widget.kind() {
                        WidgetKind::List => "list",
                        WidgetKind::Stat => "stat",
                    },
                    function: widget.func().to_owned(),
                    problem,
                });
            }
        }
        for node in &self.menu {
            let module = self
                .menu_owners
                .get(node.path())
                .cloned()
                .unwrap_or_default();
            if !self.known.contains_key(node.func()) && !self.views.contains_key(node.func()) {
                problems.push(FreezeProblem::MenuFunction {
                    module: module.clone(),
                    path: node.path().to_owned(),
                    function: node.func().to_owned(),
                });
            }
            let group = group_of(node.path());
            if !self.group_owners.contains_key(group) {
                problems.push(FreezeProblem::MenuGroupUnknown {
                    module,
                    path: node.path().to_owned(),
                    group: group.to_owned(),
                });
            }
        }
        self.problems.extend(problems);
    }

    fn tasks(&mut self) {
        let runner = self.services.contains(&ServiceKey::of::<dyn TaskRunner>());
        for function in self.functions.values() {
            let ui = function.descriptor.ui.as_ref();
            let answers_task = ui.is_some_and(|ui| ui.result == Some(ScreenResult::Task));
            let group_only =
                function.def.kind() == Kind::Form && ui.is_some_and(|ui| !ui.multi.is_empty());
            let batches = function.def.limits().batch_threshold.is_some() && !group_only;
            if !answers_task && !batches {
                continue;
            }
            let module = function.module().to_owned();
            if answers_task && !runner {
                self.problems.push(FreezeProblem::TaskRunnerMissing {
                    module: module.clone(),
                    function: function.name().to_owned(),
                });
            }
            for field in Plan::of_params(&function.descriptor).secret_fields() {
                self.problems.push(FreezeProblem::TaskStoresSecret {
                    module: module.clone(),
                    function: function.name().to_owned(),
                    field,
                });
            }
        }
    }

    fn translations(&mut self, products: &[&'static ProductManifest]) {
        let mut required: Vec<(String, String, Option<String>)> = Vec::new();
        for function in self.functions.values() {
            let owner = format!("function {}", function.name());
            if let Some(ui) = &function.descriptor.ui {
                required.push((owner.clone(), ui.title.clone(), None));
                if let Some(empty) = &ui.empty {
                    required.push((owner.clone(), empty.title.clone(), None));
                }
            }
            let patched: BTreeMap<String, String> = function
                .patched
                .iter()
                .map(|field| (field.name.clone(), field.module.clone()))
                .collect();
            for derived in keys::of(&function.descriptor, &patched) {
                required.push((
                    format!("{owner} {}", derived.member),
                    derived.key,
                    derived.shared,
                ));
            }
        }
        for node in &self.menu {
            let owner = format!("menu node {}", node.path());
            let label = match self.functions.get(node.func()) {
                Some(function) => Some(label_of(node, function)),
                None if self.views.contains_key(node.func()) => Some(node.label_key()),
                None => None,
            };
            match label {
                Some(Some(label)) => required.push((owner, label.to_owned(), None)),
                Some(None) => self.problems.push(FreezeProblem::MenuLabel {
                    module: self
                        .menu_owners
                        .get(node.path())
                        .cloned()
                        .unwrap_or_default(),
                    path: node.path().to_owned(),
                    function: node.func().to_owned(),
                }),
                None => {}
            }
        }
        for group in &self.groups {
            let owner = match &group.product {
                Some(product) => format!("product {product} group {}", group.id),
                None => format!("{PLATFORM} group {}", group.id),
            };
            required.push((owner, group.label.clone(), None));
        }
        for bundle in products {
            required.push((
                format!("product {}", bundle.id),
                bundle.name.to_owned(),
                None,
            ));
            for (level, key) in bundle.levels.labels {
                required.push((
                    format!("product {} level {level}", bundle.id),
                    (*key).to_owned(),
                    None,
                ));
            }
        }
        for (owner, key, shared) in required {
            if self.catalogue.has(REQUIRED_LANGUAGE, &key) {
                continue;
            }
            match shared {
                Some(shared) if self.catalogue.has(REQUIRED_LANGUAGE, &shared) => {}
                Some(shared) => {
                    self.problems
                        .push(FreezeProblem::TranslationInherited { owner, key, shared })
                }
                None => self
                    .problems
                    .push(FreezeProblem::Translation { owner, key }),
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
        self.masks.fill(self.functions.values());
        let registry = Registry {
            platform: Platform {
                version: graph.version().to_owned(),
                api: API,
            },
            modules,
            manifests: graph.manifests().to_vec(),
            products: graph.products().to_vec(),
            functions: self.functions,
            views: self.views,
            groups: self.groups,
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
            products = ?graph.product_ids(),
            functions = registry.functions.len(),
            views = registry.views.len(),
            "registries frozen"
        );
        Ok(Frozen {
            graph,
            registry: Arc::new(registry),
            lifecycle: self.lifecycle,
            locks: self.locks,
            backups: self.backups,
        })
    }
}

struct Languages(Vec<String>);

impl LanguageSource for Languages {
    fn languages(&self) -> Vec<String> {
        self.0.clone()
    }
}

struct References<'a> {
    function: &'a str,
    known: &'a BTreeMap<String, Kind>,
    permissions: &'a BTreeMap<String, PermissionDef>,
    icons: &'a IconSet,
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
            self.multi(descriptor, ui);
            self.layout(descriptor.kind, ui);
        }
        if descriptor.kind == Kind::Upload && descriptor.ui.is_none() {
            self.problems.push(FreezeProblem::UploadWithoutForm {
                function: self.function.to_owned(),
            });
        }
    }

    fn multi(&mut self, descriptor: &Descriptor, ui: &ScreenUi) {
        if ui.multi.is_empty() {
            return;
        }
        let properties = descriptor
            .params
            .as_object()
            .and_then(|schema| schema.get("properties"))
            .and_then(Value::as_object);
        for field in &ui.multi {
            let property = properties.and_then(|properties| properties.get(field));
            let problem = match property {
                None => Some("not a parameter"),
                Some(_) if descriptor.key.as_deref() == Some(field.as_str()) => Some("the key"),
                Some(property) => {
                    let secret = Schema::try_from(property.clone())
                        .ok()
                        .and_then(|schema| KeelAttrs::of(&schema).ok().flatten())
                        .is_some_and(|attrs| attrs.secret);
                    secret.then_some("secret")
                }
            };
            if let Some(problem) = problem {
                self.problems.push(FreezeProblem::Multi {
                    function: self.function.to_owned(),
                    field: field.clone(),
                    problem,
                });
            }
        }
        if descriptor.key.is_none() {
            self.problems.push(FreezeProblem::MultiWithout {
                function: self.function.to_owned(),
                missing: "a key",
            });
        }
        if ui.read.is_none() {
            self.problems.push(FreezeProblem::MultiWithout {
                function: self.function.to_owned(),
                missing: "ui.read",
            });
        }
    }

    fn layout(&mut self, kind: Kind, ui: &ScreenUi) {
        let Some(layout) = ui.layout else {
            return;
        };
        let mut problems = Vec::new();
        if kind != Kind::List {
            problems.push(format!(
                "ui.layout {} needs a list function, not {}",
                layout.as_str(),
                self::kind(kind)
            ));
        }
        if let Some(parent) = &ui.parent
            && !ui.columns.iter().any(|column| column.key == *parent)
        {
            problems.push(format!(
                "ui.parent names {parent}, which is not a column of the list"
            ));
        }
        if let Some(detail) = &ui.detail {
            match self.known.get(&detail.screen) {
                None => self.function("ui.detail screen", &detail.screen, None),
                Some(Kind::Get | Kind::List) => {}
                Some(found) => problems.push(format!(
                    "ui.detail screen names {}, which is a {} function, not get or list",
                    detail.screen,
                    self::kind(*found)
                )),
            }
        }
        for problem in problems {
            self.problems.push(FreezeProblem::Layout {
                function: self.function.to_owned(),
                problem,
            });
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
            let mut file = false;
            match KeelAttrs::of(&schema) {
                Ok(Some(attrs)) => {
                    if let Some(by) = &attrs.check_by
                        && (by.field == *field
                            || !properties.is_some_and(|all| all.contains_key(&by.field)))
                    {
                        self.problems.push(FreezeProblem::CheckBySibling {
                            function: self.function.to_owned(),
                            field: field.clone(),
                            sibling: by.field.clone(),
                        });
                    }
                    if let Some(options) = &attrs.options {
                        self.function(
                            &format!("field {field} {X_KEEL}.options"),
                            options,
                            Some(Kind::Options),
                        );
                    }
                    file = attrs.file.is_some();
                }
                Ok(None) => {}
                Err(error) => self.attrs(field, X_KEEL, error),
            }
            match UiAttrs::of(&schema) {
                Ok(Some(attrs)) if attrs.control == Some(Control::File) && !file => {
                    self.problems.push(FreezeProblem::FileAttrsMissing {
                        function: self.function.to_owned(),
                        field: field.clone(),
                    });
                }
                Ok(_) => {}
                Err(error) => self.attrs(field, X_UI, error),
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
            if let Some(icon) = &action.icon {
                self.icon(&format!("action {}", action.id), icon);
            }
        }
        for column in &ui.columns {
            self.column("column", column);
        }
        for band in &ui.bands {
            let member = format!("band {} column", band.id);
            for column in &band.columns {
                self.column(&member, column);
            }
        }
        for chart in &ui.charts {
            self.function(
                &format!("chart {} series", chart.id),
                &chart.series,
                Some(Kind::Get),
            );
        }
        if let Some(action) = ui.empty.as_ref().and_then(|empty| empty.action.as_ref()) {
            self.action("ui.empty.action", action, &actions);
        }
        for action in &ui.row_actions {
            self.action("ui.row_actions", action, &actions);
        }
    }

    fn column(&mut self, member: &str, column: &keel_types::Column) {
        if let Some(edit) = &column.edit {
            self.function(
                &format!("{member} {} edit", column.key),
                &edit.function,
                None,
            );
        }
        if let Some(ColumnFilter::Remote { remote }) = &column.filter {
            self.function(
                &format!("{member} {} filter", column.key),
                remote,
                Some(Kind::Options),
            );
        }
        if let Some(link) = &column.to {
            self.function(&format!("{member} {} to", column.key), &link.screen, None);
        }
        if let Some(permission) = column
            .access
            .as_ref()
            .and_then(|access| access.permission.as_ref())
        {
            self.permission(
                &format!("{member} {} access.permission", column.key),
                permission,
            );
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

    fn icon(&mut self, member: &str, icon: &str) {
        if !self.icons.admits(icon) {
            self.problems.push(FreezeProblem::Icon {
                function: self.function.to_owned(),
                member: member.to_owned(),
                icon: icon.to_owned(),
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
    use keel_sdk::ui::{Band, Chart, Report};
    use keel_sdk::{DescriptorPatch, Registrar};
    use keel_types::{ChartType, ColumnType};
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
        "base": { "item": { "edit": { "title": "Item" }, "list": { "title": "Items" }, "empty": "Nothing",
                            "weight": "Weight", "page": { "more": "More" } } },
        "menu": { "base": "Base" }
    }"#;

    static BASE_BUNDLE: ProductManifest = ProductManifest {
        id: "base",
        name: "product.base.brand",
        version: "0.1.0",
        api: API,
        brand: Brand {
            logo: "product.base.logo",
            hue: 30,
            chroma: 0.1,
        },
        modules: &[&BASE],
        bridges: &[],
        menu: &[MenuGroup {
            id: "base",
            label: "menu.base",
            icon: "boxes",
            order: 10,
        }],
        levels: LevelLabels {
            labels: &[(Level::User, "product.base.level.user")],
            disabled: &[],
        },
        account_kinds: &["user"],
        editions: &[],
        i18n: &[
            (
                "en",
                r#"{ "product": { "base": { "brand": "Base", "level": { "user": "Customer" } } } }"#,
            ),
            ("ru", r#"{ "product": { "base": { "brand": "ru:Base" } } }"#),
        ],
        assets: &[],
    };

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
        .menu(MenuNode::new("base/items", "base.item.list").icon("boxes"))
        .i18n("en", EN)
        .provide::<dyn Clock>(Arc::new(Fixed))
        .contribute(Widget("base"))
        .invoker(Arc::new(Local));
        Ok(())
    }

    module!(BASE, "base", [], base_init);

    fn freeze(
        modules: &[&'static keel_sdk::ModuleDef],
        products: &[&'static ProductManifest],
    ) -> Result<Frozen, FreezeError> {
        let mut composition = Composition::new();
        for module in modules {
            composition = composition.module(module);
        }
        for bundle in products {
            composition = composition.product(bundle);
        }
        composition
            .resolve()
            .expect("the composition resolves")
            .init()
            .expect("init registers")
            .freeze()
    }

    fn problems(
        modules: &[&'static keel_sdk::ModuleDef],
        products: &[&'static ProductManifest],
    ) -> Vec<String> {
        freeze(modules, products)
            .expect_err("the freeze fails")
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn init_and_freeze_run_without_a_database_so_init_cannot_do_io() {
        let frozen = freeze(&[&BASE], &[&BASE_BUNDLE]).unwrap();
        let registry = frozen.registry();
        let kernel = crate::keel::FUNCTIONS.len();
        assert_eq!(registry.modules().len(), 2);
        assert_eq!(registry.modules()[0].id, "keel");
        assert_eq!(registry.modules()[1].id, "base");
        assert_eq!(registry.functions().count(), kernel + 4);
        assert!(registry.function("base.item.edit").is_some());
        assert!(registry.function("base.item.delete").is_none());
        assert!(registry.function("keel.account.list").is_some());
        assert_eq!(registry.menu().len(), 6);
        assert_eq!(registry.permissions().len(), 6);
        assert!(registry.permission("base.item.read").is_some());
        assert!(registry.permission("keel.account.manage").is_some());
        assert_eq!(registry.languages().collect::<Vec<_>>(), ["en", "ru"]);
        assert!(registry.i18n("en").is_some());
        assert!(registry.i18n("ru").is_some());
        assert!(registry.i18n("de").is_none());
        assert_eq!(registry.services().get::<dyn Clock>().unwrap().now(), 7);
        assert_eq!(registry.contributions().of::<Widget>().count(), 1);
        assert!(registry.invoker().is_some());
        assert_eq!(frozen.graph().ids(), ["keel", "base"]);
        assert_eq!(frozen.graph().product_ids(), ["base"]);
        assert_eq!(registry.product_ids(), ["base"]);
        assert_eq!(registry.platform().version, env!("CARGO_PKG_VERSION"));
        assert_eq!(registry.platform().api, API);
        assert_eq!(
            registry.services().get::<Platform>().as_deref(),
            Some(registry.platform())
        );
        let groups: Vec<(&str, Option<&str>)> = registry
            .groups()
            .iter()
            .map(|group| (group.id.as_str(), group.product.as_deref()))
            .collect();
        assert_eq!(groups, [("base", Some("base")), ("system", None)]);
        assert!(registry.i18n("en").unwrap()["product"]["base"]["brand"].is_string());
        assert!(registry.i18n("ru").unwrap()["product"]["base"]["brand"].is_string());
        assert!(format!("{frozen:?}").contains("Registry"));
        assert!(format!("{registry:?}").contains("\"base.item.edit\""));
    }

    static ORDERED_BUNDLE: ProductManifest = ProductManifest {
        id: "ordered",
        name: "product.ordered.brand",
        version: "0.1.0",
        api: API,
        brand: Brand {
            logo: "product.ordered.logo",
            hue: 90,
            chroma: 0.1,
        },
        modules: &[],
        bridges: &[],
        menu: &[
            MenuGroup {
                id: "late",
                label: "menu.late",
                icon: "clock",
                order: 50,
            },
            MenuGroup {
                id: "early",
                label: "menu.early",
                icon: "sun",
                order: 5,
            },
        ],
        levels: LevelLabels {
            labels: &[],
            disabled: &[],
        },
        account_kinds: &[],
        editions: &[],
        i18n: &[(
            "en",
            r#"{ "product": { "ordered": { "brand": "Ordered" } }, "menu": { "late": "Late", "early": "Early" } }"#,
        )],
        assets: &[],
    };

    #[test]
    fn groups_follow_the_products_in_composition_order_and_their_own_order_with_the_platform_last()
    {
        let frozen = freeze(&[], &[&ORDERED_BUNDLE, &BASE_BUNDLE]).unwrap();
        let ids: Vec<&str> = frozen
            .registry()
            .groups()
            .iter()
            .map(|group| group.id.as_str())
            .collect();
        assert_eq!(ids, ["early", "late", "base", "system"]);
        assert_eq!(frozen.registry().product_ids(), ["ordered", "base"]);
        assert_eq!(frozen.graph().ids(), ["keel", "base"]);
    }

    static SYSTEM_AGAIN: ProductManifest = ProductManifest {
        id: "again",
        name: "product.again.brand",
        version: "0.1.0",
        api: API,
        brand: Brand {
            logo: "product.again.logo",
            hue: 0,
            chroma: 0.0,
        },
        modules: &[],
        bridges: &[],
        menu: &[
            MenuGroup {
                id: "system",
                label: "menu.system",
                icon: "cog",
                order: 1,
            },
            MenuGroup {
                id: "base",
                label: "menu.base",
                icon: "boxes",
                order: 2,
            },
        ],
        levels: LevelLabels {
            labels: &[(Level::Reseller, "product.again.level.reseller")],
            disabled: &[],
        },
        account_kinds: &[],
        editions: &[],
        i18n: &[(
            "ru",
            r#"{ "product": { "again": { "brand": "ru:Again" } } }"#,
        )],
        assets: &[],
    };

    #[test]
    fn a_group_declared_twice_and_bundle_keys_without_en_fail_freeze_in_one_error() {
        assert_eq!(
            problems(&[], &[&BASE_BUNDLE, &SYSTEM_AGAIN]),
            [
                "the menu group base is declared by both product base and product again",
                "the menu group system is declared by both product again and the platform",
                "product again: the en dictionary has no entry for product.again.brand",
                "product again level reseller: the en dictionary has no entry for product.again.level.reseller",
            ]
        );
    }

    fn homeless_init(r: &mut Registrar) -> Result<()> {
        r.function(get("homeless.item.get"))
            .menu(MenuNode::new("nowhere/items", "homeless.item.get"))
            .menu(MenuNode::new("system/homeless", "homeless.item.get"));
        Ok(())
    }

    module!(HOMELESS, "homeless", [], homeless_init);

    #[test]
    fn a_menu_node_needs_a_declared_group_and_a_label_or_a_screen_title() {
        assert_eq!(
            problems(&[&HOMELESS], &[]),
            [
                "module homeless: menu node nowhere/items names the group nowhere, which no enabled product declares",
                "module homeless: menu node nowhere/items has no label and homeless.item.get has no screen title to use instead",
                "module homeless: menu node system/homeless has no label and homeless.item.get has no screen title to use instead",
            ]
        );
    }

    fn sparse_init(r: &mut Registrar) -> Result<()> {
        r.function(form(
            "sparse.item.edit",
            Form::new("sparse.item.edit.title").page("main", ["title", "kind", "weight"]),
        ))
        .function(
            function::<ListQuery, Page<Item>>("sparse.item.list")
                .kind(Kind::List)
                .unowned()
                .ui(List::new("sparse.item.list.title")
                    .column(Column::new("title", ColumnType::Text))
                    .column(
                        Column::new("kind", ColumnType::Status)
                            .map([("heavy", Tone::Warning)])
                            .filter_options(["heavy", "light"]),
                    )
                    .action(Action::new("edit", ActionRole::Edit).screen("sparse.item.edit"))
                    .action(Action::new("weigh", ActionRole::Button).function("sparse.item.edit")))
                .handler(|_: Call<ListQuery>| async { Ok(Page::new(Vec::new(), 0)) }),
        )
        .i18n(
            "en",
            r#"{ "sparse": { "item": { "edit": { "title": "Item" }, "list": { "title": "Items" }, "kind": { "heavy": "Heavy" } } } }"#,
        );
        Ok(())
    }

    module!(SPARSE, "sparse", [], sparse_init);

    #[test]
    fn derived_keys_fall_back_to_the_shared_keel_keys_and_name_both_when_missing() {
        assert_eq!(
            problems(&[&SPARSE], &[]),
            [
                "function sparse.item.edit field weight: the en dictionary has no entry for sparse.item.weight nor for the shared keel.weight",
                "function sparse.item.list action weigh: the en dictionary has no entry for sparse.item.action.weigh nor for the shared keel.action.weigh",
                "function sparse.item.list column kind value light: the en dictionary has no entry for sparse.item.kind.light nor for the shared keel.kind.light",
            ]
        );
    }

    fn shouting_init(r: &mut Registrar) -> Result<()> {
        r.i18n(
            "en",
            r#"{ "shouting": { "a": "Hello %s", "b": "Hello {{name}}", "c": "Hello {name}", "nested": { "d": "Item {0}" } } }"#,
        )
        .i18n("ru", r#"{ "shouting": { "a": "ru:Hello { name }" } }"#);
        Ok(())
    }

    module!(SHOUTING, "shouting", [], shouting_init);

    static SHOUTING_BUNDLE: ProductManifest = ProductManifest {
        id: "shout",
        name: "product.shout.brand",
        version: "0.1.0",
        api: API,
        brand: Brand {
            logo: "product.shout.logo",
            hue: 0,
            chroma: 0.0,
        },
        modules: &[&SHOUTING],
        bridges: &[],
        menu: &[],
        levels: LevelLabels {
            labels: &[],
            disabled: &[],
        },
        account_kinds: &[],
        editions: &[],
        i18n: &[
            (
                "en",
                r#"{ "product": { "shout": { "brand": "%d shouts" } } }"#,
            ),
            ("ru", r#"[1, 2]"#),
            ("de", r#"{ not json"#),
        ],
        assets: &[
            Asset {
                id: "product.shout.logo",
                content_type: "image/svg+xml",
                bytes: b"<svg/>",
            },
            Asset {
                id: "product.shout.logo",
                content_type: "image/png",
                bytes: b"",
            },
        ],
    };

    #[test]
    fn placeholders_outside_the_single_brace_form_and_a_twice_shipped_asset_fail_freeze() {
        assert_eq!(
            problems(&[], &[&SHOUTING_BUNDLE]),
            [
                "module shouting: the en entry shouting.a carries the placeholder \"%s\"; placeholders are written {name}",
                "module shouting: the en entry shouting.b carries the placeholder \"{{\"; placeholders are written {name}",
                "module shouting: the en entry shouting.nested.d carries the placeholder \"{0}\"; placeholders are written {name}",
                "module shouting: the ru entry shouting.a carries the placeholder \"{ name }\"; placeholders are written {name}",
                "product shout: the en entry product.shout.brand carries the placeholder \"%d\"; placeholders are written {name}",
                "product shout: the ru dictionary is not a JSON object: the document is not an object",
                "product shout: the de dictionary is not a JSON object: key must be a string at line 1 column 3",
                "product shout: the asset product.shout.logo is shipped twice",
            ]
        );
    }

    #[test]
    fn a_compiled_descriptor_carries_the_conditions_as_ast() {
        let frozen = freeze(&[&BASE], &[&BASE_BUNDLE]).unwrap();
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
        let listed = problems(&[&NEEDY], &[]);
        assert_eq!(listed.len(), 1);
        assert!(
            listed[0].starts_with("module needy requires the service "),
            "{listed:?}"
        );
        assert!(
            listed[0].ends_with("Clock, which no module provides"),
            "{listed:?}"
        );
        let frozen = freeze(&[&NEEDY, &BASE], &[&BASE_BUNDLE]).unwrap();
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
        let listed = problems(&[&BASE, &SECOND_CLOCK], &[&BASE_BUNDLE]);
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
        let error = freeze(&[&TYPO], &[]).unwrap_err();
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
            problems(&[&UNTITLED], &[]),
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
            problems(&[&BASE, &LOWERING], &[&BASE_BUNDLE]),
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
        let frozen = freeze(&[&BASE, &RAISING], &[&BASE_BUNDLE]).unwrap();
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

    fn sealing_init(r: &mut Registrar) -> Result<()> {
        r.patch(
            DescriptorPatch::new("base.item.edit")
                .field(
                    "token",
                    json_schema!({ "type": "string", "x-keel": { "secret": true } }),
                )
                .store(|_, _| async { Ok(()) }),
        )
        .i18n("en", r#"{ "sealing": { "item": { "token": "Token" } } }"#);
        Ok(())
    }

    module!(SEALING, "sealing", ["base"], sealing_init);

    #[test]
    fn the_platform_masks_the_parameters_of_a_function_including_a_patched_secret_field() {
        let frozen = freeze(&[&BASE, &SEALING], &[&BASE_BUNDLE]).unwrap();
        let masker = frozen
            .registry()
            .services()
            .get::<dyn ParamMasker>()
            .expect("the platform provides the masker");
        let mut params = json!({ "title": "x", "kind": "plain", "token": "hunter2" });
        masker.mask("base.item.edit", &mut params).unwrap();
        assert_eq!(
            params,
            json!({ "title": "x", "kind": "plain", "token": crate::dispatcher::MASK })
        );
        let mut absent = json!({ "title": "x", "token": null });
        masker.mask("base.item.edit", &mut absent).unwrap();
        assert_eq!(absent["token"], Value::Null);
        let mut untouched = json!({ "token": "kept" });
        let missing = masker
            .mask("base.item.nothing", &mut untouched)
            .unwrap_err();
        assert_eq!(missing.key(), "keel.error.function_not_found");
        assert_eq!(missing.params()["function"], json!("base.item.nothing"));
        assert_eq!(untouched["token"], json!("kept"));
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
            problems(&[&BASE, &STORELESS], &[&BASE_BUNDLE]),
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
        )
        .i18n(
            "en",
            r#"{ "storing": { "item": { "sync": "Synchronise", "note": "Note" } } }"#,
        );
        Ok(())
    }

    module!(STORING, "storing", ["base"], storing_init);

    #[test]
    fn a_patch_inserts_its_field_after_the_named_one_and_records_the_store_hook() {
        let frozen = freeze(&[&BASE, &STORING], &[&BASE_BUNDLE]).unwrap();
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
            problems(&[&BASE, &MISPLACED], &[&BASE_BUNDLE]),
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
            problems(&[&BASE, &HOOKED], &[&BASE_BUNDLE]),
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
        let frozen = freeze(&[&BASE, &WELL_HOOKED], &[&BASE_BUNDLE]).unwrap();
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
        .function(
            function::<Nothing, Nothing>("dangling.report.sums")
                .kind(Kind::Report)
                .unowned()
                .ui(Report::new("dangling.report.sums.title")
                    .band(Band::new("totals").column(Column::new("weight", ColumnType::Link).to("dangling.item.open", [])))
                    .chart(Chart::new("trend", ChartType::Line, "dangling.report.series"))
                    .chart(Chart::new("share", ChartType::Pie, "dangling.item.list")))
                .handler(|_: Call<Nothing>| async { Ok(Nothing {}) }),
        )
        .menu(MenuNode::new("system/dangling", "dangling.item.nowhere"))
        .i18n("en", r#"{ "dangling": { "item": { "edit": { "title": "E" }, "list": { "title": "L" }, "wrong": { "title": "W" }, "empty": "N",
                                          "action": { "new": "New", "drop": "Drop" }, "column": { "weight": "Weight" } },
                                "report": { "sums": { "title": "S" }, "band": { "totals": "T" }, "column": { "weight": "W" },
                                            "chart": { "trend": "Tr", "share": "Sh" } } } }"#);
        Ok(())
    }

    module!(DANGLING, "dangling", [], dangling_init);

    #[test]
    fn every_function_action_and_permission_a_descriptor_names_must_exist() {
        assert_eq!(
            problems(&[&DANGLING], &[]),
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
                "function dangling.report.sums: band totals column weight to names dangling.item.open, which is not registered",
                "function dangling.report.sums: chart trend series names dangling.report.series, which is not registered",
                "function dangling.report.sums: chart share series names dangling.item.list, which is a list function, not get",
                "module dangling: menu node system/dangling points at dangling.item.nowhere, which is not registered",
            ]
        );
    }

    fn layered_init(r: &mut Registrar) -> Result<()> {
        let list = |name: &str, ui: List| {
            function::<ListQuery, Page<Item>>(name)
                .kind(Kind::List)
                .unowned()
                .ui(ui)
                .handler(|_: Call<ListQuery>| async { Ok(Page::new(Vec::new(), 0)) })
        };
        r.function(list(
            "layered.item.list",
            List::new("layered.item.list.title")
                .column(Column::new("title", ColumnType::Text))
                .column(Column::new("parentId", ColumnType::Text))
                .tree("parentId"),
        ))
        .function(list(
            "layered.item.split",
            List::new("layered.item.split.title")
                .column(Column::new("title", ColumnType::Text))
                .master_detail("layered.item.list", [("parent", "$id")]),
        ))
        .function(list(
            "layered.item.orphan",
            List::new("layered.item.orphan.title")
                .column(Column::new("title", ColumnType::Text))
                .tree("parentId"),
        ))
        .function(list(
            "layered.item.stray",
            List::new("layered.item.stray.title")
                .column(Column::new("title", ColumnType::Text))
                .master_detail("layered.item.save", []),
        ))
        .function(list(
            "layered.item.lost",
            List::new("layered.item.lost.title")
                .column(Column::new("title", ColumnType::Text))
                .master_detail("layered.item.nowhere", []),
        ))
        .function(
            function::<Nothing, Nothing>("layered.item.save")
                .kind(Kind::Form)
                .unowned()
                .ui(List::new("layered.item.save.title")
                    .column(Column::new("title", ColumnType::Text))
                    .tree("title"))
                .handler(|_: Call<Nothing>| async { Ok(Nothing {}) }),
        )
        .i18n(
            "en",
            r#"{ "layered": { "item": { "list": { "title": "L" }, "split": { "title": "S" }, "orphan": { "title": "O" },
                                          "stray": { "title": "St" }, "lost": { "title": "Lo" }, "save": { "title": "Sa" },
                                          "column": { "title": "T", "parentId": "P" } } } }"#,
        );
        Ok(())
    }

    module!(LAYERED, "layered", [], layered_init);

    #[test]
    fn a_layout_names_a_parent_column_or_a_detail_screen_of_a_list() {
        assert_eq!(
            problems(&[&LAYERED], &[]),
            [
                "function layered.item.lost: ui.detail screen names layered.item.nowhere, which is not registered",
                "function layered.item.orphan: ui.parent names parentId, which is not a column of the list",
                "function layered.item.save: ui.layout tree needs a list function, not form",
                "function layered.item.stray: ui.detail screen names layered.item.save, which is a form function, not get or list",
            ]
        );
    }

    fn iconic_init(r: &mut Registrar) -> Result<()> {
        r.function(
            function::<ListQuery, Page<Item>>("iconic.item.list")
                .kind(Kind::List)
                .unowned()
                .ui(List::new("iconic.item.list.title")
                    .column(Column::new("title", ColumnType::Text))
                    .action(
                        Action::new("new", ActionRole::New)
                            .screen("iconic.item.list")
                            .icon("plus"),
                    )
                    .action(
                        Action::new("weigh", ActionRole::Button)
                            .function("iconic.item.list")
                            .icon("scale-balanced"),
                    ))
                .handler(|_: Call<ListQuery>| async { Ok(Page::new(Vec::new(), 0)) }),
        )
        .menu(MenuNode::new("iconic/items", "iconic.item.list").icon("notebook"))
        .menu(
            MenuNode::new("iconic/tags", "iconic.item.list")
                .label("iconic.tags")
                .icon("tag"),
        )
        .i18n(
            "en",
            r#"{ "iconic": { "item": { "list": { "title": "Items" }, "action": { "new": "New", "weigh": "Weigh" } }, "tags": "Tags" } }"#,
        );
        Ok(())
    }

    module!(ICONIC, "iconic", [], iconic_init);

    static ICONIC_BUNDLE: ProductManifest = ProductManifest {
        id: "iconic",
        name: "product.iconic.brand",
        version: "0.1.0",
        api: API,
        brand: Brand {
            logo: "product.iconic.logo",
            hue: 200,
            chroma: 0.1,
        },
        modules: &[&ICONIC],
        bridges: &[],
        menu: &[MenuGroup {
            id: "iconic",
            label: "menu.iconic",
            icon: "flask",
            order: 10,
        }],
        levels: LevelLabels {
            labels: &[],
            disabled: &[],
        },
        account_kinds: &[],
        editions: &[],
        i18n: &[(
            "en",
            r#"{ "product": { "iconic": { "brand": "Iconic" } }, "menu": { "iconic": "Iconic" } }"#,
        )],
        assets: &[],
    };

    #[test]
    fn an_icon_outside_the_phoenix_set_in_a_descriptor_or_a_menu_node_fails_freeze() {
        assert_eq!(
            problems(&[], &[&ICONIC_BUNDLE]),
            [
                "module iconic: menu node iconic/items names the icon notebook, which is outside the phoenix icon set",
                "product iconic: the menu group iconic names the icon flask, which is outside the phoenix icon set",
                "function iconic.item.list: action weigh names the icon scale-balanced, which is outside the phoenix icon set",
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
        #[schemars(extend("x-keel" = json!({ "checkBy": { "field": "knob", "cases": { "v4": "ipv4" } } })))]
        x_chosen: String,
        #[schemars(extend("x-keel" = json!({ "checkBy": { "field": "x_self", "cases": { "v4": "ipv4" } } })))]
        x_self: String,
        #[schemars(extend("x-keel" = json!({ "checkBy": { "field": "missing", "cases": { "v4": "ipv4" } } })))]
        x_stray: String,
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
        let listed = problems(&[&BASE, &ATTRIBUTED], &[&BASE_BUNDLE]);
        assert_eq!(listed.len(), 5, "{listed:?}");
        assert!(listed[0].starts_with("function attributed.item.edit: field broken carries an invalid x-keel attribute: unknown variant `boss`"), "{listed:?}");
        assert!(listed[1].starts_with("function attributed.item.edit: field knob carries an invalid x-ui attribute: unknown variant `knob`"), "{listed:?}");
        assert_eq!(
            listed[2..],
            [
                "function attributed.item.edit: field wrong x-keel.options names base.item.get, which is a get function, not options",
                "function attributed.item.edit: field x_self chooses its check by x_self, which is not a parameter beside it",
                "function attributed.item.edit: field x_stray chooses its check by missing, which is not a parameter beside it",
            ]
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
            problems(&[&BASE, &CLASHING], &[&BASE_BUNDLE]),
            [
                "the menu path base/items is registered by both base and clashing",
                "the en dictionaries of module base and module clashing both define menu.base",
                "module clashing: menu node clashing/items names the group clashing, which no enabled product declares",
                "menu node clashing/items: the en dictionary has no entry for clashing.menu",
            ]
        );
    }

    fn viewing_init(r: &mut Registrar) -> Result<()> {
        r.view(
            "viewing.map",
            ViewSpec::new(Level::Reseller)
                .permission("base.item.read")
                .menu(
                    MenuNode::new("base/map", "viewing.map")
                        .label("viewing.map.title")
                        .icon("map"),
                )
                .needs(["base.maps"]),
        )
        .view("viewing.health", ViewSpec::new(Level::Admin))
        .i18n("en", r#"{ "viewing": { "map": { "title": "Map" } } }"#);
        Ok(())
    }

    module!(VIEWING, "viewing", ["base"], viewing_init);

    #[test]
    fn a_view_is_frozen_with_its_access_and_features_and_its_menu_node_beside_the_functions() {
        let frozen = freeze(&[&BASE, &VIEWING], &[&BASE_BUNDLE]).unwrap();
        let registry = frozen.registry();
        let names: Vec<&str> = registry.views().map(View::name).collect();
        assert_eq!(names, ["viewing.health", "viewing.map"]);
        let map = registry.view("viewing.map").unwrap();
        assert_eq!(map.module(), "viewing");
        assert_eq!(map.access().level, Level::Reseller);
        assert_eq!(map.access().permission.as_deref(), Some("base.item.read"));
        assert_eq!(map.needs(), ["base.maps"]);
        assert!(registry.function("viewing.map").is_none());
        let node = registry
            .menu()
            .iter()
            .find(|node| node.path() == "base/map")
            .unwrap();
        assert_eq!(node.func(), "viewing.map");
        assert!(format!("{registry:?}").contains("\"viewing.map\""));
    }

    fn misviewing_init(r: &mut Registrar) -> Result<()> {
        r.view(
            "misviewing.map",
            ViewSpec::new(Level::User)
                .permission("misviewing.map.open")
                .menu(MenuNode::new("nowhere/map", "misviewing.map")),
        );
        Ok(())
    }

    module!(MISVIEWING, "misviewing", [], misviewing_init);

    #[test]
    fn a_view_needs_a_registered_permission_a_declared_group_and_a_labelled_menu_node() {
        assert_eq!(
            problems(&[&MISVIEWING], &[]),
            [
                "module misviewing: view misviewing.map names the permission misviewing.map.open, which no module registers",
                "module misviewing: menu node nowhere/map names the group nowhere, which no enabled product declares",
                "module misviewing: menu node nowhere/map has no label and misviewing.map has no screen title to use instead",
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
        let frozen = freeze(&[&CONTRIBUTING, &BASE], &[&BASE_BUNDLE]).unwrap();
        let widgets: Vec<&str> = frozen
            .registry()
            .contributions()
            .of::<Widget>()
            .map(|widget| widget.0)
            .collect();
        assert_eq!(widgets, ["base", "contributing"]);
        assert_eq!(frozen.registry().contributions().of::<u8>().count(), 1);
        assert_eq!(
            frozen
                .registry()
                .contributions()
                .of::<keel_sdk::StatCollector>()
                .count(),
            2
        );
        assert_eq!(
            frozen
                .registry()
                .contributions()
                .of::<keel_sdk::DashboardWidget>()
                .count(),
            1
        );
        assert_eq!(frozen.registry().contributions().len(), 6);
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
        assert_eq!(initialised.graph().ids(), ["keel", "base"]);
        assert!(format!("{initialised:?}").contains("base"));
    }
}

#[cfg(test)]
mod section_tests {
    use std::io;
    use std::sync::{Arc, Mutex, PoisonError};

    use keel_sdk::prelude::*;
    use keel_sdk::system::Backups;
    use keel_sdk::{ConfigSection, Registrar};
    use tracing_subscriber::fmt::MakeWriter;

    use crate::loader::Composition;
    use crate::loader::Sections;
    use crate::loader::testing::module;

    #[derive(Clone, Debug, Default, Deserialize, JsonSchema, PartialEq)]
    #[serde(default, deny_unknown_fields)]
    struct Host {
        socket: String,
    }

    fn host_init(r: &mut Registrar) -> Result<()> {
        r.contribute(ConfigSection::new::<Host>("host"));
        Ok(())
    }

    fn host_again_init(r: &mut Registrar) -> Result<()> {
        r.contribute(ConfigSection::new::<Host>("host"));
        Ok(())
    }

    module!(HOST, "host", [], host_init);
    module!(HOST_AGAIN, "host_again", [], host_again_init);

    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Captured {
        type Writer = Captured;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[test]
    fn a_claimed_section_is_parsed_into_the_services_and_an_unclaimed_key_is_logged() {
        let captured = Captured::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(captured.clone())
            .with_ansi(false)
            .finish();
        let frozen = tracing::subscriber::with_default(subscriber, || {
            Composition::new()
                .module(&HOST)
                .sections(Sections::new(
                    "/etc/keel/config.yaml",
                    [
                        ("host".to_owned(), json!({ "socket": "/tmp/agent.sock" })),
                        ("plugins".to_owned(), json!({ "dir": "/tmp" })),
                    ]
                    .into_iter()
                    .collect(),
                ))
                .resolve()
                .expect("the composition resolves")
                .init()
                .expect("init registers")
                .freeze()
                .expect("the sections freeze")
        });
        let host = frozen
            .registry()
            .services()
            .get::<Host>()
            .expect("the section is a service");
        assert_eq!(host.socket, "/tmp/agent.sock");
        let logged = String::from_utf8(
            captured
                .0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        )
        .expect("utf-8 logs");
        assert!(logged.contains("WARN"), "{logged}");
        assert!(logged.contains("section=\"plugins\""), "{logged}");
        assert!(logged.contains("/etc/keel/config.yaml"), "{logged}");
        assert!(!logged.contains("section=\"host\""), "{logged}");

        let absent = Composition::new()
            .module(&HOST)
            .resolve()
            .expect("the composition resolves")
            .init()
            .expect("init registers")
            .freeze()
            .expect("an absent section is the default");
        assert_eq!(
            *absent.registry().services().get::<Host>().expect("a Host"),
            Host::default()
        );
    }

    #[test]
    fn a_section_claimed_twice_and_one_of_the_wrong_shape_are_freeze_problems() {
        let twice = Composition::new()
            .module(&HOST)
            .module(&HOST_AGAIN)
            .resolve()
            .expect("the composition resolves")
            .init()
            .expect("init registers")
            .freeze()
            .expect_err("two claims fail");
        let listed: Vec<String> = twice.problems().iter().map(ToString::to_string).collect();
        assert_eq!(
            listed,
            ["the section host of config.yaml is claimed by both host and host_again"]
        );
        let wrong = Composition::new()
            .module(&HOST)
            .section("host", json!({ "socket": 7 }))
            .resolve()
            .expect("the composition resolves")
            .init()
            .expect("init registers")
            .freeze()
            .expect_err("a wrong shape fails");
        let listed: Vec<String> = wrong.problems().iter().map(ToString::to_string).collect();
        assert_eq!(listed.len(), 1);
        assert!(
            listed[0].starts_with(
                "module host: the section host of config.yaml does not fit at host.socket: "
            ),
            "{}",
            listed[0]
        );
    }

    fn zones_init(r: &mut Registrar) -> Result<()> {
        r.contribute(BackupSource::new("zones.files", |_| async { Ok(()) }));
        Ok(())
    }

    fn zones_again_init(r: &mut Registrar) -> Result<()> {
        r.contribute(BackupSource::new("zones.files", |_| async { Ok(()) }))
            .contribute(BackupSource::new("other.files", |_| async { Ok(()) }));
        Ok(())
    }

    module!(ZONES, "zones", [], zones_init);
    module!(ZONES_AGAIN, "zones_again", [], zones_again_init);

    #[test]
    fn a_backup_source_carries_its_module_prefix_and_is_contributed_once() {
        let alone = Composition::new()
            .module(&ZONES)
            .resolve()
            .expect("the composition resolves")
            .init()
            .expect("init registers")
            .freeze()
            .expect("one source freezes");
        assert!(
            alone.registry().services().get::<dyn Backups>().is_some(),
            "the kernel provides the backups service"
        );
        assert_eq!(
            alone
                .registry()
                .contributions()
                .of::<BackupSource>()
                .map(BackupSource::name)
                .collect::<Vec<_>>(),
            ["zones.files"]
        );
        let twice = Composition::new()
            .module(&ZONES)
            .module(&ZONES_AGAIN)
            .resolve()
            .expect("the composition resolves")
            .init()
            .expect("init registers")
            .freeze()
            .expect_err("a foreign prefix and a second claim fail");
        let listed: Vec<String> = twice.problems().iter().map(ToString::to_string).collect();
        assert_eq!(
            listed,
            [
                "module zones_again: the backup source zones.files does not carry the module prefix",
                "the backup source zones.files is contributed by both zones and zones_again",
                "module zones_again: the backup source other.files does not carry the module prefix",
            ]
        );
    }
}
