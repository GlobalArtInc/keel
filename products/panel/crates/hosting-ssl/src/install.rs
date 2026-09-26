use std::sync::OnceLock;

use keel::prelude::*;
use keel::sdk::chrono::{DateTime, Utc};
use keel::sdk::sea_orm::sea_query::Expr;
use keel::sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use keel::sdk::tokio::sync::Mutex;
use keel_host::api::{Attributes, Dest, HostExecutor, Origin};
use keel_hosting_web::api::{Invoke, SiteCertificates, SiteTls, Sites, TlsPair};

use crate::PRODUCT;
use crate::certificate::{self, STATE_ACTIVE, STATE_ERROR};
use crate::config::CertificateConfig;
use crate::functions::REQUEST;
use crate::pem::Material;

const CERTS: &str = "certs";
pub(crate) const PREFIX: &str = "keel-panel-";

const CHAIN_MODE: &str = "0644";
const KEY_MODE: &str = "0600";
const CHAIN_SUFFIX: &str = ".crt";
const KEY_SUFFIX: &str = ".key";
const DETACHED: &str = "hosting.certificate.error.detached";
const ID: &str = "hosting.certificate.error.id";

pub(crate) struct Store {
    config: OnceLock<Arc<CertificateConfig>>,
    clock: OnceLock<Arc<dyn Clock>>,
    turn: Mutex<()>,
}

impl Store {
    pub(crate) fn new() -> Self {
        Self {
            config: OnceLock::new(),
            clock: OnceLock::new(),
            turn: Mutex::new(()),
        }
    }

    pub(crate) fn attach(&self, ctx: &Context) -> Result<()> {
        let _ = self.config.set(ctx.config::<CertificateConfig>()?);
        let _ = self.clock.set(ctx.require::<dyn Clock>()?);
        Ok(())
    }

    pub(crate) fn config(&self) -> Result<&CertificateConfig> {
        self.config
            .get()
            .map(AsRef::as_ref)
            .ok_or_else(|| KeelError::internal(DETACHED))
    }

    pub(crate) fn now(&self) -> Result<DateTime<Utc>> {
        self.clock
            .get()
            .map(|clock| clock.now())
            .ok_or_else(|| KeelError::internal(DETACHED))
    }
}

#[async_trait]
impl SiteCertificates for Store {
    async fn tls(&self, db: &Db, site: &Id) -> Result<SiteTls> {
        let config = self.config()?;
        let installed = certificate::Entity::find()
            .filter(certificate::Column::SiteId.eq(site.to_string()))
            .one(db)
            .await?
            .and_then(|row| row.installed);
        Ok(SiteTls {
            challenges: config.challenges(),
            pair: installed.map(|stem| TlsPair {
                certificate: config.cert_path(&format!("{stem}{CHAIN_SUFFIX}")),
                key: config.cert_path(&format!("{stem}{KEY_SUFFIX}")),
            }),
        })
    }

    async fn request(&self, invoke: &Invoke<'_>, site: &Id) -> Result<()> {
        invoke(REQUEST, json!({ "site": site })).await.map(drop)
    }
}

pub(crate) struct Installer {
    host: Arc<dyn HostExecutor>,
    sites: Arc<dyn Sites>,
    store: Arc<Store>,
}

impl Installer {
    pub(crate) fn of<P>(call: &Call<P>, store: &Arc<Store>) -> Result<Self> {
        Ok(Self {
            host: call.require::<dyn HostExecutor>()?,
            sites: call.require::<dyn Sites>()?,
            store: Arc::clone(store),
        })
    }

    pub(crate) fn host(&self) -> &Arc<dyn HostExecutor> {
        &self.host
    }

    pub(crate) fn store(&self) -> &Store {
        &self.store
    }

    pub(crate) async fn render(&self, db: &Db, origin: Origin, site: &str) -> Result<()> {
        self.sites.render(db, origin, &parse_id(site)?).await
    }

    pub(crate) async fn install(
        &self,
        db: &Db,
        origin: Origin,
        id: &str,
        kind: &str,
        material: Material,
    ) -> Result<()> {
        let _turn = self.store.turn.lock().await;
        let Some(row) = certificate::Entity::find_by_id(id).one(db).await? else {
            return Ok(());
        };
        let host = self.host.on(origin.clone());
        let stem = format!("{PREFIX}{}", Id::generate().to_string().to_lowercase());
        let files = [
            (
                format!("{stem}{CHAIN_SUFFIX}"),
                material.chain.as_str(),
                CHAIN_MODE,
            ),
            (
                format!("{stem}{KEY_SUFFIX}"),
                material.key.expose(),
                KEY_MODE,
            ),
        ];
        for (name, content, mode) in &files {
            if let Err(error) = host
                .fs()
                .write(
                    &Dest::new(CERTS, name),
                    content,
                    &Attributes::default().mode(mode),
                )
                .await
            {
                self.fail(db, id, &error).await?;
                self.drop_files(origin.clone(), std::slice::from_ref(&stem))
                    .await?;
                return Err(error);
            }
        }
        let now = self.store.now()?;
        certificate::Entity::update_many()
            .col_expr(
                certificate::Column::Installed,
                Expr::value(Some(stem.clone())),
            )
            .col_expr(certificate::Column::Kind, Expr::value(kind))
            .col_expr(
                certificate::Column::NotAfter,
                Expr::value(Some(material.not_after)),
            )
            .col_expr(certificate::Column::State, Expr::value(STATE_ACTIVE))
            .col_expr(
                certificate::Column::Error,
                Expr::value(Option::<String>::None),
            )
            .col_expr(certificate::Column::UpdatedAt, Expr::value(now))
            .filter(certificate::Column::Id.eq(id))
            .exec(db)
            .await?;
        match self.render(db, origin.clone(), &row.site_id).await {
            Ok(()) => {
                if let Some(previous) = row.installed {
                    self.drop_files(origin, &[previous]).await?;
                }
                Ok(())
            }
            Err(error) => {
                certificate::Entity::update_many()
                    .col_expr(certificate::Column::Installed, Expr::value(row.installed))
                    .col_expr(certificate::Column::Kind, Expr::value(row.kind))
                    .col_expr(certificate::Column::NotAfter, Expr::value(row.not_after))
                    .filter(certificate::Column::Id.eq(id))
                    .exec(db)
                    .await?;
                self.fail(db, id, &error).await?;
                self.drop_files(origin, &[stem]).await?;
                Err(error)
            }
        }
    }

    pub(crate) async fn fail(&self, db: &Db, id: &str, error: &KeelError) -> Result<()> {
        certificate::Entity::update_many()
            .col_expr(certificate::Column::State, Expr::value(STATE_ERROR))
            .col_expr(certificate::Column::Error, Expr::value(Some(detail(error))))
            .col_expr(
                certificate::Column::UpdatedAt,
                Expr::value(self.store.now()?),
            )
            .filter(certificate::Column::Id.eq(id))
            .exec(db)
            .await?;
        Ok(())
    }

    pub(crate) async fn drop_files(&self, origin: Origin, stems: &[String]) -> Result<()> {
        let host = self.host.on(origin);
        for stem in stems {
            for suffix in [CHAIN_SUFFIX, KEY_SUFFIX] {
                host.fs()
                    .remove(&Dest::new(CERTS, &format!("{stem}{suffix}")), false)
                    .await?;
            }
        }
        Ok(())
    }
}

pub(crate) fn origin(principal: &Principal, request_id: &str) -> Origin {
    Origin::new(PRODUCT, principal, request_id)
}

pub(crate) fn parse_id(value: &str) -> Result<Id> {
    value
        .parse()
        .map_err(|_| KeelError::internal(ID).param("id", value))
}

pub(crate) fn detail(error: &KeelError) -> String {
    error
        .params()
        .get("detail")
        .and_then(Value::as_str)
        .map_or_else(|| error.key().to_owned(), str::to_owned)
}
