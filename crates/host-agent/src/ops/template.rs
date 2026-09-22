use std::fs;
use std::path::{Component, Path, PathBuf};

use minijinja::{Environment, UndefinedBehavior};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::files::{Kind, Placement, Previous, place, rollback};
use super::service::{check_passes, control};
use super::{Call, Dest};
use crate::protocol::{ERROR_RELOAD_FAILED, ERROR_RENDER_FAILED, ERROR_VALIDATE_FAILED, Failure};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Apply {
    template: String,
    #[serde(default)]
    vars: Map<String, Value>,
    dest: Dest,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    group: Option<String>,
    #[serde(default)]
    validate: Option<String>,
    #[serde(default)]
    reload: Option<String>,
}

pub(super) fn apply(call: &Call<'_>, args: Apply) -> Result<Value, Failure> {
    let target = call.resolve(&args.dest)?;
    let check = match &args.validate {
        Some(name) => Some(call.check(name)?),
        None => None,
    };
    if let Some(service) = &args.reload {
        call.service(service)?;
    }
    let source = template_path(call.templates, &args.template)?;
    let placement = Placement::resolve(
        call,
        args.mode.as_deref(),
        args.owner.as_deref(),
        args.group.as_deref(),
        Kind::File,
    )?;
    let rendered = render(&source, &args.template, &args.vars)?;
    let previous = Previous::read(&target)?;
    if previous.as_ref().is_some_and(|previous| {
        previous.content == rendered.as_bytes() && placement.matches(previous)
    }) {
        return Ok(json!({ "changed": false }));
    }
    place(
        &target,
        rendered.as_bytes(),
        placement.mode,
        placement.owner,
    )?;
    if let Some(argv) = check
        && let Err(output) = check_passes(argv)
    {
        rollback(&target, previous.as_ref())?;
        return Err(Failure::new(ERROR_VALIDATE_FAILED, output));
    }
    if let Some(service) = &args.reload
        && let Err(output) = control(call, "reload", service)
    {
        rollback(&target, previous.as_ref())?;
        if let Err(again) = control(call, "reload", service) {
            tracing::warn!(service, error = %again, "the reload after the rollback failed too");
        }
        return Err(Failure::new(ERROR_RELOAD_FAILED, output));
    }
    Ok(json!({ "changed": true }))
}

fn template_path(templates: &Path, id: &str) -> Result<PathBuf, Failure> {
    let relative = Path::new(id);
    let plain = !id.is_empty()
        && !id.contains('\0')
        && relative.components().all(|component| match component {
            Component::Normal(part) => !part.to_string_lossy().starts_with('.'),
            _ => false,
        });
    if !plain {
        return Err(Failure::policy(format!(
            "the template id {id} is not a relative path of plain names"
        )));
    }
    let path = templates.join(relative);
    match fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_file() => Ok(path),
        Ok(_) => Err(Failure::policy(format!(
            "the template {id} at {} is not a regular file",
            path.display()
        ))),
        Err(error) => Err(Failure::new(
            ERROR_RENDER_FAILED,
            format!("could not read the template {id}: {error}"),
        )),
    }
}

fn render(source: &Path, id: &str, vars: &Map<String, Value>) -> Result<String, Failure> {
    let text = fs::read_to_string(source).map_err(|error| {
        Failure::new(
            ERROR_RENDER_FAILED,
            format!("could not read the template {id}: {error}"),
        )
    })?;
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env.set_keep_trailing_newline(true);
    env.add_template(id, &text)
        .and_then(|()| env.get_template(id)?.render(vars))
        .map_err(|error| Failure::new(ERROR_RENDER_FAILED, format!("{error:#}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ERROR_POLICY;

    #[test]
    fn a_template_id_is_a_relative_path_of_plain_names_under_the_templates_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("bind")).unwrap();
        fs::write(dir.path().join("bind/zone.j2"), "x").unwrap();
        assert_eq!(
            template_path(dir.path(), "bind/zone.j2").unwrap(),
            dir.path().join("bind/zone.j2")
        );
        for bad in [
            "",
            "../zone.j2",
            "/etc/passwd",
            "bind/../zone.j2",
            ".hidden",
            "bind/.zone",
        ] {
            let failure = template_path(dir.path(), bad).unwrap_err();
            assert_eq!(failure.key, ERROR_POLICY, "{bad}");
        }
        let failure = template_path(dir.path(), "bind").unwrap_err();
        assert_eq!(failure.key, ERROR_POLICY);
        let failure = template_path(dir.path(), "bind/other.j2").unwrap_err();
        assert_eq!(failure.key, ERROR_RENDER_FAILED);
        assert!(
            failure.detail.contains("bind/other.j2"),
            "{}",
            failure.detail
        );
    }

    #[test]
    fn a_template_renders_its_variables_and_a_missing_one_fails_the_render() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("zone.j2");
        fs::write(
            &source,
            "$ORIGIN {{ origin }}.\n{{ name }} IN A {{ address }}\n",
        )
        .unwrap();
        let vars = serde_json::from_value(json!({
            "origin": "a.org",
            "name": "www",
            "address": "192.0.2.1",
        }))
        .unwrap();
        assert_eq!(
            render(&source, "zone.j2", &vars).unwrap(),
            "$ORIGIN a.org.\nwww IN A 192.0.2.1\n"
        );
        let missing = serde_json::from_value(json!({ "origin": "a.org" })).unwrap();
        let failure = render(&source, "zone.j2", &missing).unwrap_err();
        assert_eq!(failure.key, ERROR_RENDER_FAILED);
        assert!(failure.detail.contains("name"), "{}", failure.detail);
    }
}
