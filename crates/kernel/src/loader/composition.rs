use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use keel_sdk::ModuleDef;

use crate::keel;

pub const LEDGER_PREFIX: &str = "keel_migration_";

#[derive(Default)]
pub struct Composition {
    modules: Vec<&'static ModuleDef>,
}

#[derive(Clone)]
pub struct Graph {
    modules: Vec<&'static ModuleDef>,
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
    #[error("module {module} requires {dependency}, which is not part of the composition")]
    MissingDependency { module: String, dependency: String },
    #[error("modules depend on each other in a cycle: {}", .cycle.join(" -> "))]
    Cycle { cycle: Vec<String> },
    #[error("module {module} keeps its migrations under the ledger {found}; it must be {expected}")]
    Ledger {
        module: String,
        found: String,
        expected: String,
    },
}

impl Composition {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn module(mut self, def: &'static ModuleDef) -> Self {
        self.modules.push(def);
        self
    }

    pub fn resolve(self) -> Result<Graph, CompositionError> {
        let mut problems = Vec::new();
        let mut unique: BTreeMap<&'static str, &'static ModuleDef> =
            BTreeMap::from([(keel::MODULE.id, &keel::MODULE)]);
        for def in &self.modules {
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
            Ok(Graph { modules: ordered })
        } else {
            Err(CompositionError { problems })
        }
    }
}

fn check_id(id: &str) -> Result<(), &'static str> {
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
                    dependency: "nobody".into()
                },
                Problem::MissingDependency {
                    module: "tasks".into(),
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
