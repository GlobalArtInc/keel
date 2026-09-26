use std::collections::{BTreeMap, BTreeSet};

use keel::prelude::*;
use keel::sdk::sea_orm::sea_query::Expr;
use keel::sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use keel::sdk::tokio::sync::Mutex;
use keel_host::api::{Dest, Host, HostExecutor, Origin};
use keel_hosting_users::api::HostingUsers;

use crate::{PRODUCT, names, site};

pub(crate) const SERVER_TEMPLATE: &str = "panel/nginx-site.conf.j2";
pub(crate) const POOL_TEMPLATE: &str = "panel/php-fpm-pool.conf.j2";
pub(crate) const NGINX_CHECK: &str = "nginx.test";

const NGINX: &str = "nginx";
const VHOSTS: &str = "vhosts";
const DROP_IN_PREFIX: &str = "keel-panel-";
const SOCKETS: &str = "/run/php";
const MODE: &str = "0644";
const USER_GONE: &str = "hosting.site.error.user_gone";

pub(crate) type Turn = Arc<Mutex<()>>;

pub(crate) struct Gone {
    domain: String,
    php: Option<String>,
    php_applied: Option<String>,
}

impl From<site::Model> for Gone {
    fn from(row: site::Model) -> Self {
        Self {
            domain: row.domain,
            php: row.php,
            php_applied: row.php_applied,
        }
    }
}

pub(crate) struct Effects {
    host: Arc<dyn HostExecutor>,
    users: Arc<dyn HostingUsers>,
    clock: Arc<dyn Clock>,
    turn: Turn,
}

impl Effects {
    pub(crate) fn of<P>(call: &Call<P>, turn: &Turn) -> Result<Self> {
        Ok(Self {
            host: call.require::<dyn HostExecutor>()?,
            users: call.require::<dyn HostingUsers>()?,
            clock: call.require::<dyn Clock>()?,
            turn: Arc::clone(turn),
        })
    }

    pub(crate) async fn apply(&self, ctx: &AfterContext, ids: &[String]) -> Result<()> {
        let _turn = self.turn.lock().await;
        let db = ctx.db()?;
        let host = self.host.on(origin(ctx));
        let mut first = None;
        for id in ids {
            let Some(row) = site::Entity::find_by_id(id.as_str()).one(db).await? else {
                continue;
            };
            let outcome = self.render(&host, db, &row).await;
            let update = site::Entity::update_many()
                .col_expr(site::Column::UpdatedAt, Expr::value(self.clock.now()))
                .filter(site::Column::Id.eq(id.as_str()));
            match outcome {
                Ok(()) => {
                    update
                        .col_expr(site::Column::State, Expr::value(site::STATE_ACTIVE))
                        .col_expr(site::Column::Error, Expr::value(Option::<String>::None))
                        .col_expr(site::Column::PhpApplied, Expr::value(row.php.clone()))
                        .exec(db)
                        .await?;
                }
                Err(error) => {
                    update
                        .col_expr(site::Column::State, Expr::value(site::STATE_ERROR))
                        .col_expr(site::Column::Error, Expr::value(detail(&error)))
                        .exec(db)
                        .await?;
                    first.get_or_insert(error);
                }
            }
        }
        first.map_or(Ok(()), Err)
    }

    async fn render(&self, host: &Host<'_>, db: &Db, row: &site::Model) -> Result<()> {
        let user_id: Id = row
            .user_id
            .parse()
            .map_err(|_| KeelError::internal(USER_GONE).param("user", row.user_id.as_str()))?;
        let user =
            self.users.get(db, &user_id).await?.ok_or_else(|| {
                KeelError::internal(USER_GONE).param("user", row.user_id.as_str())
            })?;
        let home = user
            .home
            .as_deref()
            .ok_or_else(|| KeelError::internal(USER_GONE).param("user", user.name.as_str()))?;
        let domain = names::host_name(&row.domain, "domain")?;
        let aliases = names::aliases(&domain, &row.aliases.0)?;
        let root = format!("{}/{}", names::home(home)?, names::root(&row.root)?);
        let php = names::php(row.php.as_deref())?;
        let applied = names::php(row.php_applied.as_deref())?;
        let file = file_of(&domain);
        let socket = php.as_deref().map(|version| socket_of(&row.id, version));
        if let (Some(version), Some(socket)) = (php.as_deref(), socket.as_deref()) {
            host.template(POOL_TEMPLATE)
                .vars(&json!({
                    "pool": pool_of(&domain),
                    "user": user.name,
                    "socket": socket,
                    "home": home,
                }))
                .dest(&pool_root(version), &file)
                .mode(MODE)
                .validate(&pool_check(version))
                .reload(&pool_service(version))
                .apply()
                .await?;
        }
        let served = host
            .template(SERVER_TEMPLATE)
            .vars(&json!({
                "domain": domain,
                "aliases": aliases,
                "root": root,
                "socket": socket,
                "suspended": row.suspended,
            }))
            .dest(VHOSTS, &file)
            .mode(MODE)
            .validate(NGINX_CHECK)
            .reload(NGINX)
            .apply()
            .await;
        let (kept, dropped) = if served.is_ok() {
            (php.as_deref(), applied.as_deref())
        } else {
            (applied.as_deref(), php.as_deref())
        };
        if let Some(version) = dropped.filter(|version| kept != Some(*version)) {
            remove_pool(host, &file, version).await?;
        }
        served?;
        Ok(())
    }

    pub(crate) async fn remove(&self, ctx: &AfterContext, sites: &[Gone]) -> Result<()> {
        let _turn = self.turn.lock().await;
        let host = self.host.on(origin(ctx));
        let mut pools: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
        for site in sites {
            for version in [&site.php, &site.php_applied].into_iter().flatten() {
                pools
                    .entry(version.as_str())
                    .or_default()
                    .insert(file_of(&site.domain));
            }
        }
        let mut removed = false;
        for site in sites {
            let file = file_of(&site.domain);
            removed |= host.fs().remove(&Dest::new(VHOSTS, &file), false).await?;
        }
        let mut emptied = Vec::new();
        for (version, files) in pools {
            let mut gone = false;
            for file in files {
                gone |= host
                    .fs()
                    .remove(&Dest::new(&pool_root(version), &file), false)
                    .await?;
            }
            if gone {
                emptied.push(version);
            }
        }
        if removed {
            host.services().reload(NGINX, Some(NGINX_CHECK)).await?;
        }
        for version in emptied {
            host.services()
                .reload(&pool_service(version), Some(&pool_check(version)))
                .await?;
        }
        Ok(())
    }
}

async fn remove_pool(host: &Host<'_>, file: &str, version: &str) -> Result<()> {
    if host
        .fs()
        .remove(&Dest::new(&pool_root(version), file), false)
        .await?
    {
        host.services()
            .reload(&pool_service(version), Some(&pool_check(version)))
            .await?;
    }
    Ok(())
}

fn file_of(domain: &str) -> String {
    format!("{DROP_IN_PREFIX}{domain}.conf")
}

fn pool_of(domain: &str) -> String {
    format!("{DROP_IN_PREFIX}{domain}")
}

fn socket_of(site: &str, version: &str) -> String {
    format!("{SOCKETS}/{DROP_IN_PREFIX}{site}-{version}.sock")
}

fn pool_root(version: &str) -> String {
    format!("pools-{version}")
}

fn pool_service(version: &str) -> String {
    format!("php{version}-fpm")
}

fn pool_check(version: &str) -> String {
    format!("php{version}-fpm.test")
}

fn origin(ctx: &AfterContext) -> Origin {
    Origin::new(PRODUCT, ctx.principal(), ctx.request_id())
}

fn detail(error: &KeelError) -> String {
    error
        .params()
        .get("detail")
        .and_then(Value::as_str)
        .map_or_else(|| error.key().to_owned(), str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_files_of_a_site_carry_the_drop_in_prefix_and_the_php_version() {
        assert_eq!(file_of("example.com"), "keel-panel-example.com.conf");
        assert_eq!(pool_of("example.com"), "keel-panel-example.com");
        assert_eq!(
            socket_of("01J9Q3ZK5V6W7X8Y9Z0A1B2C3D", "8.4"),
            "/run/php/keel-panel-01J9Q3ZK5V6W7X8Y9Z0A1B2C3D-8.4.sock"
        );
        assert_eq!(pool_root("8.4"), "pools-8.4");
        assert_eq!(pool_service("8.4"), "php8.4-fpm");
        assert_eq!(pool_check("8.4"), "php8.4-fpm.test");
    }
}
