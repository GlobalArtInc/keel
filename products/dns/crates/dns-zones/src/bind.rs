use std::io;
use std::path::Path;
use std::sync::OnceLock;

use keel::prelude::*;
use keel::sdk::sea_orm::sea_query::Expr;
use keel::sdk::sea_orm::{
    ColumnTrait, Condition, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};
use keel::sdk::tokio;
use keel::sdk::tokio::sync::Mutex;
use keel_host::api::{Dest, Host, HostExecutor, Origin};

use crate::api::{RecordLine, ZoneRecords};
use crate::config::DnsConfig;
use crate::provider::DnsProvider;
use crate::{PRODUCT, zone};

pub(crate) const ZONE_TEMPLATE: &str = "bind/zone.j2";
pub(crate) const INCLUDE_TEMPLATE: &str = "bind/zone.conf.j2";
pub(crate) const INDEX_TEMPLATE: &str = "bind/zones.conf.j2";
pub(crate) const CHECK: &str = "named.check";

const SERVICE: &str = "named";
const ZONES_ROOT: &str = "zones";
const NAMED_ROOT: &str = "named";
const INDEX: &str = "keel-dns-zones.conf";
const MODE: &str = "0644";
const NOT_STARTED: &str = "dns.zone.error.not_started";
const BACKUP_FAILED: &str = "dns.zone.error.backup";

struct Attached {
    host: Arc<dyn HostExecutor>,
    config: Arc<DnsConfig>,
    records: Option<Arc<dyn ZoneRecords>>,
}

#[derive(Default)]
pub(crate) struct BindProvider {
    attached: OnceLock<Attached>,
    turn: Mutex<()>,
}

impl BindProvider {
    pub(crate) fn attach(
        &self,
        host: Arc<dyn HostExecutor>,
        config: Arc<DnsConfig>,
        records: Option<Arc<dyn ZoneRecords>>,
    ) {
        let _ = self.attached.set(Attached {
            host,
            config,
            records,
        });
    }

    fn attached(&self) -> Result<&Attached> {
        self.attached
            .get()
            .ok_or_else(|| KeelError::unavailable(NOT_STARTED))
    }

    pub(crate) async fn copy_zone_files(&self, dir: &BackupDir) -> Result<()> {
        let config = &self.attached()?.config;
        for name in served_names(dir.db(), None).await? {
            let file = zone::file_of(&name);
            let source = Path::new(&config.zones_dir).join(&file);
            let copied = match tokio::fs::symlink_metadata(&source).await {
                Ok(meta) if meta.is_file() => tokio::fs::copy(&source, dir.path().join(&file))
                    .await
                    .map(drop),
                Ok(_) => Err(io::Error::other("not a regular file")),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            };
            copied.map_err(|error| {
                KeelError::internal(BACKUP_FAILED)
                    .param("file", source.display().to_string())
                    .source(error)
            })?;
        }
        Ok(())
    }
}

#[async_trait]
impl DnsProvider for BindProvider {
    async fn apply(&self, ctx: &AfterContext, zone: &str) -> Result<()> {
        let _turn = self.turn.lock().await;
        let attached = self.attached()?;
        let db = ctx.db()?;
        let Some(current) = zone::Entity::find_by_id(zone).one(db).await? else {
            return Ok(());
        };
        if current.state != zone::STATE_PENDING {
            return Ok(());
        }
        let host = attached.host.on(origin(ctx));
        let published = match lines(attached, db, &current).await {
            Ok(records) => publish(&host, &attached.config, db, &current, &records).await,
            Err(error) => Err(error),
        };
        match published {
            Ok(()) => mark(db, &current, zone::STATE_ACTIVE, None, true).await,
            Err(error) => {
                mark(
                    db,
                    &current,
                    zone::STATE_ERROR,
                    Some(detail(&error, &current)),
                    current.served,
                )
                .await?;
                Err(error)
            }
        }
    }

    async fn remove(&self, ctx: &AfterContext, zones: &[zone::Model]) -> Result<()> {
        let _turn = self.turn.lock().await;
        let attached = self.attached()?;
        let host = attached.host.on(origin(ctx));
        index(&host, &attached.config, ctx.db()?, None).await?;
        for zone in zones {
            host.fs()
                .remove(&Dest::new(NAMED_ROOT, &zone.include()), false)
                .await?;
            host.fs()
                .remove(&Dest::new(ZONES_ROOT, &zone.file()), false)
                .await?;
        }
        Ok(())
    }
}

fn origin(ctx: &AfterContext) -> Origin {
    Origin::new(PRODUCT, ctx.principal(), ctx.request_id())
}

async fn lines(attached: &Attached, db: &Db, zone: &zone::Model) -> Result<Vec<RecordLine>> {
    let Some(records) = &attached.records else {
        return Ok(Vec::new());
    };
    let id: Id = zone
        .id
        .parse()
        .map_err(|_| KeelError::internal("dns.zone.error.id").param("id", zone.id.as_str()))?;
    records.lines(db, &id).await
}

async fn publish(
    host: &Host<'_>,
    config: &DnsConfig,
    db: &Db,
    zone: &zone::Model,
    records: &[RecordLine],
) -> Result<()> {
    let file = zone.file();
    host.template(ZONE_TEMPLATE)
        .vars(&json!({
            "name": zone.name,
            "serial": zone.serial,
            "refresh": zone.refresh,
            "retry": zone.retry,
            "expire": zone.expire,
            "minimum": zone.minimum,
            "nameserver": config.nameserver,
            "hostmaster": config.hostmaster,
            "records": records,
        }))
        .dest(ZONES_ROOT, &file)
        .mode(MODE)
        .validate(CHECK)
        .reload(SERVICE)
        .apply()
        .await?;
    host.template(INCLUDE_TEMPLATE)
        .vars(&json!({ "name": zone.name, "file": config.zone_path(&file) }))
        .dest(NAMED_ROOT, &zone.include())
        .mode(MODE)
        .validate(CHECK)
        .reload(SERVICE)
        .apply()
        .await?;
    index(host, config, db, Some(&zone.id)).await
}

async fn index(host: &Host<'_>, config: &DnsConfig, db: &Db, current: Option<&str>) -> Result<()> {
    let includes: Vec<String> = served_names(db, current)
        .await?
        .iter()
        .map(|name| config.include_path(&zone::include_of(name)))
        .collect();
    host.template(INDEX_TEMPLATE)
        .vars(&json!({ "includes": includes }))
        .dest(NAMED_ROOT, INDEX)
        .mode(MODE)
        .validate(CHECK)
        .reload(SERVICE)
        .apply()
        .await?;
    Ok(())
}

async fn served_names(db: &Db, current: Option<&str>) -> Result<Vec<String>> {
    let mut served = Condition::any().add(zone::Column::Served.eq(true));
    if let Some(id) = current {
        served = served.add(zone::Column::Id.eq(id));
    }
    Ok(zone::Entity::find()
        .select_only()
        .column(zone::Column::Name)
        .filter(served)
        .order_by_asc(zone::Column::Name)
        .into_tuple()
        .all(db)
        .await?)
}

async fn mark(
    db: &Db,
    zone: &zone::Model,
    state: &str,
    error: Option<String>,
    served: bool,
) -> Result<()> {
    zone::Entity::update_many()
        .col_expr(zone::Column::State, Expr::value(state))
        .col_expr(zone::Column::Error, Expr::value(error))
        .col_expr(zone::Column::Served, Expr::value(served))
        .filter(zone::Column::Id.eq(zone.id.as_str()))
        .filter(zone::Column::Serial.eq(zone.serial))
        .exec(db)
        .await?;
    Ok(())
}

fn detail(error: &KeelError, zone: &zone::Model) -> String {
    let marks = [zone.name.clone(), zone.file(), zone.include()];
    let lines = error
        .params()
        .get("detail")
        .and_then(Value::as_str)
        .map(|text| {
            text.lines()
                .filter(|line| marks.iter().any(|mark| mentions(line, mark)))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    if lines.is_empty() {
        error.key().to_owned()
    } else {
        lines
    }
}

fn mentions(line: &str, mark: &str) -> bool {
    let bounded =
        |c: Option<char>| c.is_none_or(|c| !(c.is_alphanumeric() || matches!(c, '-' | '_' | '.')));
    line.match_indices(mark).any(|(at, _)| {
        let mut after = line[at + mark.len()..].chars();
        bounded(line[..at].chars().next_back())
            && match after.next() {
                Some('.') => bounded(after.next()),
                next => bounded(next),
            }
    })
}

#[cfg(test)]
mod tests {
    use keel::sdk::chrono::Utc;

    use super::*;

    fn zone(name: &str) -> zone::Model {
        zone::Model {
            id: "01J9Q3W6B1R8N5X2K4T7M0V9Z3".to_owned(),
            account_id: "01J8A0000000000000000000A1".to_owned(),
            name: name.to_owned(),
            kind: zone::KIND_PRIMARY.to_owned(),
            serial: 2_026_092_400,
            refresh: 10_800,
            retry: 3_600,
            expire: 604_800,
            minimum: 3_600,
            template: None,
            state: zone::STATE_PENDING.to_owned(),
            error: None,
            served: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn the_stored_error_keeps_only_the_lines_about_the_zone_itself() {
        let output = [
            "zone ba.org/IN: loaded serial 1",
            "zone a.org.zone/IN: loaded serial 2",
            "/var/lib/keel/zones/keel-dns-a.org.zone.zone:3: unknown RR type 'X'",
            "zone x.a.org/IN: loaded serial 3",
            "/var/lib/keel/zones/keel-dns-a.org.zone:5: unknown RR type 'FOO'",
            "zone a.org/IN: loading from master file keel-dns-a.org.zone failed",
            "_default/a.org./IN: bad zone",
        ]
        .join("\n");
        let error = KeelError::internal("keel.host.error.check").param("detail", output);
        assert_eq!(
            detail(&error, &zone("a.org")),
            [
                "/var/lib/keel/zones/keel-dns-a.org.zone:5: unknown RR type 'FOO'",
                "zone a.org/IN: loading from master file keel-dns-a.org.zone failed",
                "_default/a.org./IN: bad zone",
            ]
            .join("\n")
        );
    }

    #[test]
    fn an_output_naming_only_other_zones_stores_the_error_key() {
        let error = KeelError::internal("keel.host.error.check")
            .param("detail", "zone b.org/IN: loaded serial 1");
        assert_eq!(detail(&error, &zone("a.org")), "keel.host.error.check");
        let bare = KeelError::internal("keel.host.error.check");
        assert_eq!(detail(&bare, &zone("a.org")), "keel.host.error.check");
    }
}
