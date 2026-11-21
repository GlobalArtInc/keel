use serde::Deserialize;
use serde_json::{Value, json};

use std::collections::BTreeMap;
use std::path::Path;

use super::Call;
use crate::policy::{is_pattern, service_matches, tool};
use crate::process;
use crate::protocol::{ERROR_EXEC_FAILED, ERROR_RELOAD_FAILED, ERROR_VALIDATE_FAILED, Failure};

const SERVICE_SUFFIX: &str = ".service";
const TIMESTAMP_UNIX: &str = "--timestamp=unix";
const PROPERTIES: &str = "--property=LoadState,ActiveState,MemoryCurrent,ActiveEnterTimestamp";
const LOAD_STATE: &str = "LoadState";
const ACTIVE_STATE: &str = "ActiveState";
const MEMORY_CURRENT: &str = "MemoryCurrent";
const ACTIVE_ENTER: &str = "ActiveEnterTimestamp";
const NOT_FOUND: &str = "not-found";
const ACTIVE: &str = "active";
const UNKNOWN: &str = "unknown";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Action {
    name: String,
    #[serde(default)]
    validate: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Unit {
    name: String,
}

pub(super) fn reload(call: &Call<'_>, args: Action) -> Result<Value, Failure> {
    act(call, "reload", args)
}

pub(super) fn restart(call: &Call<'_>, args: Action) -> Result<Value, Failure> {
    act(call, "restart", args)
}

pub(super) fn start(call: &Call<'_>, args: Action) -> Result<Value, Failure> {
    act(call, "start", args)
}

pub(super) fn stop(call: &Call<'_>, args: Unit) -> Result<Value, Failure> {
    call.service(&args.name)?;
    control(call, "stop", &args.name).map_err(|failure| failure.rekeyed(ERROR_RELOAD_FAILED))?;
    Ok(json!({ "name": args.name, "stop": true }))
}

fn act(call: &Call<'_>, verb: &str, args: Action) -> Result<Value, Failure> {
    call.service(&args.name)?;
    if let Some(check) = &args.validate {
        let argv = call.check(check)?;
        check_passes(call, check, argv)
            .map_err(|failure| failure.rekeyed(ERROR_VALIDATE_FAILED))?;
    }
    control(call, verb, &args.name).map_err(|failure| failure.rekeyed(ERROR_RELOAD_FAILED))?;
    Ok(json!({ "name": args.name, verb: true }))
}

pub(super) fn list(call: &Call<'_>, (): ()) -> Result<Value, Failure> {
    let systemctl = &call.policy.tools().systemctl;
    let units = units(call)?;
    if units.is_empty() {
        return Ok(json!({ "services": [] }));
    }
    let mut argv: Vec<&str> = vec!["is-active"];
    argv.extend(units.iter().map(String::as_str));
    let output = process::run(systemctl, &argv, call.policy.timeout(tool::SYSTEMCTL))?;
    let text = process::stdout(&output);
    let states: Vec<&str> = text.lines().collect();
    if states.len() != units.len() {
        return Err(Failure::new(
            ERROR_EXEC_FAILED,
            format!(
                "{} is-active answered {} line(s) for {} unit(s): {}",
                systemctl.display(),
                states.len(),
                units.len(),
                process::detail(systemctl, &output)
            ),
        ));
    }
    let listed: Vec<Value> = units
        .iter()
        .zip(states)
        .map(|(name, state)| json!({ "name": name, "state": state.trim() }))
        .collect();
    Ok(json!({ "services": listed }))
}

pub(super) fn status(call: &Call<'_>, (): ()) -> Result<Value, Failure> {
    let systemctl = &call.policy.tools().systemctl;
    let units = units(call)?;
    if units.is_empty() {
        return Ok(json!({ "services": [] }));
    }
    let mut argv: Vec<&str> = vec!["show", TIMESTAMP_UNIX, PROPERTIES];
    argv.extend(units.iter().map(String::as_str));
    let output = process::run(systemctl, &argv, call.policy.timeout(tool::SYSTEMCTL))?;
    if !output.status.success() {
        return Err(Failure::new(
            ERROR_EXEC_FAILED,
            process::detail(systemctl, &output),
        ));
    }
    let text = process::stdout(&output);
    let blocks = blocks(&text);
    if blocks.len() != units.len() {
        return Err(Failure::new(
            ERROR_EXEC_FAILED,
            format!(
                "{} show answered {} block(s) for {} unit(s): {}",
                systemctl.display(),
                blocks.len(),
                units.len(),
                process::detail(systemctl, &output)
            ),
        ));
    }
    let listed: Vec<Value> = units
        .iter()
        .zip(&blocks)
        .map(|(name, properties)| described(name, properties))
        .collect();
    Ok(json!({ "services": listed }))
}

fn units(call: &Call<'_>) -> Result<Vec<String>, Failure> {
    let mut units: Vec<String> = Vec::new();
    for entry in call.policy.services() {
        if is_pattern(entry) {
            for unit in installed(call, entry)? {
                if !units.contains(&unit) {
                    units.push(unit);
                }
            }
        } else if !units.contains(entry) {
            units.push(entry.clone());
        }
    }
    Ok(units)
}

fn blocks(text: &str) -> Vec<BTreeMap<&str, &str>> {
    let mut blocks = Vec::new();
    let mut current = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            if !current.is_empty() {
                blocks.push(std::mem::take(&mut current));
            }
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            current.insert(key, value);
        }
    }
    if !current.is_empty() {
        blocks.push(current);
    }
    blocks
}

fn described(name: &str, properties: &BTreeMap<&str, &str>) -> Value {
    let state = properties
        .get(ACTIVE_STATE)
        .copied()
        .filter(|state| !state.is_empty())
        .unwrap_or(UNKNOWN);
    let installed = properties
        .get(LOAD_STATE)
        .is_some_and(|load| *load != NOT_FOUND);
    let memory = properties
        .get(MEMORY_CURRENT)
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|bytes| *bytes != u64::MAX);
    let since = properties
        .get(ACTIVE_ENTER)
        .filter(|_| state == ACTIVE)
        .and_then(|value| value.strip_prefix('@'))
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|seconds| *seconds > 0);
    json!({
        "name": name,
        "state": state,
        "installed": installed,
        "memory": memory,
        "since": since,
    })
}

fn installed(call: &Call<'_>, entry: &str) -> Result<Vec<String>, Failure> {
    let systemctl = &call.policy.tools().systemctl;
    let pattern = format!("{entry}{SERVICE_SUFFIX}");
    let output = process::run(
        systemctl,
        [
            "list-unit-files",
            "--no-legend",
            "--type=service",
            pattern.as_str(),
        ],
        call.policy.timeout(tool::SYSTEMCTL),
    )?;
    if !output.status.success() {
        return Err(Failure::new(
            ERROR_EXEC_FAILED,
            process::detail(systemctl, &output),
        ));
    }
    Ok(process::stdout(&output)
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter_map(|unit| unit.strip_suffix(SERVICE_SUFFIX))
        .filter(|unit| service_matches(entry, unit))
        .map(str::to_owned)
        .collect())
}

pub(super) fn check_passes(call: &Call<'_>, name: &str, argv: &[String]) -> Result<(), Failure> {
    let (program, rest) = argv
        .split_first()
        .ok_or_else(|| Failure::new(ERROR_EXEC_FAILED, "the check has no command"))?;
    let program = Path::new(program);
    let output = process::run(program, rest, call.policy.timeout(name))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(Failure::new(
            ERROR_EXEC_FAILED,
            process::detail(program, &output),
        ))
    }
}

pub(super) fn control(call: &Call<'_>, verb: &str, unit: &str) -> Result<(), Failure> {
    let systemctl = &call.policy.tools().systemctl;
    let output = process::run(
        systemctl,
        [verb, unit],
        call.policy.timeout(tool::SYSTEMCTL),
    )?;
    if output.status.success() {
        Ok(())
    } else {
        Err(Failure::new(
            ERROR_EXEC_FAILED,
            process::detail(systemctl, &output),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_show_answer_reads_as_one_block_per_unit_with_memory_and_start_only_when_known() {
        let text = "ActiveState=active\nLoadState=loaded\nMemoryCurrent=7340032\nActiveEnterTimestamp=@1790000000\n\nLoadState=loaded\nActiveState=inactive\nMemoryCurrent=[not set]\nActiveEnterTimestamp=@1789000000\n\nActiveState=inactive\nLoadState=not-found\nMemoryCurrent=18446744073709551615\nActiveEnterTimestamp=\n";
        let blocks = blocks(text);
        assert_eq!(blocks.len(), 3);
        assert_eq!(
            described("nginx", &blocks[0]),
            json!({ "name": "nginx", "state": "active", "installed": true, "memory": 7340032, "since": 1790000000 })
        );
        assert_eq!(
            described("postfix", &blocks[1]),
            json!({ "name": "postfix", "state": "inactive", "installed": true, "memory": null, "since": null })
        );
        assert_eq!(
            described("vsftpd", &blocks[2]),
            json!({ "name": "vsftpd", "state": "inactive", "installed": false, "memory": null, "since": null })
        );
    }

    #[test]
    fn a_block_without_properties_is_unknown_and_trailing_blank_lines_add_no_block() {
        let blocks = blocks("\n\nLoadState=loaded\n\n\n");
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            described("dovecot", &blocks[0]),
            json!({ "name": "dovecot", "state": "unknown", "installed": true, "memory": null, "since": null })
        );
    }
}
