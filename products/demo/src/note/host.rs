use std::io;

use keel::prelude::*;

const DIGEST_TEMPLATE: &str = "note/digest.j2";
const DIGEST_CHECK: &str = "note.digest";
const DIGEST_STAT: &str = "note.digest.stat";
const DIGEST_CLASS: &str = "note";

const ERROR_POLICY: &str = "host.error.policy";
const ERROR_NOT_FOUND: &str = "host.error.not_found";
const ERROR_IO: &str = "host.error.io";

#[derive(Deserialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", deny_unknown_fields)]
#[schemars(crate = "keel::sdk::schemars")]
struct StatArgs {
    name: String,
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Stat {
    product: String,
    name: String,
    lines: usize,
    bytes: usize,
}

pub(super) fn template() -> ConfigTemplate {
    ConfigTemplate::new(
        DIGEST_TEMPLATE,
        include_str!("../../templates/note/digest.j2"),
    )
    .validate(DIGEST_CHECK)
}

pub(super) fn stat_op() -> AgentOp {
    AgentOp::new::<StatArgs, Stat>(DIGEST_STAT, stat).policy(DIGEST_CLASS)
}

fn stat(scope: &dyn AgentScope, args: StatArgs) -> std::result::Result<Stat, AgentOpError> {
    let root = scope
        .policy()
        .get("root")
        .and_then(Value::as_str)
        .ok_or_else(|| AgentOpError::new(ERROR_POLICY, "the policy class note names no root"))?;
    let path = scope.resolve(root, &args.name)?;
    let text = std::fs::read_to_string(&path).map_err(|error| {
        let key = if error.kind() == io::ErrorKind::NotFound {
            ERROR_NOT_FOUND
        } else {
            ERROR_IO
        };
        AgentOpError::new(key, format!("{}: {error}", args.name))
    })?;
    Ok(Stat {
        product: scope.product().to_owned(),
        name: args.name,
        lines: text.lines().count(),
        bytes: text.len(),
    })
}
