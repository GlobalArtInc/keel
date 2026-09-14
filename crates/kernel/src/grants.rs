use keel_sdk::KeelError;
use keel_sdk::runtime::Db;
use keel_sdk::system::PermissionSource;
use keel_types::{Level, Principal};

use crate::registry::Registry;

pub(crate) async fn overlay(
    registry: &Registry,
    principal: &mut Principal,
    db: &Db,
) -> Result<(), KeelError> {
    if principal.level == Level::Guest || principal.level >= Level::Admin {
        return Ok(());
    }
    if let Some(source) = registry.services().get::<dyn PermissionSource>() {
        principal.grants = source.grants(principal.id, db).await?;
    }
    Ok(())
}
