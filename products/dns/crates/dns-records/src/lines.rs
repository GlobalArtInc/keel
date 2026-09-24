use keel::prelude::*;
use keel::sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use keel_dns_zones::api::{RecordLine, ZoneRecords};

use crate::{record, value};

pub(crate) struct Lines;

#[async_trait]
impl ZoneRecords for Lines {
    async fn lines(&self, db: &Db, zone: &Id) -> Result<Vec<RecordLine>> {
        let rows = record::Entity::find()
            .filter(record::Column::ZoneId.eq(zone.to_string()))
            .order_by_asc(record::Column::Name)
            .order_by_asc(record::Column::Kind)
            .order_by_asc(record::Column::CreatedAt)
            .order_by_asc(record::Column::Id)
            .all(db)
            .await?;
        Ok(rows
            .iter()
            .map(|row| RecordLine {
                name: row.name.clone(),
                ttl: u32::try_from(row.ttl).unwrap_or_default(),
                kind: row.kind.clone(),
                data: value::data(row),
            })
            .collect())
    }
}
