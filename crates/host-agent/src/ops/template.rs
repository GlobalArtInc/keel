use std::fs;
use std::io;
use std::path::{Component, Path};

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
    let (text, compiled_check) = source(call, &args.template)?;
    let check = match args.validate.as_deref().or(compiled_check) {
        Some(name) => Some((name, call.check(name)?)),
        None => None,
    };
    if let Some(service) = &args.reload {
        call.service(service)?;
    }
    let placement = Placement::resolve(
        call,
        args.mode.as_deref(),
        args.owner.as_deref(),
        args.group.as_deref(),
        Kind::File,
    )?;
    let rendered = render(&text, &args.template, &args.vars)?;
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
    if let Some((name, argv)) = check
        && let Err(failure) = check_passes(call, name, argv)
    {
        rollback(&target, previous.as_ref())?;
        return Err(failure.rekeyed(ERROR_VALIDATE_FAILED));
    }
    if let Some(service) = &args.reload
        && let Err(failure) = control(call, "reload", service)
    {
        rollback(&target, previous.as_ref())?;
        if let Err(again) = control(call, "reload", service) {
            tracing::warn!(service, error = %again.detail, "the reload after the rollback failed too");
        }
        return Err(failure.rekeyed(ERROR_RELOAD_FAILED));
    }
    Ok(json!({ "changed": true }))
}

fn plain_id(id: &str) -> Result<&Path, Failure> {
    let relative = Path::new(id);
    let plain = !id.is_empty()
        && !id.contains('\0')
        && relative.components().all(|component| match component {
            Component::Normal(part) => !part.to_string_lossy().starts_with('.'),
            _ => false,
        });
    if plain {
        Ok(relative)
    } else {
        Err(Failure::policy(format!(
            "the template id {id} is not a relative path of plain names"
        )))
    }
}

fn source<'c>(call: &Call<'c>, id: &str) -> Result<(String, Option<&'c str>), Failure> {
    let relative = plain_id(id)?;
    let compiled = call.compiled.get(id);
    let check = compiled.and_then(|template| template.check());
    let path = call.templates.join(relative);
    match fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_file() => {
            fs::read_to_string(&path)
                .map(|text| (text, check))
                .map_err(|error| {
                    Failure::new(
                        ERROR_RENDER_FAILED,
                        format!("could not read the template {id}: {error}"),
                    )
                })
        }
        Ok(_) => Err(Failure::policy(format!(
            "the template {id} at {} is not a regular file",
            path.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => compiled
            .map(|template| (template.text().to_owned(), check))
            .ok_or_else(|| {
                Failure::new(
                    ERROR_RENDER_FAILED,
                    format!(
                        "no template {id}: the binary carries none and {} does not exist",
                        path.display()
                    ),
                )
            }),
        Err(error) => Err(Failure::new(
            ERROR_RENDER_FAILED,
            format!("could not read the template {id}: {error}"),
        )),
    }
}

fn render(text: &str, id: &str, vars: &Map<String, Value>) -> Result<String, Failure> {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env.set_keep_trailing_newline(true);
    env.add_template(id, text)
        .and_then(|()| env.get_template(id)?.render(vars))
        .map_err(|error| Failure::new(ERROR_RENDER_FAILED, format!("{error:#}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ERROR_POLICY;

    #[test]
    fn a_template_id_is_a_relative_path_of_plain_names() {
        assert_eq!(plain_id("bind/zone.j2").unwrap(), Path::new("bind/zone.j2"));
        for bad in [
            "",
            "../zone.j2",
            "/etc/passwd",
            "bind/../zone.j2",
            ".hidden",
            "bind/.zone",
            "bind/\0",
        ] {
            let failure = plain_id(bad).unwrap_err();
            assert_eq!(failure.key, ERROR_POLICY, "{bad}");
        }
    }

    #[test]
    fn a_template_renders_its_variables_and_a_missing_one_fails_the_render() {
        let text = "$ORIGIN {{ origin }}.\n{{ name }} IN A {{ address }}\n";
        let vars = serde_json::from_value(json!({
            "origin": "a.org",
            "name": "www",
            "address": "192.0.2.1",
        }))
        .unwrap();
        assert_eq!(
            render(text, "zone.j2", &vars).unwrap(),
            "$ORIGIN a.org.\nwww IN A 192.0.2.1\n"
        );
        let missing = serde_json::from_value(json!({ "origin": "a.org" })).unwrap();
        let failure = render(text, "zone.j2", &missing).unwrap_err();
        assert_eq!(failure.key, ERROR_RENDER_FAILED);
        assert!(failure.detail.contains("name"), "{}", failure.detail);
    }
}
