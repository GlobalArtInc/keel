use serde::Deserialize;
use serde_json::{Value, json};

use super::Call;
use crate::process;
use crate::protocol::{ERROR_EXEC_FAILED, ERROR_RELOAD_FAILED, ERROR_VALIDATE_FAILED, Failure};

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
    let services = call.policy.services();
    if services.is_empty() {
        return Ok(json!({ "services": [] }));
    }
    let systemctl = &call.policy.tools().systemctl;
    let mut argv: Vec<&str> = vec!["is-active"];
    argv.extend(services.iter().map(String::as_str));
    let output = process::run(systemctl, &argv)?;
    let text = process::stdout(&output);
    let states: Vec<&str> = text.lines().collect();
    if states.len() != services.len() {
        return Err(Failure::new(
            ERROR_EXEC_FAILED,
            format!(
                "{} is-active answered {} line(s) for {} unit(s): {}",
                systemctl.display(),
                states.len(),
                services.len(),
                process::detail(systemctl, &output)
            ),
        ));
    }
    let listed: Vec<Value> = services
        .iter()
        .zip(states)
        .map(|(name, state)| json!({ "name": name, "state": state.trim() }))
        .collect();
    Ok(json!({ "services": listed }))
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
