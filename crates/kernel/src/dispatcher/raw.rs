use keel_sdk::KeelError;

use crate::registry::Function;

pub const RAW_PREFIX: &str = "/api/v1/x";
pub const DOWNLOAD_ROUTE_KEY: &str = "keel.error.download_route";
pub const UPLOAD_ROUTE_KEY: &str = "keel.error.upload_route";

pub fn raw_route(function: &Function) -> String {
    let module = function.module();
    let dotted = module.replace('_', ".");
    let name = [module, dotted.as_str()]
        .into_iter()
        .find_map(|prefix| function.name().strip_prefix(prefix)?.strip_prefix('.'))
        .unwrap_or_else(|| function.name());
    format!("{RAW_PREFIX}/{module}/{name}")
}

pub fn download_route_error(function: &Function) -> KeelError {
    KeelError::validation(DOWNLOAD_ROUTE_KEY)
        .param("function", function.name())
        .param("route", raw_route(function))
}

pub fn upload_route_error(function: &Function) -> KeelError {
    KeelError::validation(UPLOAD_ROUTE_KEY)
        .param("function", function.name())
        .param("route", raw_route(function))
}
