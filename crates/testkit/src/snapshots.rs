use std::path::Path;

use keel_kernel::Registry;
use keel_kernel::http::Projection;
use keel_types::{Level, Principal};
use serde_json::json;

use crate::auth;

const KERNEL_MODULE: &str = "keel";

pub fn snapshot_descriptors(
    registry: &Registry,
    principal: &Principal,
    level: Level,
    dir: &Path,
) -> Vec<String> {
    assert!(
        dir.is_absolute(),
        "the snapshot directory must be absolute, build it from CARGO_MANIFEST_DIR: {}",
        dir.display()
    );
    let projection = Projection::new(registry, principal);
    let mut settings = insta::Settings::new();
    settings.set_snapshot_path(dir);
    settings.set_prepend_module_to_snapshot(false);
    settings.set_omit_expression(true);
    settings.set_snapshot_suffix(level.as_str());
    let mut names = Vec::new();
    settings.bind(|| {
        for function in registry.functions() {
            if is_platform_module(function.module()) || projection.admit(function).is_err() {
                continue;
            }
            let descriptor = projection.project(function);
            let name = function.name().to_owned();
            let mut settings = insta::Settings::clone_current();
            settings.set_info(&json!({ "function": name, "level": level }));
            settings.bind(|| insta::assert_json_snapshot!(name.clone(), descriptor));
            names.push(name);
        }
    });
    names
}

fn is_platform_module(module: &str) -> bool {
    module == KERNEL_MODULE || module == auth::MODULE.id
}
