use std::collections::BTreeSet;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use keel_sdk::KeelError;
use keel_sdk::runtime::property_names;
use keel_types::descriptor::MemberAccess;
use keel_types::{Access, Descriptor, Level, Principal};
use serde_json::{Map, Value};

use super::Shared;
use super::live::Live;
use super::reply::{self, RequestId};
use crate::dispatcher::guards;
use crate::registry::{Function, Registry, View};

const MAX_DEPTH: usize = 16;

pub struct Projection<'a> {
    registry: &'a Registry,
    principal: &'a Principal,
}

impl<'a> Projection<'a> {
    pub fn new(registry: &'a Registry, principal: &'a Principal) -> Self {
        Self {
            registry,
            principal,
        }
    }

    pub fn admit(&self, function: &Function) -> Result<(), KeelError> {
        if function.def().is_internal() {
            return Err(guards::internal_only(function.name()));
        }
        self.admits(&function.descriptor().access)
    }

    fn admits(&self, access: &Access) -> Result<(), KeelError> {
        guards::level(access, self.principal)?;
        guards::permission(self.registry, access, self.principal)?;
        guards::feature(self.registry, access)
    }

    pub(crate) fn admits_view(&self, view: &View) -> Result<(), KeelError> {
        self.admits(view.access())?;
        guards::needs(self.registry, view.needs())
    }

    pub(super) fn callable(&self, name: &str) -> bool {
        self.registry
            .function(name)
            .is_some_and(|function| self.admit(function).is_ok())
    }

    pub(super) fn visible(&self) -> Vec<Descriptor> {
        self.registry
            .functions()
            .filter(|function| self.admit(function).is_ok())
            .map(|function| self.project(function))
            .collect()
    }

    pub fn project(&self, function: &Function) -> Descriptor {
        let level = self.principal.level;
        let mut descriptor = function.descriptor().clone();
        if let Some(params) = descriptor.params.as_object_mut() {
            strip_schema(params, level, 0);
        }
        if let Some(result) = descriptor.result.as_object_mut() {
            strip_schema(result, level, 0);
        }
        let fields = property_names(&descriptor.params);
        if let Some(ui) = &mut descriptor.ui {
            ui.columns
                .retain(|column| self.member_visible(column.access.as_ref()));
            for column in &mut ui.columns {
                if column.edit.as_ref().is_some_and(|edit| {
                    !self.callable(&edit.function) || self.degraded(&edit.function)
                }) {
                    column.edit = None;
                }
            }
            ui.actions.retain(|action| {
                action
                    .function
                    .as_deref()
                    .is_none_or(|function| self.callable(function))
                    && action
                        .screen
                        .as_deref()
                        .is_none_or(|screen| self.callable(screen))
            });
            for action in &mut ui.actions {
                let degraded = [action.function.as_deref(), action.screen.as_deref()]
                    .into_iter()
                    .flatten()
                    .any(|name| self.degraded(name));
                if degraded {
                    action.disabled = Some(guards::LICENSE.to_owned());
                }
            }
            let kept: BTreeSet<&str> = ui.actions.iter().map(|action| action.id.as_str()).collect();
            ui.row_actions.retain(|id| kept.contains(id.as_str()));
            if let Some(empty) = &mut ui.empty
                && empty
                    .action
                    .as_deref()
                    .is_some_and(|action| !kept.contains(action))
            {
                empty.action = None;
            }
            ui.fields.retain(|name, _| fields.contains(name));
            ui.multi.retain(|name| fields.contains(name));
            for page in &mut ui.pages {
                page.fields.retain(|name| fields.contains(name));
            }
        }
        descriptor
    }

    fn degraded(&self, name: &str) -> bool {
        self.registry.function(name).is_some_and(|function| {
            guards::degraded(self.registry, function.def(), self.principal).is_err()
        })
    }

    fn member_visible(&self, access: Option<&MemberAccess>) -> bool {
        let Some(access) = access else {
            return true;
        };
        if access
            .level
            .is_some_and(|required| self.principal.level < required)
        {
            return false;
        }
        match &access.permission {
            Some(permission) => guards::holds(self.registry, permission, self.principal),
            None => true,
        }
    }
}

fn strip_schema(schema: &mut Map<String, Value>, level: Level, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    let mut removed = Vec::new();
    if let Some(Value::Object(properties)) = schema.get_mut("properties") {
        removed = properties
            .iter()
            .filter(|(_, property)| above(property, level))
            .map(|(name, _)| name.clone())
            .collect();
        for name in &removed {
            properties.remove(name);
        }
        for property in properties.values_mut() {
            if let Value::Object(nested) = property {
                strip_schema(nested, level, depth + 1);
            }
        }
    }
    if !removed.is_empty()
        && let Some(Value::Array(required)) = schema.get_mut("required")
    {
        required.retain(|name| {
            name.as_str()
                .is_none_or(|name| !removed.iter().any(|gone| gone == name))
        });
    }
    for key in ["items", "additionalProperties", "not", "$defs"] {
        match schema.get_mut(key) {
            Some(Value::Object(nested)) if key == "$defs" => {
                for definition in nested.values_mut() {
                    if let Value::Object(definition) = definition {
                        strip_schema(definition, level, depth + 1);
                    }
                }
            }
            Some(Value::Object(nested)) => strip_schema(nested, level, depth + 1),
            _ => {}
        }
    }
    for key in ["anyOf", "oneOf", "allOf", "prefixItems"] {
        if let Some(Value::Array(branches)) = schema.get_mut(key) {
            for branch in branches {
                if let Value::Object(branch) = branch {
                    strip_schema(branch, level, depth + 1);
                }
            }
        }
    }
}

fn above(property: &Value, level: Level) -> bool {
    property
        .get(keel_types::descriptor::X_KEEL)
        .and_then(|attrs| attrs.get("level"))
        .and_then(Value::as_str)
        .and_then(|required| required.parse::<Level>().ok())
        .is_some_and(|required| level < required)
}

pub(super) async fn list(
    State(state): State<Arc<Shared>>,
    live: Live,
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
) -> Response {
    let version = match state.meta_version(&live.registry, &principal).await {
        Ok(version) => version,
        Err(error) => return reply::error(error, &request_id),
    };
    if reply::matches_etag(&headers, &version) {
        return reply::not_modified(&version);
    }
    let descriptors = Projection::new(&live.registry, &principal).visible();
    reply::cached(Json(descriptors).into_response(), &version)
}

pub(super) async fn one(
    State(state): State<Arc<Shared>>,
    live: Live,
    Path(name): Path<String>,
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
) -> Response {
    let Some(function) = live.registry.function(&name) else {
        return reply::error(
            KeelError::not_found("keel.error.function_not_found").param("function", name),
            &request_id,
        );
    };
    let projection = Projection::new(&live.registry, &principal);
    if let Err(error) = projection.admit(function) {
        return reply::error(error, &request_id);
    }
    let version = match state.meta_version(&live.registry, &principal).await {
        Ok(version) => version,
        Err(error) => return reply::error(error, &request_id),
    };
    if reply::matches_etag(&headers, &version) {
        return reply::not_modified(&version);
    }
    let descriptor = projection.project(function);
    reply::cached(Json(descriptor).into_response(), &version)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn fields_above_the_level_leave_the_schema_with_their_required_entries() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "note": { "type": "string", "x-keel": { "level": "admin" } },
                "account": { "type": "string", "x-keel": { "level": "reseller" } },
                "items": { "type": "array", "items": { "$ref": "#/$defs/Item" } },
                "nested": { "anyOf": [ { "$ref": "#/$defs/Item" }, { "type": "null" } ] }
            },
            "required": ["title", "note", "account"],
            "$defs": {
                "Item": {
                    "type": "object",
                    "properties": {
                        "ip": { "type": "string" },
                        "secret_note": { "type": "string", "x-keel": { "level": "admin" } }
                    },
                    "required": ["ip", "secret_note"]
                }
            }
        });
        let mut untouched = schema.clone();
        strip_schema(untouched.as_object_mut().unwrap(), Level::Admin, 0);
        assert_eq!(untouched, schema);
        let object = schema.as_object_mut().unwrap();
        strip_schema(object, Level::Reseller, 0);
        let properties = object["properties"].as_object().unwrap();
        assert!(properties.contains_key("title"));
        assert!(properties.contains_key("account"));
        assert!(!properties.contains_key("note"));
        assert_eq!(object["required"], json!(["title", "account"]));
        let item = &object["$defs"]["Item"];
        assert!(item["properties"].get("secret_note").is_none());
        assert_eq!(item["required"], json!(["ip"]));
    }
}
