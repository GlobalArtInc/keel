use serde::Deserialize;
use serde_json::{Value, json};

use std::path::Path;

use super::Call;
use crate::policy::{is_pattern, service_matches};
use crate::process;
use crate::protocol::{ERROR_EXEC_FAILED, ERROR_RELOAD_FAILED, ERROR_VALIDATE_FAILED, Failure};

const SERVICE_SUFFIX: &str = ".service";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Action {
    name: String,
    #[serde(default)]
    validate: Option<String>,
}

pub(super) fn reload(call: &Call<'_>, args: Action) -> Result<Value, Failure> {
    act(call, "reload", args)
}

pub(super) fn restart(call: &Call<'_>, args: Action) -> Result<Value, Failure> {
    act(call, "restart", args)
}

fn act(call: &Call<'_>, verb: &str, args: Action) -> Result<Value, Failure> {
    call.service(&args.name)?;
    if let Some(check) = &args.validate {
        let argv = call.check(check)?;
        check_passes(argv).map_err(|detail| Failure::new(ERROR_VALIDATE_FAILED, detail))?;
    }
    control(call, verb, &args.name).map_err(|detail| Failure::new(ERROR_RELOAD_FAILED, detail))?;
    Ok(json!({ "name": args.name, verb: true }))
}

pub(super) fn list(call: &Call<'_>, (): ()) -> Result<Value, Failure> {
    let systemctl = &call.policy.tools().systemctl;
    let mut units: Vec<String> = Vec::new();
    for entry in call.policy.services() {
        if is_pattern(entry) {
            for unit in installed(systemctl, entry)? {
                if !units.contains(&unit) {
                    units.push(unit);
                }
            }
        } else if !units.contains(entry) {
            units.push(entry.clone());
        }
    }
    if units.is_empty() {
        return Ok(json!({ "services": [] }));
    }
    let mut argv: Vec<&str> = vec!["is-active"];
    argv.extend(units.iter().map(String::as_str));
    let output = process::run(systemctl, &argv)?;
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

fn installed(systemctl: &Path, entry: &str) -> Result<Vec<String>, Failure> {
    let pattern = format!("{entry}{SERVICE_SUFFIX}");
    let output = process::run(
        systemctl,
        [
            "list-unit-files",
            "--no-legend",
            "--type=service",
            pattern.as_str(),
        ],
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

pub(super) fn check_passes(argv: &[String]) -> Result<(), String> {
    let (program, rest) = argv
        .split_first()
        .ok_or_else(|| "the check has no command".to_owned())?;
    let program = std::path::Path::new(program);
    let output = process::run(program, rest).map_err(|failure| failure.detail)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(process::detail(program, &output))
    }
}

pub(super) fn control(call: &Call<'_>, verb: &str, unit: &str) -> Result<(), String> {
    let systemctl = &call.policy.tools().systemctl;
    let output = process::run(systemctl, [verb, unit]).map_err(|failure| failure.detail)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(process::detail(systemctl, &output))
    }
}
