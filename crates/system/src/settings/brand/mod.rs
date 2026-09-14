pub(super) mod functions;
mod svg;

use keel_sdk::async_trait::async_trait;
use keel_sdk::sea_orm::EntityTrait;
use keel_sdk::system::{ManifestPatch, ManifestSource};
use keel_sdk::{Db, Result};

use super::entity::brand;

pub use svg::check_svg;

pub const HUE_MAX: u16 = 359;
pub const CHROMA_MAX: f64 = 0.4;

pub struct Brands;

pub fn normalise_host(host: &str) -> String {
    host.trim().to_ascii_lowercase()
}

pub fn patch_of(row: &brand::Model) -> ManifestPatch {
    ManifestPatch {
        name: row.name.clone(),
        logo: row.logo.clone(),
        hue: row.hue.and_then(|hue| u16::try_from(hue).ok()),
        chroma: row.chroma,
    }
}

#[async_trait]
impl ManifestSource for Brands {
    async fn patch(&self, host: &str, db: &Db) -> Result<Option<ManifestPatch>> {
        let row = brand::Entity::find_by_id(normalise_host(host))
            .one(db)
            .await?;
        Ok(row.as_ref().map(patch_of).filter(|patch| !patch.is_empty()))
    }
}

#[cfg(test)]
mod tests {
    use keel_sdk::chrono::Utc;

    use super::*;

    #[test]
    fn a_row_becomes_a_patch_of_its_present_members_and_the_host_is_matched_in_lower_case() {
        let row = brand::Model {
            host: "panel.example".into(),
            name: Some("Panel".into()),
            logo: None,
            hue: Some(12),
            chroma: None,
            updated_at: Utc::now(),
            updated_by: "root".into(),
        };
        assert_eq!(
            patch_of(&row),
            ManifestPatch {
                name: Some("Panel".into()),
                logo: None,
                hue: Some(12),
                chroma: None,
            }
        );
        let negative = brand::Model {
            hue: Some(-1),
            ..row.clone()
        };
        assert_eq!(patch_of(&negative).hue, None);
        let bare = brand::Model {
            name: None,
            hue: None,
            ..row
        };
        assert!(patch_of(&bare).is_empty());
        assert_eq!(normalise_host(" Panel.Example "), "panel.example");
    }
}
