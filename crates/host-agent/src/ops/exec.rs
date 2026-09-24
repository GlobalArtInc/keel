use std::path::Path;

use serde::Deserialize;
use serde_json::{Value, json};

use super::Call;
use crate::process;
use crate::protocol::Failure;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Exec {
    name: String,
}

pub(super) fn exec(call: &Call<'_>, args: Exec) -> Result<Value, Failure> {
    let argv = call
        .policy
        .exec(&args.name)
        .ok_or_else(|| Failure::policy(format!("{} is not an entry of exec_allow", args.name)))?;
    let (program, rest) = argv
        .split_first()
        .ok_or_else(|| Failure::policy(format!("the entry {} has no binary", args.name)))?;
    let output = process::succeed(Path::new(program), rest)?;
    Ok(json!({ "output": process::stdout(&output) }))
}
