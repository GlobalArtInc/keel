use std::sync::Arc;

use keel_sdk::function::{ErasedHandler, natural_mode};
use keel_sdk::prelude::*;
use keel_sdk::schemars::json_schema;
use keel_sdk::system::DataModule;
use keel_sdk::types::{DESCRIPTOR_VERSION, Descriptor};

use super::manifest::{FunctionSpec, Manifest};
use crate::cron::api::CronJob;
use crate::notify::api::ProblemSource;

pub const ERROR_UNAVAILABLE: &str = "plugin.error.unavailable";

pub(super) struct Plugin {
    pub(super) manifest: Manifest,
    pub(super) dictionaries: Vec<(String, String)>,
}

pub(super) fn module(plugin: Plugin) -> DataModule {
    let plugin = Arc::new(plugin);
    DataModule {
        id: plugin.manifest.id.clone(),
        version: plugin.manifest.version.clone(),
        init: Arc::new(move |r: &mut Registrar| register(&plugin, r)),
    }
}

fn register(plugin: &Plugin, r: &mut Registrar) -> Result<()> {
    let manifest = &plugin.manifest;
    let id = manifest.id.as_str();
    for spec in &manifest.functions {
        r.described(descriptor(id, spec), unavailable(id));
    }
    for setting in &manifest.settings {
        let mut def = SettingDef::new(&setting.key, setting.schema.clone());
        if let Some(default) = &setting.default {
            def = def.default(default.clone());
        }
        if let Some(level) = setting.level {
            def = def.level(level);
        }
        r.contribute(def);
    }
    for job in &manifest.cron {
        let func = job.func.clone();
        r.contribute(CronJob::new(&job.id, &job.schedule, move |run| {
            let func = func.clone();
            async move { run.invoke(&func, json!({})).await.map(|_| ()) }
        }));
    }
    for node in &manifest.menu {
        let mut menu = MenuNode::new(&node.path, &node.func);
        if let Some(icon) = &node.icon {
            menu = menu.icon(icon);
        }
        if let Some(order) = node.order {
            menu = menu.order(order);
        }
        if let Some(label) = &node.label {
            menu = menu.label(label);
        }
        if let Some(badge) = &node.badge {
            menu = menu.badge(badge);
        }
        r.menu(menu);
    }
    for problem in &manifest.problems {
        let plugin = id.to_owned();
        r.contribute(ProblemSource::new(&problem.source, move |_db| {
            let plugin = plugin.clone();
            async move { Err(unanswered(&plugin)) }
        }));
    }
    for widget in &manifest.widgets {
        let mut def = DashboardWidget::new(widget.kind, &widget.func);
        if let Some(limit) = widget.limit {
            def = def.limit(limit);
        }
        if let Some(level) = widget.level {
            def = def.level(level);
        }
        r.contribute(def);
    }
    for (lang, text) in &plugin.dictionaries {
        r.i18n(lang, text);
    }
    Ok(())
}

fn descriptor(id: &str, spec: &FunctionSpec) -> Descriptor {
    Descriptor {
        keel: DESCRIPTOR_VERSION,
        name: spec.name.clone(),
        module: id.to_owned(),
        kind: spec.kind,
        mode: spec.mode.unwrap_or_else(|| natural_mode(spec.kind)),
        access: spec.access.clone(),
        key: spec.key.clone(),
        idempotency: spec.idempotency,
        params: spec
            .params
            .clone()
            .unwrap_or_else(|| json_schema!({ "type": "object" })),
        result: spec.result.clone().unwrap_or_else(|| json_schema!({})),
        ui: spec.ui.clone(),
    }
}

fn unavailable(id: &str) -> ErasedHandler {
    let plugin = id.to_owned();
    Arc::new(move |_call| {
        let plugin = plugin.clone();
        Box::pin(async move { Err(unanswered(&plugin)) })
    })
}

fn unanswered(plugin: &str) -> KeelError {
    KeelError::unavailable(ERROR_UNAVAILABLE).param("plugin", plugin)
}
