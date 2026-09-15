use std::collections::BTreeMap;
use std::fmt;
use std::sync::OnceLock;

use keel_sdk::KeelError;
use keel_sdk::system::ParamMasker;
use serde_json::Value;

use super::strip::Plan;
use crate::registry::Function;

pub const ERROR_UNAVAILABLE: &str = "keel.error.masker_unavailable";
pub const ERROR_FUNCTION: &str = "keel.error.function_not_found";

#[derive(Default)]
pub struct MaskHandle {
    plans: OnceLock<BTreeMap<String, Plan>>,
}

impl MaskHandle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fill<'a>(&self, functions: impl IntoIterator<Item = &'a Function>) {
        self.plans.get_or_init(|| {
            functions
                .into_iter()
                .map(|function| {
                    (
                        function.name().to_owned(),
                        Plan::of_params(function.descriptor()),
                    )
                })
                .collect()
        });
    }

    pub fn is_filled(&self) -> bool {
        self.plans.get().is_some()
    }
}

impl ParamMasker for MaskHandle {
    fn mask(&self, func: &str, params: &mut Value) -> Result<(), KeelError> {
        let plans = self
            .plans
            .get()
            .ok_or_else(|| KeelError::unavailable(ERROR_UNAVAILABLE).param("function", func))?;
        let plan = plans
            .get(func)
            .ok_or_else(|| KeelError::not_found(ERROR_FUNCTION).param("function", func))?;
        plan.mask(params);
        Ok(())
    }
}

impl fmt::Debug for MaskHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MaskHandle")
            .field("filled", &self.is_filled())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use keel_types::ErrorCode;
    use serde_json::json;

    use super::*;

    #[test]
    fn an_unfilled_handle_answers_unavailable_and_reports_itself_as_such() {
        let handle = MaskHandle::new();
        assert!(!handle.is_filled());
        assert_eq!(format!("{handle:?}"), "MaskHandle { filled: false }");
        let mut params = json!({ "password": "hunter2" });
        let error = handle
            .mask("auth.login", &mut params)
            .expect_err("nothing is filled");
        assert_eq!(error.code(), ErrorCode::Unavailable);
        assert_eq!(error.key(), ERROR_UNAVAILABLE);
        assert_eq!(error.params()["function"], json!("auth.login"));
        assert_eq!(params["password"], json!("hunter2"));
        handle.fill([]);
        assert!(handle.is_filled());
        assert_eq!(format!("{handle:?}"), "MaskHandle { filled: true }");
        let missing = handle
            .mask("auth.login", &mut params)
            .expect_err("no such function");
        assert_eq!(missing.code(), ErrorCode::NotFound);
        assert_eq!(missing.key(), ERROR_FUNCTION);
    }
}
