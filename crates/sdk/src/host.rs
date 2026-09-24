use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use schemars::{JsonSchema, Schema, schema_for};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

const ERROR_ARGS: &str = "host.error.args";
const ERROR_RESULT: &str = "host.error.io";

pub trait AgentScope {
    fn product(&self) -> &str;

    fn policy(&self) -> &Value;

    fn resolve(&self, root: &str, name: &str) -> Result<PathBuf, AgentOpError>;
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{key}: {detail}")]
pub struct AgentOpError {
    key: String,
    detail: String,
}

impl AgentOpError {
    pub fn new(key: &str, detail: impl Into<String>) -> Self {
        Self {
            key: key.to_owned(),
            detail: detail.into(),
        }
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

type Handler = dyn Fn(&dyn AgentScope, Value) -> Result<Value, AgentOpError> + Send + Sync;

#[derive(Clone)]
pub struct AgentOp {
    name: String,
    class: Option<String>,
    args: Schema,
    result: Schema,
    handler: Arc<Handler>,
}

impl AgentOp {
    pub fn new<A, R>(
        name: &str,
        handler: impl Fn(&dyn AgentScope, A) -> Result<R, AgentOpError> + Send + Sync + 'static,
    ) -> Self
    where
        A: DeserializeOwned + JsonSchema,
        R: Serialize + JsonSchema,
    {
        let erased = move |scope: &dyn AgentScope, args: Value| {
            let args = serde_json::from_value::<A>(args)
                .map_err(|error| AgentOpError::new(ERROR_ARGS, error.to_string()))?;
            let result = handler(scope, args)?;
            serde_json::to_value(result)
                .map_err(|error| AgentOpError::new(ERROR_RESULT, error.to_string()))
        };
        Self {
            name: name.to_owned(),
            class: None,
            args: schema_for!(A),
            result: schema_for!(R),
            handler: Arc::new(erased),
        }
    }

    pub fn policy(mut self, class: &str) -> Self {
        self.class = Some(class.to_owned());
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn class(&self) -> &str {
        match &self.class {
            Some(class) => class,
            None => self
                .name
                .split_once('.')
                .map_or(self.name.as_str(), |(module, _)| module),
        }
    }

    pub fn args_schema(&self) -> &Schema {
        &self.args
    }

    pub fn result_schema(&self) -> &Schema {
        &self.result
    }

    pub fn run(&self, scope: &dyn AgentScope, args: Value) -> Result<Value, AgentOpError> {
        (self.handler)(scope, args)
    }
}

impl fmt::Debug for AgentOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentOp")
            .field("name", &self.name)
            .field("class", &self.class())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigTemplate {
    id: String,
    text: String,
    check: Option<String>,
}

impl ConfigTemplate {
    pub fn new(id: &str, text: &str) -> Self {
        Self {
            id: id.to_owned(),
            text: text.to_owned(),
            check: None,
        }
    }

    pub fn validate(mut self, check: &str) -> Self {
        self.check = Some(check.to_owned());
        self
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn check(&self) -> Option<&str> {
        self.check.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use serde_json::json;

    use super::*;

    struct Scope(Value);

    impl AgentScope for Scope {
        fn product(&self) -> &str {
            "demo"
        }

        fn policy(&self) -> &Value {
            &self.0
        }

        fn resolve(&self, root: &str, name: &str) -> Result<PathBuf, AgentOpError> {
            Ok(PathBuf::from(format!("/{root}/{name}")))
        }
    }

    #[derive(Deserialize, JsonSchema)]
    #[serde(deny_unknown_fields)]
    struct Args {
        name: String,
    }

    #[derive(Serialize, JsonSchema)]
    struct Answer {
        path: String,
        product: String,
        greeting: String,
    }

    fn answer(scope: &dyn AgentScope, args: Args) -> Result<Answer, AgentOpError> {
        let greeting = scope
            .policy()
            .get("greeting")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentOpError::new("host.error.policy", "no greeting"))?;
        Ok(Answer {
            path: scope.resolve("zones", &args.name)?.display().to_string(),
            product: scope.product().to_owned(),
            greeting: greeting.to_owned(),
        })
    }

    #[test]
    fn an_agent_op_decodes_its_arguments_runs_its_handler_and_encodes_its_result() {
        let op = AgentOp::new::<Args, Answer>("vm.libvirt.create", answer).policy("vm");
        assert_eq!(op.name(), "vm.libvirt.create");
        assert_eq!(op.class(), "vm");
        let scope = Scope(json!({ "greeting": "hello" }));
        assert_eq!(
            op.run(&scope, json!({ "name": "a" })).unwrap(),
            json!({ "path": "/zones/a", "product": "demo", "greeting": "hello" })
        );
        let refused = op
            .run(&scope, json!({ "name": "a", "path": "/etc" }))
            .unwrap_err();
        assert_eq!(refused.key(), "host.error.args");
        assert!(refused.detail().contains("path"), "{refused}");
        let failed = op
            .run(&Scope(json!({})), json!({ "name": "a" }))
            .unwrap_err();
        assert_eq!(
            failed,
            AgentOpError::new("host.error.policy", "no greeting")
        );
        assert!(format!("{op:?}").contains("vm.libvirt.create"));
    }

    #[test]
    fn an_agent_op_without_a_policy_runs_under_the_class_of_its_module() {
        let op = AgentOp::new::<Args, Answer>("vm.libvirt.create", answer);
        assert_eq!(op.class(), "vm");
        let schema = serde_json::to_value(op.args_schema()).unwrap();
        assert_eq!(schema["required"], json!(["name"]));
        let schema = serde_json::to_value(op.result_schema()).unwrap();
        assert!(schema["properties"].get("greeting").is_some(), "{schema}");
    }

    #[test]
    fn a_config_template_carries_its_id_text_and_named_check() {
        let template = ConfigTemplate::new("bind/zone.j2", "{{ origin }}\n");
        assert_eq!(template.check(), None);
        let template = template.validate("named.check");
        assert_eq!(template.id(), "bind/zone.j2");
        assert_eq!(template.text(), "{{ origin }}\n");
        assert_eq!(template.check(), Some("named.check"));
    }
}
