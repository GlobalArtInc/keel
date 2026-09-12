use keel_sdk::DescriptorPatch;
use keel_sdk::runtime::AddedField;
use keel_types::{Access, Descriptor};
use serde_json::{Map, Value};

use super::freeze::FreezeProblem;
use crate::registry::PatchedField;

pub(super) fn apply(
    module: &str,
    patch: &DescriptorPatch,
    descriptor: &mut Descriptor,
    problems: &mut Vec<FreezeProblem>,
) -> Vec<PatchedField> {
    if let Some(change) = patch.access_change() {
        match strengthened(&descriptor.access, change) {
            Some(access) => descriptor.access = access,
            None => problems.push(FreezeProblem::PatchWeakensAccess {
                module: module.to_owned(),
                target: patch.target().to_owned(),
                from: descriptor.access.clone(),
                to: change.clone(),
            }),
        }
    }
    let mut patched = Vec::new();
    if patch.fields().is_empty() {
        return patched;
    }
    if properties(descriptor).is_none() {
        problems.push(FreezeProblem::PatchNotPatchable {
            module: module.to_owned(),
            target: patch.target().to_owned(),
        });
        return patched;
    }
    for field in patch.fields() {
        if let Some(store) = check(module, patch, field, descriptor, problems) {
            insert(descriptor, field);
            patched.push(PatchedField {
                name: field.name.clone(),
                module: module.to_owned(),
                store,
            });
        }
    }
    patched
}

fn strengthened(original: &Access, change: &Access) -> Option<Access> {
    if change.level < original.level {
        return None;
    }
    let permission = match (&original.permission, &change.permission) {
        (Some(kept), Some(changed)) if kept != changed => return None,
        (Some(kept), None) => Some(kept.clone()),
        (_, changed) => changed.clone(),
    };
    Some(Access {
        level: change.level,
        permission,
        feature: original.feature.clone(),
    })
}

fn check(
    module: &str,
    patch: &DescriptorPatch,
    field: &AddedField,
    descriptor: &Descriptor,
    problems: &mut Vec<FreezeProblem>,
) -> Option<keel_sdk::runtime::StoreHook> {
    let known: Vec<&String> = properties(descriptor)
        .map(Map::keys)
        .into_iter()
        .flatten()
        .collect();
    let mut sound = true;
    if known.contains(&&field.name) {
        problems.push(FreezeProblem::PatchFieldExists {
            module: module.to_owned(),
            target: patch.target().to_owned(),
            field: field.name.clone(),
        });
        sound = false;
    }
    if let Some(after) = &field.after
        && !known.contains(&after)
    {
        problems.push(FreezeProblem::PatchAfterUnknown {
            module: module.to_owned(),
            target: patch.target().to_owned(),
            field: field.name.clone(),
            after: after.clone(),
        });
        sound = false;
    }
    let store = field.store.clone();
    if store.is_none() {
        problems.push(FreezeProblem::PatchFieldWithoutStore {
            module: module.to_owned(),
            target: patch.target().to_owned(),
            field: field.name.clone(),
        });
    }
    store.filter(|_| sound)
}

fn insert(descriptor: &mut Descriptor, field: &AddedField) {
    let schema = field.schema.clone().to_value();
    if let Some(properties) = properties_mut(descriptor) {
        match &field.after {
            Some(after) => {
                let existing = std::mem::take(properties);
                for (name, value) in existing {
                    let follows = &name == after;
                    properties.insert(name, value);
                    if follows {
                        properties.insert(field.name.clone(), schema.clone());
                    }
                }
            }
            None => {
                properties.insert(field.name.clone(), schema);
            }
        }
    }
    let Some(ui) = descriptor.ui.as_mut() else {
        return;
    };
    let page = match &field.after {
        Some(after) => ui
            .pages
            .iter_mut()
            .find(|page| page.fields.iter().any(|name| name == after)),
        None => ui.pages.last_mut(),
    };
    if let Some(page) = page {
        let position = field
            .after
            .as_ref()
            .and_then(|after| page.fields.iter().position(|name| name == after))
            .map_or(page.fields.len(), |index| index + 1);
        page.fields.insert(position, field.name.clone());
    }
}

fn properties(descriptor: &Descriptor) -> Option<&Map<String, Value>> {
    descriptor
        .params
        .as_object()
        .and_then(|schema| schema.get("properties"))
        .and_then(Value::as_object)
}

fn properties_mut(descriptor: &mut Descriptor) -> Option<&mut Map<String, Value>> {
    descriptor
        .params
        .as_object_mut()
        .and_then(|schema| schema.get_mut("properties"))
        .and_then(Value::as_object_mut)
}
