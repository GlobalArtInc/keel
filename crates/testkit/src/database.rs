use std::fmt;
use std::ops::Deref;
use std::time::{Duration, Instant};

use keel_kernel::config::{DatabaseUrl, DatabaseUrlError};
use keel_kernel::{Config, ConfigError, Pool, Storage, StorageError};
use keel_types::Id;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, DbErr, Statement};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt, ReuseDirective, TestcontainersError};
use testcontainers_modules::postgres::Postgres;
use tokio::sync::OnceCell;

pub const VARIABLE: &str = "KEEL_TEST_DATABASE_URL";
pub const PREFIX: &str = "keel_test_";
pub const TEMPLATE: &str = "template1";
pub const CONTAINER_IMAGE: &str = "postgres";
pub const CONTAINER_TAG: &str = "16";
pub const CONTAINER_NAME: &str = "keel-testkit-postgres";
pub const CONTAINER_PORT: u16 = 5432;
pub const CONTAINER_READY_TIMEOUT: Duration = Duration::from_secs(60);
pub const POOL: Pool = Pool {
    max_connections: 20,
    idle_timeout: Some(Duration::from_millis(250)),
};

const CONTAINER_USER: &str = "postgres";
const CONTAINER_PASSWORD: &str = "postgres";
const CONTAINER_START_ATTEMPTS: u32 = 3;
const CONTAINER_READY_PAUSE: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Server {
    Named,
    Container,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatabaseHandle {
    admin: String,
    server: Server,
    name: String,
    url: String,
}

pub struct TestDatabase {
    handle: DatabaseHandle,
    finished: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum DatabaseError {
    #[error("could not connect to {VARIABLE}: {0}")]
    Connect(#[source] DbErr),
    #[error(
        "{VARIABLE} is not set and the {CONTAINER_IMAGE}:{CONTAINER_TAG} container could not be started: {0}"
    )]
    Container(#[source] TestcontainersError),
    #[error(
        "the {CONTAINER_IMAGE}:{CONTAINER_TAG} container at {url} did not answer within {timeout:?}: {last}"
    )]
    ContainerReady {
        url: String,
        timeout: Duration,
        #[source]
        last: DbErr,
    },
    #[error("could not create the test database {name}: {source}")]
    Create {
        name: String,
        #[source]
        source: DbErr,
    },
    #[error("could not drop the test database {name}: {source}")]
    Drop {
        name: String,
        #[source]
        source: DbErr,
    },
    #[error("the test database URL is not usable: {0}")]
    Url(#[from] DatabaseUrlError),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("a runtime for dropping the test database could not start: {0}")]
    Runtime(#[source] std::io::Error),
}

struct Started {
    url: String,
    _container: ContainerAsync<Postgres>,
}

static STARTED: OnceCell<Started> = OnceCell::const_new();

async fn admin_url() -> Result<(String, Server), DatabaseError> {
    match std::env::var(VARIABLE) {
        Ok(url) => Ok((url, Server::Named)),
        Err(_) => {
            let started = STARTED.get_or_try_init(start_container).await?;
            Ok((started.url.clone(), Server::Container))
        }
    }
}

async fn start_container() -> Result<Started, DatabaseError> {
    let mut attempt = 0;
    let container = loop {
        attempt += 1;
        match Postgres::default()
            .with_user(CONTAINER_USER)
            .with_password(CONTAINER_PASSWORD)
            .with_name(CONTAINER_IMAGE)
            .with_tag(CONTAINER_TAG)
            .with_container_name(CONTAINER_NAME)
            .with_reuse(ReuseDirective::Always)
            .start()
            .await
        {
            Ok(container) => break container,
            Err(error) if attempt < CONTAINER_START_ATTEMPTS => {
                eprintln!(
                    "keel-testkit: starting {CONTAINER_IMAGE}:{CONTAINER_TAG} failed (attempt {attempt}): {error}"
                );
                tokio::time::sleep(CONTAINER_READY_PAUSE).await;
            }
            Err(error) => return Err(DatabaseError::Container(error)),
        }
    };
    let host = container
        .get_host()
        .await
        .map_err(DatabaseError::Container)?;
    let port = container
        .get_host_port_ipv4(CONTAINER_PORT)
        .await
        .map_err(DatabaseError::Container)?;
    let url = format!("postgres://{CONTAINER_USER}:{CONTAINER_PASSWORD}@{host}:{port}/postgres");
    wait_until_ready(&url).await?;
    eprintln!(
        "keel-testkit: {VARIABLE} is not set; using the {CONTAINER_IMAGE}:{CONTAINER_TAG} container {CONTAINER_NAME} at {url}"
    );
    Ok(Started {
        url,
        _container: container,
    })
}

async fn wait_until_ready(url: &str) -> Result<(), DatabaseError> {
    let started = Instant::now();
    loop {
        let outcome = match Database::connect(url).await {
            Ok(connection) => {
                let answered = connection
                    .execute(Statement::from_string(DbBackend::Postgres, "SELECT 1"))
                    .await;
                let _ = connection.close().await;
                answered.map(|_| ())
            }
            Err(error) => Err(error),
        };
        match outcome {
            Ok(()) => return Ok(()),
            Err(last) if started.elapsed() >= CONTAINER_READY_TIMEOUT => {
                return Err(DatabaseError::ContainerReady {
                    url: url.to_owned(),
                    timeout: CONTAINER_READY_TIMEOUT,
                    last,
                });
            }
            Err(_) => tokio::time::sleep(CONTAINER_READY_PAUSE).await,
        }
    }
}

impl TestDatabase {
    pub async fn create() -> Self {
        match Self::try_create().await {
            Ok(database) => database,
            Err(error) => panic!("{error}"),
        }
    }

    pub async fn try_create() -> Result<Self, DatabaseError> {
        let (admin, server) = admin_url().await?;
        let name = format!("{PREFIX}{}", Id::generate().to_string().to_lowercase());
        let connection = connect(&admin).await?;
        let created = connection
            .execute_unprepared(&format!("CREATE DATABASE \"{name}\" TEMPLATE {TEMPLATE}"))
            .await
            .map_err(|source| DatabaseError::Create {
                name: name.clone(),
                source,
            });
        let closed = connection.close().await;
        created?;
        closed.map_err(DatabaseError::Connect)?;
        let url = with_database(&admin, &name);
        Ok(Self {
            handle: DatabaseHandle {
                admin,
                server,
                name,
                url,
            },
            finished: false,
        })
    }

    pub fn handle(&self) -> DatabaseHandle {
        self.handle.clone()
    }

    pub async fn finish(self) {
        if let Err(error) = self.try_finish().await {
            panic!("{error}");
        }
    }

    pub async fn try_finish(mut self) -> Result<(), DatabaseError> {
        self.finished = true;
        drop_database(&self.handle.admin, &self.handle.name).await
    }
}

impl Deref for TestDatabase {
    type Target = DatabaseHandle;

    fn deref(&self) -> &DatabaseHandle {
        &self.handle
    }
}

impl DatabaseHandle {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn server(&self) -> Server {
        self.server
    }

    pub fn admin_url(&self) -> &str {
        &self.admin
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn config(&self) -> Config {
        match self.try_config() {
            Ok(config) => config,
            Err(error) => panic!("{error}"),
        }
    }

    pub fn try_config(&self) -> Result<Config, DatabaseError> {
        let mut config = Config::parse(None, |_| None)?;
        config.database = DatabaseUrl::parse(&self.url)?;
        Ok(config)
    }

    pub async fn storage(&self) -> Storage {
        match self.try_storage().await {
            Ok(storage) => storage,
            Err(error) => panic!("{error}"),
        }
    }

    pub async fn try_storage(&self) -> Result<Storage, DatabaseError> {
        Ok(Storage::connect_with(&self.try_config()?, POOL).await?)
    }

    pub async fn count(&self, storage: &Storage, sql: &str) -> i64 {
        let row = storage
            .db()
            .query_one(Statement::from_string(DbBackend::Postgres, sql))
            .await
            .unwrap_or_else(|error| panic!("{sql}: {error}"))
            .unwrap_or_else(|| panic!("{sql}: no row"));
        row.try_get_by_index::<i64>(0)
            .unwrap_or_else(|error| panic!("{sql}: {error}"))
    }

    pub async fn rows(&self, sql: &str) -> Vec<String> {
        let storage = self.storage().await;
        let rows = storage
            .db()
            .query_all(Statement::from_string(DbBackend::Postgres, sql))
            .await
            .unwrap_or_else(|error| panic!("{sql}: {error}"));
        let values = rows
            .iter()
            .map(|row| {
                row.try_get_by_index::<String>(0)
                    .unwrap_or_else(|error| panic!("{sql}: {error}"))
            })
            .collect();
        if let Err(error) = storage.close().await {
            panic!("closing the pool of {}: {error}", self.name);
        }
        values
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        let admin = self.handle.admin.clone();
        let name = self.handle.name.clone();
        let outcome = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(DatabaseError::Runtime)?;
            runtime.block_on(drop_database(&admin, &name))
        })
        .join();
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(error)) => eprintln!("keel-testkit: {error}"),
            Err(_) => eprintln!(
                "keel-testkit: the thread dropping the test database {} panicked",
                self.handle.name
            ),
        }
    }
}

impl fmt::Debug for TestDatabase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TestDatabase")
            .field("name", &self.handle.name)
            .field("finished", &self.finished)
            .finish()
    }
}

pub async fn exists(name: &str) -> Result<bool, DatabaseError> {
    let (admin, _) = admin_url().await?;
    let connection = connect(&admin).await?;
    let row = connection
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) FROM pg_database WHERE datname = $1",
            [name.into()],
        ))
        .await
        .map_err(DatabaseError::Connect)?;
    let count = match row {
        Some(row) => row
            .try_get_by_index::<i64>(0)
            .map_err(DatabaseError::Connect)?,
        None => 0,
    };
    connection.close().await.map_err(DatabaseError::Connect)?;
    Ok(count > 0)
}

async fn connect(url: &str) -> Result<DatabaseConnection, DatabaseError> {
    Database::connect(url).await.map_err(DatabaseError::Connect)
}

async fn drop_database(admin: &str, name: &str) -> Result<(), DatabaseError> {
    let connection = connect(admin).await?;
    let dropped = connection
        .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
        .await
        .map_err(|source| DatabaseError::Drop {
            name: name.to_owned(),
            source,
        });
    let closed = connection.close().await;
    dropped?;
    closed.map_err(DatabaseError::Connect)
}

fn with_database(url: &str, name: &str) -> String {
    let (base, query) = match url.split_once('?') {
        Some((base, query)) => (base, format!("?{query}")),
        None => (url, String::new()),
    };
    let scheme_end = base.find("://").map_or(0, |index| index + 3);
    let path_start = base[scheme_end..]
        .find('/')
        .map_or(base.len(), |index| scheme_end + index);
    format!("{}/{name}{query}", &base[..path_start])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_database_name_replaces_the_path_and_keeps_the_query() {
        assert_eq!(
            with_database(
                "postgres://u:p@h:5432/postgres?sslmode=disable",
                "keel_test_x"
            ),
            "postgres://u:p@h:5432/keel_test_x?sslmode=disable"
        );
        assert_eq!(
            with_database("postgres://localhost", "keel_test_x"),
            "postgres://localhost/keel_test_x"
        );
        assert_eq!(
            with_database("postgres:///postgres?host=/run/postgresql", "keel_test_x"),
            "postgres:///keel_test_x?host=/run/postgresql"
        );
    }
}
