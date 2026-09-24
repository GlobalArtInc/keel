use std::path::PathBuf;

use serde_json::Value;

use super::{Call, Dest};
use crate::protocol::Failure;

type Handler = dyn Fn(&OperationCall<'_>, Value) -> Result<Value, Failure> + Send + Sync;

pub struct Operation {
    name: String,
    class: String,
    handler: Box<Handler>,
}

impl Operation {
    pub fn new(
        name: &str,
        class: &str,
        handler: impl Fn(&OperationCall<'_>, Value) -> Result<Value, Failure> + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.to_owned(),
            class: class.to_owned(),
            handler: Box::new(handler),
        }
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn class(&self) -> &str {
        &self.class
    }
}

pub struct Template {
    id: String,
    text: String,
    check: Option<String>,
}

impl Template {
    pub fn new(id: &str, text: &str, check: Option<&str>) -> Self {
        Self {
            id: id.to_owned(),
            text: text.to_owned(),
            check: check.map(str::to_owned),
        }
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn check(&self) -> Option<&str> {
        self.check.as_deref()
    }
}

pub struct OperationCall<'a> {
    call: &'a Call<'a>,
    policy: &'a Value,
}

impl OperationCall<'_> {
    pub fn product(&self) -> &str {
        self.call.product_id
    }

    pub fn policy(&self) -> &Value {
        self.policy
    }

    pub fn resolve(&self, root: &str, name: &str) -> Result<PathBuf, Failure> {
        self.call.resolve(&Dest {
            root: root.to_owned(),
            name: name.to_owned(),
        })
    }
}

pub(super) fn run(call: &Call<'_>, operation: &Operation, args: &Value) -> Result<Value, Failure> {
    let policy = call.product.classes.get(&operation.class).ok_or_else(|| {
        Failure::policy(format!(
            "the product {} is not granted the policy class {} of {}",
            call.product_id, operation.class, operation.name
        ))
    })?;
    let args = if args.is_null() {
        Value::Object(Default::default())
    } else {
        args.clone()
    };
    (operation.handler)(&OperationCall { call, policy }, args)
}
