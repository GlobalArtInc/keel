use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use keel_sdk::serde_json::{self, Value};
use keel_sdk::system::{Discovery, Extension, Landscape};

use super::DIRECTORY;
use super::entity::state;
use super::manifest::{self, FILE, Manifest};
use super::register::{Plugin, module};

pub(super) struct Read {
    pub(super) manifest: Manifest,
    dictionaries: Vec<(String, String)>,
    parsed: BTreeMap<String, Value>,
}

pub(super) fn root(data_dir: &Path) -> PathBuf {
    data_dir.join(DIRECTORY)
}

pub(super) async fn discover(landscape: Landscape) -> Result<Discovery> {
    let enabled = state::Entity::find()
        .filter(state::Column::Enabled.eq(true))
        .order_by_asc(state::Column::Id)
        .all(&landscape.db)
        .await?;
    let root = root(&landscape.data_dir);
    let mut discovery = Discovery::default();
    for row in enabled {
        match load(&root, &row, &landscape).await {
            Ok(plugin) => discovery.modules.push(module(plugin)),
            Err((version, reason)) => discovery.refused.push(Extension {
                id: row.id,
                version,
                refused: Some(reason),
            }),
        }
    }
    Ok(discovery)
}

async fn load(
    root: &Path,
    row: &state::Model,
    landscape: &Landscape,
) -> std::result::Result<Plugin, (String, String)> {
    let directory = root.join(&row.id);
    let read = read(&directory)
        .await
        .map_err(|reason| (String::new(), reason))?;
    let mut faults = manifest::faults(&read.manifest, &row.id, landscape, &read.parsed);
    if let Some(fault) = program(&directory, &read.manifest).await {
        faults.push(fault);
    }
    if read.manifest.level.as_str() != row.level {
        faults.push(format!(
            "it asks for the level {}, and it was enabled at {}; enable it again to grant the new level",
            read.manifest.level, row.level
        ));
    }
    if !faults.is_empty() {
        return Err((read.manifest.version, faults.join("; ")));
    }
    Ok(Plugin {
        manifest: read.manifest,
        dictionaries: read.dictionaries,
        levels: landscape.functions.clone(),
    })
}

async fn program(directory: &Path, manifest: &Manifest) -> Option<String> {
    let program = manifest.run.cmd.first()?;
    if !manifest::runnable(program) {
        return None;
    }
    let file = match tokio::fs::canonicalize(directory).await {
        Ok(home) => match tokio::fs::canonicalize(home.join(program)).await {
            Ok(found) => {
                found.starts_with(&home)
                    && tokio::fs::metadata(&found)
                        .await
                        .is_ok_and(|metadata| metadata.is_file())
            }
            Err(_) => false,
        },
        Err(_) => false,
    };
    (!file).then(|| format!("the run command {program} is not a file inside the plugin directory"))
}

pub(super) async fn read(directory: &Path) -> std::result::Result<Read, String> {
    let text = tokio::fs::read_to_string(directory.join(FILE))
        .await
        .map_err(|error| format!("{FILE} cannot be read: {error}"))?;
    let manifest = manifest::parse(&text)?;
    let home = tokio::fs::canonicalize(directory)
        .await
        .map_err(|error| format!("the plugin directory cannot be resolved: {error}"))?;
    let mut dictionaries = Vec::new();
    let mut parsed = BTreeMap::new();
    for (lang, path) in &manifest.i18n {
        if !manifest::inside(path) {
            continue;
        }
        let file = tokio::fs::canonicalize(directory.join(path))
            .await
            .map_err(|error| format!("the {lang} dictionary {path} cannot be read: {error}"))?;
        if !file.starts_with(&home) {
            return Err(format!(
                "the {lang} dictionary {path} resolves outside the plugin directory"
            ));
        }
        let text = tokio::fs::read_to_string(&file)
            .await
            .map_err(|error| format!("the {lang} dictionary {path} cannot be read: {error}"))?;
        let messages: Value = serde_json::from_str(&text)
            .map_err(|error| format!("the {lang} dictionary {path} is not JSON: {error}"))?;
        if !messages.is_object() {
            return Err(format!("the {lang} dictionary {path} is not a JSON object"));
        }
        parsed.insert(lang.clone(), messages);
        dictionaries.push((lang.clone(), text));
    }
    Ok(Read {
        manifest,
        dictionaries,
        parsed,
    })
}
