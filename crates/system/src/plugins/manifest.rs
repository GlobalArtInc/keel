use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use keel_sdk::schemars::Schema;
use keel_sdk::serde::Deserialize;
use keel_sdk::serde_json::{self, Map, Value};
use keel_sdk::system::Landscape;
use keel_sdk::types::ui::ScreenUi;
use keel_sdk::types::{Access, Idempotency, Kind, Level, Mode, PROTOCOL, WidgetKind};
use semver::{Version, VersionReq};

use crate::cron::api::schedule_fault;

pub const FILE: &str = "keel-plugin.json";
const OLDEST_PROTOCOL: u32 = if PROTOCOL > 1 { PROTOCOL - 1 } else { 1 };
pub(super) const LEVELS: [Level; 3] = [Level::Service, Level::User, Level::Admin];
const CLOSED: [&str; 2] = ["keel", "auth"];

#[derive(Debug, Deserialize)]
#[serde(crate = "keel_sdk::serde", deny_unknown_fields)]
pub(super) struct Manifest {
    pub(super) protocol: u32,
    pub(super) id: String,
    pub(super) version: String,
    #[serde(default)]
    pub(super) requires: Requires,
    pub(super) run: Run,
    pub(super) level: Level,
    #[serde(default)]
    pub(super) storage: Option<Storage>,
    #[serde(default)]
    pub(super) rights: Rights,
    #[serde(default)]
    pub(super) functions: Vec<FunctionSpec>,
    #[serde(default)]
    pub(super) patches: Vec<Value>,
    #[serde(default)]
    pub(super) settings: Vec<SettingSpec>,
    #[serde(default)]
    pub(super) cron: Vec<CronSpec>,
    #[serde(default)]
    pub(super) menu: Vec<MenuSpec>,
    #[serde(default)]
    pub(super) problems: Vec<ProblemSpec>,
    #[serde(default)]
    pub(super) widgets: Vec<WidgetSpec>,
    #[serde(default)]
    pub(super) i18n: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(crate = "keel_sdk::serde", deny_unknown_fields)]
pub(super) struct Requires {
    #[serde(default)]
    pub(super) keel: Option<String>,
    #[serde(default)]
    pub(super) modules: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(crate = "keel_sdk::serde", deny_unknown_fields)]
pub(super) struct Run {
    pub(super) mode: RunMode,
    pub(super) cmd: Vec<String>,
    #[serde(default)]
    pub(super) restart: Option<Restart>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(crate = "keel_sdk::serde", rename_all = "lowercase")]
pub(super) enum RunMode {
    Daemon,
    Exec,
}

impl RunMode {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            RunMode::Daemon => "daemon",
            RunMode::Exec => "exec",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(crate = "keel_sdk::serde", rename_all = "kebab-case")]
pub(super) enum Restart {
    Always,
    OnFailure,
    Never,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(crate = "keel_sdk::serde", rename_all = "lowercase")]
pub(super) enum Storage {
    Kv,
}

#[derive(Debug, Default, Deserialize)]
#[serde(crate = "keel_sdk::serde", deny_unknown_fields)]
pub(super) struct Rights {
    #[serde(default)]
    pub(super) observe: Vec<String>,
    #[serde(default)]
    pub(super) hook: Vec<String>,
    #[serde(default)]
    pub(super) patch: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(crate = "keel_sdk::serde", deny_unknown_fields)]
pub(super) struct FunctionSpec {
    pub(super) name: String,
    pub(super) kind: Kind,
    #[serde(default)]
    pub(super) mode: Option<Mode>,
    pub(super) access: Access,
    #[serde(default)]
    pub(super) key: Option<String>,
    #[serde(default)]
    pub(super) idempotency: Idempotency,
    #[serde(default)]
    pub(super) params: Option<Schema>,
    #[serde(default)]
    pub(super) result: Option<Schema>,
    #[serde(default)]
    pub(super) ui: Option<ScreenUi>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(crate = "keel_sdk::serde", deny_unknown_fields)]
pub(super) struct SettingSpec {
    pub(super) key: String,
    pub(super) schema: Schema,
    #[serde(default)]
    pub(super) default: Option<Value>,
    #[serde(default)]
    pub(super) level: Option<Level>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(crate = "keel_sdk::serde", deny_unknown_fields)]
pub(super) struct CronSpec {
    pub(super) id: String,
    pub(super) schedule: String,
    pub(super) func: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(crate = "keel_sdk::serde", deny_unknown_fields)]
pub(super) struct MenuSpec {
    pub(super) path: String,
    pub(super) func: String,
    #[serde(default)]
    pub(super) icon: Option<String>,
    #[serde(default)]
    pub(super) order: Option<i32>,
    #[serde(default)]
    pub(super) label: Option<String>,
    #[serde(default)]
    pub(super) badge: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(crate = "keel_sdk::serde", deny_unknown_fields)]
pub(super) struct ProblemSpec {
    pub(super) source: String,
    pub(super) func: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(crate = "keel_sdk::serde", deny_unknown_fields)]
pub(super) struct WidgetSpec {
    pub(super) kind: WidgetKind,
    pub(super) func: String,
    #[serde(default)]
    pub(super) limit: Option<u32>,
    #[serde(default)]
    pub(super) level: Option<Level>,
}

pub(super) fn parse(text: &str) -> Result<Manifest, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|error| format!("{FILE} is not JSON: {error}"))?;
    let protocol = value.get("protocol").and_then(Value::as_u64);
    serde_json::from_value(value).map_err(|error| match protocol {
        Some(protocol) if !spoken(protocol) => unspoken(protocol),
        _ => format!("{FILE} does not fit: {error}"),
    })
}

fn spoken(protocol: u64) -> bool {
    (u64::from(OLDEST_PROTOCOL)..=u64::from(PROTOCOL)).contains(&protocol)
}

fn unspoken(protocol: u64) -> String {
    format!(
        "it speaks protocol {protocol}; this installation speaks {OLDEST_PROTOCOL} to {PROTOCOL}"
    )
}

fn prefixed(id: &str, name: &str) -> bool {
    let dotted = id.replace('_', ".");
    [id, dotted.as_str()].iter().any(|prefix| {
        name.strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('.') && rest.len() > 1)
    })
}

fn overlaps(id: &str, module: &str) -> bool {
    let plugin = id.replace('_', ".");
    let module = module.replace('_', ".");
    plugin == module
        || plugin.starts_with(&format!("{module}."))
        || module.starts_with(&format!("{plugin}."))
}

fn closed(function: &str) -> bool {
    CLOSED
        .iter()
        .any(|module| function.split('.').next() == Some(module))
}

pub(super) fn inside(path: &str) -> bool {
    let path = Path::new(path);
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn leaves(messages: &Map<String, Value>) -> Vec<String> {
    let mut found = Vec::new();
    for (key, value) in messages {
        match value {
            Value::Object(nested) => found.extend(
                leaves(nested)
                    .into_iter()
                    .map(|leaf| format!("{key}.{leaf}")),
            ),
            _ => found.push(key.clone()),
        }
    }
    found
}

pub(super) fn faults(
    manifest: &Manifest,
    directory: &str,
    landscape: &Landscape,
    dictionaries: &BTreeMap<String, Value>,
) -> Vec<String> {
    let mut faults = Vec::new();
    let id = manifest.id.as_str();
    if !spoken(u64::from(manifest.protocol)) {
        faults.push(unspoken(u64::from(manifest.protocol)));
    }
    if id != directory {
        faults.push(format!(
            "the id {id} differs from its directory {directory}"
        ));
    }
    if CLOSED.iter().any(|module| overlaps(id, module)) {
        faults.push(format!("the id {id} is closed to plugins"));
    } else if let Some(module) = landscape.modules.keys().find(|module| overlaps(id, module)) {
        faults.push(format!(
            "the id {id} overlaps the namespace of the module {module}"
        ));
    }
    if Version::parse(&manifest.version).is_err() {
        faults.push(format!(
            "the version {} is not a semantic version",
            manifest.version
        ));
    }
    requirements(manifest, landscape, &mut faults);
    if !LEVELS.contains(&manifest.level) {
        faults.push(format!(
            "the level {} is not one a plugin may hold",
            manifest.level
        ));
    }
    if manifest.run.cmd.is_empty() {
        faults.push("the run command is empty".to_owned());
    }
    if manifest.run.mode == RunMode::Exec && manifest.run.restart.is_some() {
        faults
            .push("an exec plugin runs a process per frame and takes no restart policy".to_owned());
    }
    hooks(manifest, landscape, &mut faults);
    rights(manifest, landscape, &mut faults);
    let own: BTreeSet<&str> = manifest
        .functions
        .iter()
        .map(|function| function.name.as_str())
        .collect();
    for function in &manifest.functions {
        if matches!(function.kind, Kind::Stream | Kind::Download | Kind::Upload) {
            faults.push(format!(
                "the function {} is of a kind a plugin cannot answer",
                function.name
            ));
        }
    }
    contributions(manifest, &own, &mut faults);
    for (lang, messages) in dictionaries {
        let Value::Object(messages) = messages else {
            continue;
        };
        for key in leaves(messages) {
            if !prefixed(id, &key) {
                faults.push(format!(
                    "the {lang} key {key} does not carry the plugin prefix"
                ));
            }
        }
    }
    faults
}

fn requirements(manifest: &Manifest, landscape: &Landscape, faults: &mut Vec<String>) {
    if let Some(range) = &manifest.requires.keel {
        match fits(range, &landscape.version) {
            Ok(true) => {}
            Ok(false) => faults.push(format!(
                "it requires keel {range}, and this installation runs {}",
                landscape.version
            )),
            Err(reason) => faults.push(format!("the keel requirement {range}: {reason}")),
        }
    }
    for (module, range) in &manifest.requires.modules {
        let Some(version) = landscape.modules.get(module) else {
            faults.push(format!(
                "it requires the module {module}, which is not loaded"
            ));
            continue;
        };
        match fits(range, version) {
            Ok(true) => {}
            Ok(false) => faults.push(format!(
                "it requires {module} {range}, and the loaded {module} is {version}"
            )),
            Err(reason) => {
                faults.push(format!("the requirement {module} {range}: {reason}"));
            }
        }
    }
}

fn fits(range: &str, version: &str) -> Result<bool, String> {
    let range = VersionReq::parse(range).map_err(|error| error.to_string())?;
    let version = Version::parse(version).map_err(|error| error.to_string())?;
    Ok(range.matches(&version))
}

fn hooks(manifest: &Manifest, landscape: &Landscape, faults: &mut Vec<String>) {
    for target in &manifest.rights.hook {
        if closed(target) {
            faults.push(format!(
                "it may not hook {target}: keel.* and auth.* are closed to plugins"
            ));
            continue;
        }
        match landscape.functions.get(target) {
            None => faults.push(format!(
                "it may not hook {target}: no loaded module registers it"
            )),
            Some(level) if *level > manifest.level => faults.push(format!(
                "it may not hook {target}: the function asks for {level}, above the plugin's {}",
                manifest.level
            )),
            Some(_) => {}
        }
    }
}

fn rights(manifest: &Manifest, landscape: &Landscape, faults: &mut Vec<String>) {
    for mask in &manifest.rights.observe {
        if !mask_shaped(mask) {
            faults.push(format!(
                "the observe mask {mask:?} is neither *, a module wildcard nor an event name"
            ));
        }
    }
    for target in &manifest.rights.patch {
        if !landscape.functions.contains_key(target) {
            faults.push(format!(
                "it may not patch {target}: no loaded module registers it"
            ));
        }
    }
    for patch in &manifest.patches {
        match patch.get("target").and_then(Value::as_str) {
            Some(target) if manifest.rights.patch.iter().any(|right| right == target) => {}
            Some(target) => faults.push(format!(
                "a patch targets {target}, which rights.patch does not grant"
            )),
            None => faults.push("a patch names no target".to_owned()),
        }
    }
}

fn mask_shaped(mask: &str) -> bool {
    if mask == "*" {
        return true;
    }
    let name = mask.strip_suffix(".*").unwrap_or(mask);
    !name.is_empty()
        && name.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
}

fn contributions(manifest: &Manifest, own: &BTreeSet<&str>, faults: &mut Vec<String>) {
    let id = manifest.id.as_str();
    let mut named = |kind: &str, name: &str, seen: &mut BTreeSet<String>| {
        if !prefixed(id, name) {
            faults.push(format!(
                "the {kind} {name} does not carry the plugin prefix"
            ));
        } else if !seen.insert(name.to_owned()) {
            faults.push(format!("the {kind} {name} is declared twice"));
        }
    };
    let mut settings = BTreeSet::new();
    for setting in &manifest.settings {
        named("setting", &setting.key, &mut settings);
    }
    let mut jobs = BTreeSet::new();
    for job in &manifest.cron {
        named("cron job", &job.id, &mut jobs);
    }
    let mut sources = BTreeSet::new();
    for problem in &manifest.problems {
        named("problem source", &problem.source, &mut sources);
    }
    for setting in &manifest.settings {
        if let Some(fault) = schema_fault(setting) {
            faults.push(format!("the setting {}: {fault}", setting.key));
        }
    }
    for job in &manifest.cron {
        if let Some(reason) = schedule_fault(&job.schedule) {
            faults.push(format!(
                "the schedule {} of {} does not parse: {reason}",
                job.schedule, job.id
            ));
        }
    }
    let targets = manifest
        .cron
        .iter()
        .map(|job| ("cron job", job.id.as_str(), job.func.as_str()))
        .chain(manifest.problems.iter().map(|problem| {
            (
                "problem source",
                problem.source.as_str(),
                problem.func.as_str(),
            )
        }))
        .chain(
            manifest
                .menu
                .iter()
                .map(|node| ("menu node", node.path.as_str(), node.func.as_str())),
        )
        .chain(
            manifest
                .widgets
                .iter()
                .map(|widget| ("widget", widget.func.as_str(), widget.func.as_str())),
        );
    for (kind, name, func) in targets {
        if !own.contains(func) {
            faults.push(format!(
                "the {kind} {name} points at {func}, which is not a function of the plugin"
            ));
        }
    }
    for node in &manifest.menu {
        for key in [&node.label, &node.badge].into_iter().flatten() {
            if !prefixed(id, key) {
                faults.push(format!(
                    "the menu node {} names {key}, which does not carry the plugin prefix",
                    node.path
                ));
            }
        }
    }
    for (lang, path) in &manifest.i18n {
        if !inside(path) {
            faults.push(format!(
                "the {lang} dictionary {path} is not a path inside the plugin directory"
            ));
        }
    }
}

fn schema_fault(setting: &SettingSpec) -> Option<String> {
    let validator = match jsonschema::validator_for(setting.schema.as_value()) {
        Ok(validator) => validator,
        Err(error) => return Some(format!("the schema does not compile: {error}")),
    };
    let default = setting.default.as_ref()?;
    validator
        .validate(default)
        .err()
        .map(|error| format!("the default does not fit the schema: {error}"))
}

#[cfg(test)]
mod tests {
    use keel_sdk::Db;
    use keel_sdk::sea_orm::DatabaseConnection;
    use keel_sdk::serde_json::json;

    use super::*;

    fn landscape() -> Landscape {
        Landscape {
            version: "0.4.0".to_owned(),
            data_dir: "/var/lib/keel".into(),
            modules: BTreeMap::from([
                ("keel".to_owned(), "0.4.0".to_owned()),
                ("notify".to_owned(), "0.4.0".to_owned()),
                ("dns".to_owned(), "1.2.0".to_owned()),
            ]),
            functions: BTreeMap::from([
                ("dns.zone.delete".to_owned(), Level::User),
                ("dns.zone.edit".to_owned(), Level::User),
                ("dns.server.restart".to_owned(), Level::Admin),
                ("keel.account.delete".to_owned(), Level::Reseller),
                ("auth.session.list".to_owned(), Level::User),
            ]),
            db: Db::from_connection(DatabaseConnection::Disconnected),
        }
    }

    fn manifest() -> Value {
        json!({
            "protocol": PROTOCOL,
            "id": "acme",
            "version": "1.0.0",
            "requires": { "keel": ">=0.4, <1", "modules": { "dns": ">=1" } },
            "run": { "mode": "daemon", "cmd": ["./acme"], "restart": "on-failure" },
            "level": "user",
            "storage": "kv",
            "rights": {
                "observe": ["*", "dns.*", "keel.account.deleted"],
                "hook": ["dns.zone.delete"],
                "patch": ["dns.zone.edit"]
            },
            "functions": [
                { "name": "acme.sync.list", "kind": "list", "access": { "level": "user" } },
                { "name": "acme.sync.run", "kind": "action", "access": { "level": "user" } }
            ],
            "patches": [{ "target": "dns.zone.edit", "op": "add_field",
                          "field": { "name": "acme", "schema": { "type": "boolean" }, "store": "acme" } }],
            "settings": [{ "key": "acme.email", "schema": { "type": "string" }, "default": "a@b.c" }],
            "cron": [{ "id": "acme.renew", "schedule": "0 3 * * *", "func": "acme.sync.run" }],
            "menu": [{ "path": "dns/acme", "func": "acme.sync.list", "label": "acme.menu" }],
            "problems": [{ "source": "acme.queue", "func": "acme.sync.list" }],
            "widgets": [{ "kind": "list", "func": "acme.sync.list" }],
            "i18n": { "en": "i18n/en.json" }
        })
    }

    fn faults_of(value: Value) -> Vec<String> {
        let manifest = parse(&value.to_string()).expect("the manifest parses");
        let dictionaries = BTreeMap::from([(
            "en".to_owned(),
            json!({ "acme": { "sync": { "list": { "title": "Sync" } } } }),
        )]);
        faults(&manifest, "acme", &landscape(), &dictionaries)
    }

    fn refused(change: impl FnOnce(&mut Value), expected: &str) {
        let mut value = manifest();
        change(&mut value);
        let faults = faults_of(value);
        assert!(
            faults.iter().any(|fault| fault.contains(expected)),
            "{expected:?} is not among {faults:?}"
        );
    }

    #[test]
    fn the_full_manifest_of_the_design_shape_passes_every_check() {
        assert_eq!(faults_of(manifest()), Vec::<String>::new());
    }

    #[test]
    fn protocol_n_and_n_minus_one_are_spoken_and_nothing_else() {
        assert!(spoken(u64::from(PROTOCOL)));
        assert!(spoken(u64::from(OLDEST_PROTOCOL)));
        assert!(!spoken(0));
        assert!(!spoken(u64::from(PROTOCOL) + 1));
        refused(
            |value| value["protocol"] = json!(PROTOCOL + 1),
            &format!("it speaks protocol {}", PROTOCOL + 1),
        );
        let mut newer = manifest();
        newer["protocol"] = json!(PROTOCOL + 1);
        newer["future"] = json!(true);
        let error = parse(&newer.to_string()).expect_err("a newer section does not fit");
        assert!(error.contains("speaks protocol"), "{error}");
        let mut typo = manifest();
        typo["setings"] = json!([]);
        let error = parse(&typo.to_string()).expect_err("an unknown section is refused");
        assert!(error.contains("unknown field `setings`"), "{error}");
    }

    #[test]
    fn hooks_on_keel_and_auth_above_the_level_or_on_unknown_functions_are_refused() {
        for target in ["keel.account.delete", "auth.session.list"] {
            refused(
                |value| value["rights"]["hook"] = json!([target]),
                &format!("it may not hook {target}: keel.* and auth.* are closed to plugins"),
            );
        }
        refused(
            |value| value["rights"]["hook"] = json!(["dns.server.restart"]),
            "it may not hook dns.server.restart: the function asks for admin, above the plugin's user",
        );
        refused(
            |value| value["rights"]["hook"] = json!(["dns.zone.purge"]),
            "it may not hook dns.zone.purge: no loaded module registers it",
        );
        let mut admin = manifest();
        admin["level"] = json!("admin");
        admin["rights"]["hook"] = json!(["dns.server.restart"]);
        assert_eq!(faults_of(admin), Vec::<String>::new());
        assert!(closed("keel.principal.create"));
        assert!(closed("auth.login"));
        assert!(!closed("keeling.thing.do"));
        assert!(!closed("dns.zone.delete"));
    }

    #[test]
    fn requirements_name_the_platform_and_loaded_modules_with_their_ranges() {
        refused(
            |value| value["requires"]["keel"] = json!(">=0.6"),
            "it requires keel >=0.6, and this installation runs 0.4.0",
        );
        refused(
            |value| value["requires"]["modules"] = json!({ "panel": ">=1" }),
            "it requires the module panel, which is not loaded",
        );
        refused(
            |value| value["requires"]["modules"] = json!({ "dns": ">=2" }),
            "it requires dns >=2, and the loaded dns is 1.2.0",
        );
        refused(
            |value| value["requires"]["modules"] = json!({ "dns": "one" }),
            "the requirement dns one",
        );
    }

    #[test]
    fn the_identity_level_and_run_section_are_checked() {
        refused(
            |value| value["id"] = json!("other"),
            "the id other differs from its directory acme",
        );
        refused(
            |value| value["version"] = json!("1"),
            "the version 1 is not a semantic version",
        );
        for level in ["guest", "reseller", "owner"] {
            refused(
                |value| value["level"] = json!(level),
                &format!("the level {level} is not one a plugin may hold"),
            );
        }
        refused(
            |value| value["run"]["cmd"] = json!([]),
            "the run command is empty",
        );
        refused(
            |value| value["run"]["mode"] = json!("exec"),
            "an exec plugin runs a process per frame and takes no restart policy",
        );
        let mut exec = manifest();
        exec["run"] = json!({ "mode": "exec", "cmd": ["./acme"] });
        assert_eq!(faults_of(exec), Vec::<String>::new());
    }

    #[test]
    fn an_id_reaching_into_a_closed_or_composed_namespace_is_refused() {
        let id_faults = |id: &str| {
            let mut value = manifest();
            value["id"] = json!(id);
            let manifest = parse(&value.to_string()).expect("the manifest parses");
            faults(&manifest, id, &landscape(), &BTreeMap::new())
                .into_iter()
                .filter(|fault| fault.starts_with(&format!("the id {id} ")))
                .collect::<Vec<_>>()
        };
        for id in ["keel", "auth", "keel_x", "auth_session"] {
            assert_eq!(
                id_faults(id),
                vec![format!("the id {id} is closed to plugins")]
            );
        }
        for (id, module) in [("dns_zone", "dns"), ("dns", "dns"), ("notify_x", "notify")] {
            assert_eq!(
                id_faults(id),
                vec![format!(
                    "the id {id} overlaps the namespace of the module {module}"
                )]
            );
        }
        for id in ["acme", "dnsx", "keeling"] {
            assert_eq!(id_faults(id), Vec::<String>::new());
        }
        assert!(overlaps("dns", "dns_zone"));
        assert!(!overlaps("dnsx", "dns"));
    }

    #[test]
    fn functions_of_a_kind_a_frame_cannot_carry_are_refused() {
        for kind in ["stream", "download", "upload"] {
            refused(
                |value| value["functions"][0]["kind"] = json!(kind),
                "the function acme.sync.list is of a kind a plugin cannot answer",
            );
        }
    }

    #[test]
    fn every_contributed_name_carries_the_plugin_prefix_once() {
        refused(
            |value| value["settings"][0]["key"] = json!("auth.password.min_length"),
            "the setting auth.password.min_length does not carry the plugin prefix",
        );
        refused(
            |value| value["cron"][0]["id"] = json!("cron.prune"),
            "the cron job cron.prune does not carry the plugin prefix",
        );
        refused(
            |value| value["problems"][0]["source"] = json!("license.lease"),
            "the problem source license.lease does not carry the plugin prefix",
        );
        refused(
            |value| {
                let setting = value["settings"][0].clone();
                value["settings"] = json!([setting.clone(), setting]);
            },
            "the setting acme.email is declared twice",
        );
        refused(
            |value| value["menu"][0]["label"] = json!("menu.system"),
            "the menu node dns/acme names menu.system",
        );
        let mut foreign = manifest();
        foreign["i18n"] = json!({});
        let manifest = parse(&foreign.to_string()).expect("the manifest parses");
        let dictionaries = BTreeMap::from([(
            "en".to_owned(),
            json!({ "acme": { "title": "Acme" }, "keel": { "name": "Taken" } }),
        )]);
        let faults = faults(&manifest, "acme", &landscape(), &dictionaries);
        assert_eq!(
            faults,
            ["the en key keel.name does not carry the plugin prefix"]
        );
    }

    #[test]
    fn contributions_point_at_the_plugins_own_functions() {
        refused(
            |value| value["cron"][0]["func"] = json!("dns.zone.delete"),
            "the cron job acme.renew points at dns.zone.delete, which is not a function of the plugin",
        );
        refused(
            |value| value["menu"][0]["func"] = json!("acme.sync.gone"),
            "the menu node dns/acme points at acme.sync.gone",
        );
        refused(
            |value| value["widgets"][0]["func"] = json!("keel.account.list"),
            "the widget keel.account.list points at keel.account.list",
        );
        refused(
            |value| value["problems"][0]["func"] = json!("notify.problem.list"),
            "the problem source acme.queue points at notify.problem.list",
        );
    }

    #[test]
    fn schedules_setting_schemas_and_dictionary_paths_are_checked() {
        refused(
            |value| value["cron"][0]["schedule"] = json!("every day"),
            "the schedule every day of acme.renew does not parse",
        );
        refused(
            |value| value["settings"][0]["schema"] = json!({ "type": "nothing" }),
            "the setting acme.email: the schema does not compile",
        );
        refused(
            |value| value["settings"][0]["default"] = json!(5),
            "the setting acme.email: the default does not fit the schema",
        );
        for path in ["../en.json", "/etc/en.json", "i18n/../../en.json", ""] {
            refused(
                |value| value["i18n"]["en"] = json!(path),
                &format!("the en dictionary {path} is not a path inside the plugin directory"),
            );
        }
        assert!(inside("i18n/en.json"));
        assert!(inside("en.json"));
    }

    #[test]
    fn rights_to_observe_and_patch_are_shaped_and_patches_need_the_right() {
        for mask in ["", "dns.", "Dns.*", "dns zone", "*.zone"] {
            refused(
                |value| value["rights"]["observe"] = json!([mask]),
                &format!("the observe mask {mask:?} is neither"),
            );
        }
        refused(
            |value| value["rights"]["patch"] = json!(["dns.zone.purge"]),
            "it may not patch dns.zone.purge: no loaded module registers it",
        );
        refused(
            |value| value["rights"]["patch"] = json!([]),
            "a patch targets dns.zone.edit, which rights.patch does not grant",
        );
        refused(
            |value| value["patches"] = json!([{ "op": "add_field" }]),
            "a patch names no target",
        );
    }

    #[test]
    fn a_prefix_admits_the_id_and_its_dotted_form_like_the_registrar() {
        assert!(prefixed("acme", "acme.sync.list"));
        assert!(prefixed("acme_sync", "acme_sync.list"));
        assert!(prefixed("acme_sync", "acme.sync.list"));
        assert!(!prefixed("acme", "acme"));
        assert!(!prefixed("acme", "acme."));
        assert!(!prefixed("acme", "acmex.list"));
        assert_eq!(
            leaves(
                json!({ "acme": { "a": "1", "b": { "c": "2" } } })
                    .as_object()
                    .expect("an object")
            ),
            ["acme.a", "acme.b.c"]
        );
    }
}
