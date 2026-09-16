use keel_sdk::KeelError;

use crate::registry::Function;

pub const RAW_PREFIX: &str = "/api/v1/x";
pub const DOWNLOAD_ROUTE_KEY: &str = "keel.error.download_route";

pub fn raw_route(function: &Function) -> String {
    let module = function.module();
    let name = function
        .name()
        .strip_prefix(module)
        .and_then(|rest| rest.strip_prefix('.'))
        .unwrap_or_else(|| function.name());
    format!("{RAW_PREFIX}/{module}/{name}")
}

pub fn download_route_error(function: &Function) -> KeelError {
    KeelError::validation(DOWNLOAD_ROUTE_KEY)
        .param("function", function.name())
        .param("route", raw_route(function))
}
