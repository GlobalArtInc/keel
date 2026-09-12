use std::fmt;

use keel_kernel::config::{DatabaseUrl, DatabaseUrlError};
use keel_kernel::{Config, ConfigError, Storage, StorageError};
use keel_types::Id;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, DbErr, Statement};

pub const VARIABLE: &str = "KEEL_TEST_DATABASE_URL";
pub const PREFIX: &str = "keel_test_";
pub const TEMPLATE: &str = "template1";

pub struct TestDatabase {
    admin: String,
    name: String,
    url: String,
    finished: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum DatabaseError {
    #[error(
        "{VARIABLE} is not set: point it at a Postgres superuser connection, every database test creates its own database from {TEMPLATE}"
    )]
    Unset,
    #[error("could not connect to {VARIABLE}: {0}")]
    Connect(#[source] DbErr),
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

fn admin_url() -> Result<String, DatabaseError> {
    std::env::var(VARIABLE).map_err(|_| DatabaseError::Unset)
}

impl TestDatabase {
    pub async fn create() -> Self {
        match Self::try_create().await {
            Ok(database) => database,
            Err(error) => panic!("{error}"),
        }
    }

    pub async fn try_create() -> Result<Self, DatabaseError> {
        let admin = admin_url()?;
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
            admin,
            name,
            url,
            finished: false,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
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
        Ok(Storage::connect(&self.try_config()?).await?)
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

    pub async fn finish(self) {
        if let Err(error) = self.try_finish().await {
            panic!("{error}");
        }
    }

    pub async fn try_finish(mut self) -> Result<(), DatabaseError> {
        self.finished = true;
        drop_database(&self.admin, &self.name).await
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        let admin = self.admin.clone();
        let name = self.name.clone();
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
                self.name
            ),
        }
    }
}

impl fmt::Debug for TestDatabase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TestDatabase")
            .field("name", &self.name)
            .field("finished", &self.finished)
            .finish()
    }
}

pub async fn exists(name: &str) -> Result<bool, DatabaseError> {
    let connection = connect(&admin_url()?).await?;
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
