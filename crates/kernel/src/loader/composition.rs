use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use keel_sdk::system::LicenseKey;
use keel_sdk::{ModuleDef, ProductManifest};
use keel_types::Id;
use serde_json::Value;

use crate::clock::{Clock, SystemClock};
use crate::instance::InstanceKey;
use crate::keel;
use crate::keel::SYSTEM_KIND;
use crate::secret::Key;

pub const LEDGER_PREFIX: &str = "keel_migration_";
pub const API: u32 = keel_types::DESCRIPTOR_VERSION;

pub struct Composition {
    modules: Vec<&'static ModuleDef>,
    manifests: Vec<&'static ProductManifest>,
    enabled: Option<Vec<String>>,
    version: String,
    clock: Arc<dyn Clock>,
    key: Option<Key>,
    instance: Option<Id>,
    instance_key: Option<InstanceKey>,
    license_key: Option<Arc<dyn LicenseKey>>,
    sections: Sections,
    data_dir: PathBuf,
}

#[derive(Clone)]
pub struct Graph {
    modules: Vec<&'static ModuleDef>,
    manifests: Vec<&'static ProductManifest>,
    products: Vec<&'static ProductManifest>,
    version: String,
    clock: Arc<dyn Clock>,
    key: Option<Key>,
    instance: Option<Id>,
    instance_key: Option<InstanceKey>,
    license_key: Option<Arc<dyn LicenseKey>>,
    sections: Sections,
    data_dir: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sections {
    file: PathBuf,
    raw: BTreeMap<String, Value>,
}

impl Default for Sections {
    fn default() -> Self {
        Self {
            file: PathBuf::from(crate::config::FILE_NAME),
            raw: BTreeMap::new(),
        }
    }
}

impl Sections {
    pub fn new(file: impl Into<PathBuf>, raw: BTreeMap<String, Value>) -> Self {
        Self {
            file: file.into(),
            raw,
        }
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    pub fn raw(&self) -> &BTreeMap<String, Value> {
        &self.raw
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.raw.get(key)
    }
}

#[derive(Debug, thiserror::Error)]
pub struct CompositionError {
    problems: Vec<Problem>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Problem {
    #[error(
        "module {module} was compiled against keel-sdk {found}; this kernel is keel-sdk {expected}"
    )]
    SdkMismatch {
        module: String,
        found: String,
        expected: String,
    },
    #[error("module {id} is composed twice")]
    DuplicateId { id: String },
    #[error("module id {id:?} is not a valid identifier: {reason}")]
    Id { id: String, reason: &'static str },
    #[error(
        "module {module}{} requires {dependency}, which is not part of the composition",
        of_product(.product)
    )]
    MissingDependency {
        module: String,
        product: Option<String>,
        dependency: String,
    },
    #[error("modules depend on each other in a cycle: {}", .cycle.join(" -> "))]
    Cycle { cycle: Vec<String> },
    #[error("module {module} keeps its migrations under the ledger {found}; it must be {expected}")]
    Ledger {
        module: String,
        found: String,
        expected: String,
    },
    #[error("product {id} is composed twice")]
    DuplicateProduct { id: String },
    #[error("product {id} is enabled but not carried by this binary; it carries {}", carried(.known))]
    ProductUnknown { id: String, known: Vec<String> },
    #[error("product id {id:?} is not a valid identifier: {reason}")]
    ProductId { id: String, reason: &'static str },
    #[error("product {product} was built for api {found}; this kernel serves api {expected}")]
    ProductApi {
        product: String,
        found: u32,
        expected: u32,
    },
    #[error("product {product} declares the account kind {kind:?}: {reason}")]
    AccountKind {
        product: String,
        kind: String,
        reason: &'static str,
    },
}

impl Default for Composition {
    fn default() -> Self {
        Self {
            modules: Vec::new(),
            manifests: Vec::new(),
            enabled: None,
            version: env!("CARGO_PKG_VERSION").to_owned(),
            clock: Arc::new(SystemClock),
            key: None,
            instance: None,
            instance_key: None,
            license_key: None,
            sections: Sections::default(),
            data_dir: PathBuf::from(crate::config::DATA_DIR),
        }
    }
}

impl Composition {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn module(mut self, def: &'static ModuleDef) -> Self {
        self.modules.push(def);
        self
    }

    pub fn product(mut self, manifest: &'static ProductManifest) -> Self {
        self.manifests.push(manifest);
        self
    }

    pub fn enabled<I, S>(mut self, ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.enabled = Some(ids.into_iter().map(Into::into).collect());
        self
    }

    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    pub fn clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    pub fn key(mut self, key: Key) -> Self {
        self.key = Some(key);
        self
    }

    pub fn instance(mut self, id: Id) -> Self {
        self.instance = Some(id);
        self
    }

    pub fn instance_key(mut self, key: InstanceKey) -> Self {
        self.instance_key = Some(key);
        self
    }

    pub fn license_key(mut self, key: Arc<dyn LicenseKey>) -> Self {
        self.license_key = Some(key);
        self
    }

    pub fn sections(mut self, sections: Sections) -> Self {
        self.sections = sections;
        self
    }

    pub fn section(mut self, key: &str, value: Value) -> Self {
        self.sections.raw.insert(key.to_owned(), value);
        self
    }

    pub fn data_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.data_dir = path.into();
        self
    }

    pub fn resolve(self) -> Result<Graph, CompositionError> {
        let mut problems = Vec::new();
        let mut manifests: Vec<&'static ProductManifest> = Vec::new();
        for bundle in &self.manifests {
            if let Err(reason) = check_id(bundle.id) {
                problems.push(Problem::ProductId {
                    id: bundle.id.to_owned(),
                    reason,
                });
            }
            if bundle.api != API {
                problems.push(Problem::ProductApi {
                    product: bundle.id.to_owned(),
                    found: bundle.api,
                    expected: API,
                });
            }
            for kind in bundle.account_kinds {
                let reason = match check_id(kind) {
                    Err(reason) => Some(reason),
                    Ok(()) if *kind == SYSTEM_KIND => Some("it is reserved for the root account"),
                    Ok(()) => None,
                };
                if let Some(reason) = reason {
                    problems.push(Problem::AccountKind {
                        product: bundle.id.to_owned(),
                        kind: (*kind).to_owned(),
                        reason,
                    });
                }
            }
            if manifests.iter().any(|known| known.id == bundle.id) {
                problems.push(Problem::DuplicateProduct {
                    id: bundle.id.to_owned(),
                });
                continue;
            }
            manifests.push(bundle);
        }
        let products: Vec<&'static ProductManifest> = match &self.enabled {
            None => manifests.clone(),
            Some(enabled) => {
                for id in enabled {
                    if !manifests.iter().any(|manifest| manifest.id == id) {
                        problems.push(Problem::ProductUnknown {
                            id: id.clone(),
                            known: manifests
                                .iter()
                                .map(|manifest| manifest.id.to_owned())
                                .collect(),
                        });
                    }
                }
                manifests
                    .iter()
                    .copied()
                    .filter(|manifest| enabled.iter().any(|id| id == manifest.id))
                    .collect()
            }
        };
        let mut modules = self.modules.clone();
        let mut origin: BTreeMap<&'static str, &'static str> = BTreeMap::new();
        for bundle in &products {
            let sides = bundle
                .bridges
                .iter()
                .map(|bridge| bridge.side(products.iter().any(|product| product.id == bridge.to)));
            for module in bundle.modules.iter().copied().chain(sides) {
                if !modules.iter().any(|known| known.id == module.id) {
                    modules.push(module);
                    origin.insert(module.id, bundle.id);
                }
            }
        }
        let mut unique: BTreeMap<&'static str, &'static ModuleDef> =
            BTreeMap::from([(keel::MODULE.id, &keel::MODULE)]);
        for def in &modules {
            if def.sdk != keel_sdk::VERSION {
                problems.push(Problem::SdkMismatch {
                    module: def.id.to_owned(),
                    found: def.sdk.to_owned(),
                    expected: keel_sdk::VERSION.to_owned(),
                });
            }
            if let Err(reason) = check_id(def.id) {
                problems.push(Problem::Id {
                    id: def.id.to_owned(),
                    reason,
                });
            }
            if let Some(migrations) = def.migrations {
                let expected = format!("{LEDGER_PREFIX}{}", def.id);
                if migrations.ledger() != expected {
                    problems.push(Problem::Ledger {
                        module: def.id.to_owned(),
                        found: migrations.ledger().to_owned(),
                        expected,
                    });
                }
            }
            if unique.insert(def.id, def).is_some() {
                problems.push(Problem::DuplicateId {
                    id: def.id.to_owned(),
                });
            }
        }
        for def in unique.values() {
            for dependency in def.requires {
                if !unique.contains_key(dependency) {
                    problems.push(Problem::MissingDependency {
                        module: def.id.to_owned(),
                        product: origin.get(def.id).map(|product| (*product).to_owned()),
                        dependency: (*dependency).to_owned(),
                    });
                }
            }
        }
        let (ordered, stuck) = sort(&unique);
        for cycle in cycles(&unique, &stuck) {
            problems.push(Problem::Cycle { cycle });
        }
        if problems.is_empty() {
            Ok(Graph {
                modules: ordered,
                manifests,
                products,
                version: self.version,
                clock: self.clock,
                key: self.key,
                instance: self.instance,
                instance_key: self.instance_key,
                license_key: self.license_key,
                sections: self.sections,
                data_dir: self.data_dir,
            })
        } else {
            Err(CompositionError { problems })
        }
    }
}

fn carried(known: &[String]) -> String {
    if known.is_empty() {
        "no product".to_owned()
    } else {
        known.join(", ")
    }
}

fn of_product(product: &Option<String>) -> String {
    product
        .as_ref()
        .map(|product| format!(" of product {product}"))
        .unwrap_or_default()
}

pub(super) fn check_id(id: &str) -> Result<(), &'static str> {
    let mut chars = id.chars();
    match chars.next() {
        None => return Err("it is empty"),
        Some(first) if !first.is_ascii_lowercase() => {
            return Err("it must start with a lowercase ascii letter");
        }
        Some(_) => {}
    }
    if chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
        Ok(())
    } else {
        Err("only lowercase ascii letters, digits and underscores are allowed")
    }
}

fn sort(
    modules: &BTreeMap<&'static str, &'static ModuleDef>,
) -> (Vec<&'static ModuleDef>, BTreeSet<&'static str>) {
    let mut pending: BTreeMap<&'static str, usize> = modules
        .values()
        .map(|def| {
            let known = def
                .requires
                .iter()
                .filter(|dependency| modules.contains_key(*dependency))
                .count();
            (def.id, known)
        })
        .collect();
    let mut dependents: BTreeMap<&'static str, Vec<&'static str>> = BTreeMap::new();
    for def in modules.values() {
        for dependency in def.requires {
            if modules.contains_key(dependency) {
                dependents.entry(dependency).or_default().push(def.id);
            }
        }
    }
    let mut ready: BTreeSet<&'static str> = pending
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(id, _)| *id)
        .collect();
    let mut ordered = Vec::with_capacity(modules.len());
    while let Some(id) = next_ready(&mut ready) {
        pending.remove(id);
        ordered.push(modules[id]);
        for dependent in dependents.get(id).into_iter().flatten() {
            if let Some(count) = pending.get_mut(dependent) {
                *count -= 1;
                if *count == 0 {
                    ready.insert(dependent);
                }
            }
        }
    }
    (ordered, pending.into_keys().collect())
}

fn next_ready(ready: &mut BTreeSet<&'static str>) -> Option<&'static str> {
    if ready.remove(keel::MODULE.id) {
        return Some(keel::MODULE.id);
    }
    ready.pop_first()
}

fn cycles(
    modules: &BTreeMap<&'static str, &'static ModuleDef>,
    stuck: &BTreeSet<&'static str>,
) -> Vec<Vec<String>> {
    let mut found: BTreeSet<Vec<String>> = BTreeSet::new();
    for start in stuck {
        let mut path = vec![*start];
        let mut visited = BTreeSet::from([*start]);
        walk(modules, stuck, &mut path, &mut visited, &mut found);
    }
    found.into_iter().collect()
}

fn walk(
    modules: &BTreeMap<&'static str, &'static ModuleDef>,
    stuck: &BTreeSet<&'static str>,
    path: &mut Vec<&'static str>,
    visited: &mut BTreeSet<&'static str>,
    found: &mut BTreeSet<Vec<String>>,
) {
    let Some(current) = path.last().copied() else {
        return;
    };
    for next in modules[current].requires {
        if !stuck.contains(next) {
            continue;
        }
        if let Some(position) = path.iter().position(|id| id == next) {
            let mut cycle: Vec<String> =
                path[position..].iter().map(|id| (*id).to_owned()).collect();
            let smallest = cycle
                .iter()
                .enumerate()
                .min_by(|a, b| a.1.cmp(b.1))
                .map(|(index, _)| index)
                .unwrap_or_default();
            cycle.rotate_left(smallest);
            let first = cycle[0].clone();
            cycle.push(first);
            found.insert(cycle);
        } else if visited.insert(next) {
            path.push(next);
            walk(modules, stuck, path, visited, found);
            path.pop();
        }
    }
}

impl Graph {
    pub fn modules(&self) -> &[&'static ModuleDef] {
        &self.modules
    }

    pub fn ids(&self) -> Vec<&'static str> {
        self.modules.iter().map(|def| def.id).collect()
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

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn clock(&self) -> &Arc<dyn Clock> {
        &self.clock
    }

    pub fn key(&self) -> Option<&Key> {
        self.key.as_ref()
    }

    pub(crate) fn instance(&self) -> Option<Id> {
        self.instance
    }

    pub(crate) fn instance_key(&self) -> Option<&InstanceKey> {
        self.instance_key.as_ref()
    }

    pub(crate) fn license_key(&self) -> Option<&Arc<dyn LicenseKey>> {
        self.license_key.as_ref()
    }

    pub fn sections(&self) -> &Sections {
        &self.sections
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
}

impl fmt::Debug for Graph {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.ids()).finish()
    }
}

impl CompositionError {
    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }
}

impl fmt::Display for CompositionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the module composition is invalid:")?;
        for problem in &self.problems {
            write!(f, "\n  - {problem}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use keel_sdk::migrator::{self, Migrations};
    use keel_sdk::{Registrar, Result};

    use super::*;

    fn init(_: &mut Registrar) -> Result<()> {
        Ok(())
    }

    macro_rules! module {
        ($name:ident, $id:literal, [$($dep:literal),*]) => {
            static $name: ModuleDef = ModuleDef {
                id: $id,
                version: "0.1.0",
                sdk: keel_sdk::VERSION,
                requires: &[$($dep),*],
                migrations: None,
                init,
            };
        };
    }

    module!(AUTH, "auth", []);
    module!(TASKS, "tasks", ["auth"]);
    module!(DNS, "dns", ["auth", "tasks"]);
    module!(NOTIFY, "notify", ["auth"]);
    module!(RING_A, "ring_a", ["ring_b"]);
    module!(RING_B, "ring_b", ["ring_a"]);
    module!(LOOP_X, "loop_x", ["loop_y"]);
    module!(LOOP_Y, "loop_y", ["loop_z"]);
    module!(LOOP_Z, "loop_z", ["loop_x"]);
    module!(SELFISH, "selfish", ["selfish"]);
    module!(ORPHAN, "orphan", ["nobody"]);
    module!(BAD_ID, "Bad-Id", []);

    static OLD_SDK: ModuleDef = ModuleDef {
        id: "old",
        version: "0.1.0",
        sdk: "0.0.0-elsewhere",
        requires: &[],
        migrations: None,
        init,
    };

    static WRONG_LEDGER_MIGRATIONS: Migrations =
        migrator::embed!("$CARGO_MANIFEST_DIR/migrations").table("keel_migration_other");

    static WRONG_LEDGER: ModuleDef = ModuleDef {
        id: "ledger",
        version: "0.1.0",
        sdk: keel_sdk::VERSION,
        requires: &[],
        migrations: Some(&WRONG_LEDGER_MIGRATIONS),
        init,
    };

    fn problems(composition: Composition) -> Vec<String> {
        composition
            .resolve()
            .expect_err("the composition is invalid")
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn the_order_of_module_calls_means_nothing_and_dependencies_come_first() {
        let forward = Composition::new()
            .module(&AUTH)
            .module(&TASKS)
            .module(&DNS)
            .module(&NOTIFY)
            .resolve()
            .unwrap();
        let backward = Composition::new()
            .module(&NOTIFY)
            .module(&DNS)
            .module(&TASKS)
            .module(&AUTH)
            .resolve()
            .unwrap();
        assert_eq!(forward.ids(), ["keel", "auth", "notify", "tasks", "dns"]);
        assert_eq!(forward.ids(), backward.ids());
        assert_eq!(
            format!("{forward:?}"),
            "[\"keel\", \"auth\", \"notify\", \"tasks\", \"dns\"]"
        );
    }

    #[test]
    fn a_composition_without_a_data_dir_hands_modules_the_configured_default() {
        let graph = Composition::new().resolve().unwrap();
        assert_eq!(graph.data_dir(), Path::new(crate::config::DATA_DIR));
        assert_eq!(graph.data_dir(), Path::new("/var/lib/keel"));
        let placed = Composition::new()
            .data_dir("/tmp/keel-data")
            .resolve()
            .unwrap();
        assert_eq!(placed.data_dir(), Path::new("/tmp/keel-data"));
    }

    #[test]
    fn an_empty_composition_resolves_to_the_kernel_module_alone() {
        let graph = Composition::new().resolve().unwrap();
        assert_eq!(graph.ids(), ["keel"]);
        assert!(std::ptr::eq(graph.modules()[0], &keel::MODULE));
    }

    module!(KEEL_AGAIN, "keel", []);

    #[test]
    fn a_module_named_after_the_kernel_is_composed_twice() {
        let listed = problems(Composition::new().module(&KEEL_AGAIN));
        assert_eq!(listed, ["module keel is composed twice"]);
    }

    #[test]
    fn a_cycle_a_to_b_to_a_fails_with_a_message_naming_the_cycle() {
        let listed = problems(Composition::new().module(&RING_B).module(&RING_A));
        assert_eq!(
            listed,
            ["modules depend on each other in a cycle: ring_a -> ring_b -> ring_a"]
        );
    }

    #[test]
    fn every_cycle_is_listed_once_and_a_self_dependency_is_a_cycle_too() {
        let error = Composition::new()
            .module(&LOOP_Z)
            .module(&LOOP_Y)
            .module(&LOOP_X)
            .module(&SELFISH)
            .module(&RING_A)
            .module(&RING_B)
            .module(&AUTH)
            .resolve()
            .unwrap_err();
        assert_eq!(
            error.problems(),
            [
                Problem::Cycle {
                    cycle: vec![
                        "loop_x".into(),
                        "loop_y".into(),
                        "loop_z".into(),
                        "loop_x".into()
                    ]
                },
                Problem::Cycle {
                    cycle: vec!["ring_a".into(), "ring_b".into(), "ring_a".into()]
                },
                Problem::Cycle {
                    cycle: vec!["selfish".into(), "selfish".into()]
                },
            ]
        );
        assert_eq!(
            error.to_string(),
            "the module composition is invalid:\n  - modules depend on each other in a cycle: loop_x -> loop_y -> loop_z -> loop_x\n  - modules depend on each other in a cycle: ring_a -> ring_b -> ring_a\n  - modules depend on each other in a cycle: selfish -> selfish"
        );
    }

    #[test]
    fn a_missing_dependency_a_duplicate_id_and_an_sdk_mismatch_are_each_named() {
        let listed = problems(
            Composition::new()
                .module(&ORPHAN)
                .module(&AUTH)
                .module(&AUTH)
                .module(&OLD_SDK),
        );
        assert_eq!(
            listed,
            [
                "module auth is composed twice".to_owned(),
                format!(
                    "module old was compiled against keel-sdk 0.0.0-elsewhere; this kernel is keel-sdk {}",
                    keel_sdk::VERSION
                ),
                "module orphan requires nobody, which is not part of the composition".to_owned(),
            ]
        );
    }

    #[test]
    fn a_module_whose_dependency_is_missing_does_not_hide_the_modules_behind_it() {
        let error = Composition::new()
            .module(&ORPHAN)
            .module(&TASKS)
            .resolve()
            .unwrap_err();
        assert_eq!(
            error.problems(),
            [
                Problem::MissingDependency {
                    module: "orphan".into(),
                    product: None,
                    dependency: "nobody".into()
                },
                Problem::MissingDependency {
                    module: "tasks".into(),
                    product: None,
                    dependency: "auth".into()
                },
            ]
        );
    }

    #[test]
    fn a_ledger_not_named_after_the_module_fails_the_composition() {
        let listed = problems(Composition::new().module(&WRONG_LEDGER));
        assert_eq!(
            listed,
            [
                "module ledger keeps its migrations under the ledger keel_migration_other; it must be keel_migration_ledger"
            ]
        );
    }

    macro_rules! bundle {
        ($name:ident, $id:literal, $api:expr, [$($module:expr),*], [$($kind:literal),*]) => {
            static $name: ProductManifest = ProductManifest {
                id: $id,
                name: "product.x.brand",
                version: "0.1.0",
                api: $api,
                brand: keel_sdk::Brand {
                    logo: "product.x.logo",
                    hue: 0,
                    chroma: 0.0,
                },
                modules: &[$($module),*],
                bridges: &[],
                menu: &[],
                levels: keel_sdk::LevelLabels {
                    labels: &[],
                    disabled: &[],
                },
                account_kinds: &[$($kind),*],
                editions: &[],
                i18n: &[],
                assets: &[],
            };
        };
    }

    module!(HOSTING, "hosting", ["auth"]);
    module!(DNS_LOCAL, "dns_local", ["hosting", "dns"]);
    module!(DNS_PEER, "dns_peer", ["hosting"]);

    static HOSTING_BUNDLE: ProductManifest = ProductManifest {
        id: "panel",
        name: "product.x.brand",
        version: "0.1.0",
        api: API,
        brand: keel_sdk::Brand {
            logo: "product.x.logo",
            hue: 0,
            chroma: 0.0,
        },
        modules: &[&HOSTING, &AUTH],
        bridges: &[keel_sdk::Bridge {
            to: "dns",
            local: &DNS_LOCAL,
            remote: &DNS_PEER,
        }],
        menu: &[],
        levels: keel_sdk::LevelLabels {
            labels: &[],
            disabled: &[],
        },
        account_kinds: &["user"],
        editions: &[],
        i18n: &[],
        assets: &[],
    };

    bundle!(DNS_BUNDLE, "dns", API, [&DNS, &TASKS, &AUTH], ["user"]);
    bundle!(NOTIFY_BUNDLE, "notify", API, [&NOTIFY, &AUTH], []);
    bundle!(HALF_DNS_BUNDLE, "dns", API, [&DNS, &AUTH], []);
    bundle!(
        BROKEN_BUNDLE,
        "Broken-Id",
        API + 1,
        [&AUTH],
        ["system", "Bad Kind"]
    );

    #[test]
    fn a_bundle_composes_its_modules_once_in_the_order_of_the_products() {
        let graph = Composition::new()
            .version("9.9.9")
            .module(&AUTH)
            .product(&NOTIFY_BUNDLE)
            .product(&DNS_BUNDLE)
            .resolve()
            .unwrap();
        assert_eq!(graph.ids(), ["keel", "auth", "notify", "tasks", "dns"]);
        assert_eq!(graph.product_ids(), ["notify", "dns"]);
        assert_eq!(graph.version(), "9.9.9");
        assert!(std::ptr::eq(graph.products()[1], &DNS_BUNDLE));
        let bare = Composition::new().resolve().unwrap();
        assert!(bare.products().is_empty());
        assert_eq!(bare.version(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn a_bridge_loads_its_local_side_when_the_target_is_enabled_and_its_remote_side_otherwise() {
        let both = Composition::new()
            .product(&DNS_BUNDLE)
            .product(&HOSTING_BUNDLE)
            .resolve()
            .unwrap();
        assert!(both.ids().contains(&"dns_local"), "{:?}", both.ids());
        assert!(!both.ids().contains(&"dns_peer"), "{:?}", both.ids());
        for composition in [
            Composition::new().product(&HOSTING_BUNDLE),
            Composition::new()
                .product(&DNS_BUNDLE)
                .product(&HOSTING_BUNDLE)
                .enabled(["panel"]),
        ] {
            let graph = composition.resolve().unwrap();
            assert!(graph.ids().contains(&"dns_peer"), "{:?}", graph.ids());
            assert!(!graph.ids().contains(&"dns_local"), "{:?}", graph.ids());
            assert!(!graph.ids().contains(&"dns"), "{:?}", graph.ids());
        }
    }

    #[test]
    fn a_bundle_module_requiring_a_module_no_enabled_bundle_carries_names_the_product() {
        let error = Composition::new()
            .product(&NOTIFY_BUNDLE)
            .product(&HALF_DNS_BUNDLE)
            .resolve()
            .unwrap_err();
        assert_eq!(
            error.problems(),
            [Problem::MissingDependency {
                module: "dns".into(),
                product: Some("dns".into()),
                dependency: "tasks".into()
            }]
        );
        assert_eq!(
            error.to_string(),
            "the module composition is invalid:\n  - module dns of product dns requires tasks, which is not part of the composition"
        );
        let explicit = Composition::new()
            .module(&DNS)
            .product(&HALF_DNS_BUNDLE)
            .resolve()
            .unwrap_err();
        assert_eq!(
            explicit.problems(),
            [Problem::MissingDependency {
                module: "dns".into(),
                product: None,
                dependency: "tasks".into()
            }]
        );
    }

    #[test]
    fn a_bundle_is_checked_for_its_id_api_account_kinds_and_repeats() {
        let listed = problems(
            Composition::new()
                .product(&BROKEN_BUNDLE)
                .product(&DNS_BUNDLE)
                .product(&DNS_BUNDLE),
        );
        assert_eq!(
            listed,
            [
                "product id \"Broken-Id\" is not a valid identifier: it must start with a lowercase ascii letter".to_owned(),
                format!("product Broken-Id was built for api {}; this kernel serves api {API}", API + 1),
                "product Broken-Id declares the account kind \"system\": it is reserved for the root account".to_owned(),
                "product Broken-Id declares the account kind \"Bad Kind\": it must start with a lowercase ascii letter".to_owned(),
                "product dns is composed twice".to_owned(),
            ]
        );
    }

    #[test]
    fn a_module_id_that_cannot_prefix_tables_and_functions_is_refused() {
        let listed = problems(Composition::new().module(&BAD_ID));
        assert_eq!(
            listed,
            [
                "module id \"Bad-Id\" is not a valid identifier: it must start with a lowercase ascii letter"
            ]
        );
        assert_eq!(check_id(""), Err("it is empty"));
        assert_eq!(
            check_id("dns-zones"),
            Err("only lowercase ascii letters, digits and underscores are allowed")
        );
        assert_eq!(check_id("dns_zones2"), Ok(()));
    }
}
