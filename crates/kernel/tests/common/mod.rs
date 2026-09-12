use keel_kernel::config::DatabaseUrl;
use keel_kernel::{Config, ManifestDefaults, Storage};
use keel_types::Id;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};

pub const PRODUCT: &str = "demo";

pub struct TestDatabase {
    admin: DatabaseConnection,
    name: String,
    url: String,
}

pub async fn create() -> TestDatabase {
    let url = std::env::var("KEEL_TEST_DATABASE_URL").unwrap_or_else(|_| {
        panic!(
            "KEEL_TEST_DATABASE_URL is not set: point it at a Postgres superuser connection, every database test creates its own database from template1"
        )
    });
    let admin = Database::connect(&url)
        .await
        .expect("connect to KEEL_TEST_DATABASE_URL");
    let name = format!("keel_test_{}", Id::generate().to_string().to_lowercase());
    admin
        .execute_unprepared(&format!("CREATE DATABASE \"{name}\" TEMPLATE template1"))
        .await
        .expect("create the test database");
    let url = with_database(&url, &name);
    TestDatabase { admin, name, url }
}

impl TestDatabase {
    pub fn config(&self) -> Config {
        let defaults = ManifestDefaults::standard(PRODUCT, "127.0.0.1:8443".parse().unwrap());
        let mut config = Config::parse(None, &defaults, |_| None).expect("the defaults resolve");
        config.database = DatabaseUrl::parse(&self.url).expect("the test database URL is usable");
        config
    }

    pub async fn storage(&self) -> Storage {
        Storage::connect(&self.config())
            .await
            .expect("connect to the test database")
    }

    pub async fn count(&self, storage: &Storage, sql: &str) -> i64 {
        storage
            .db()
            .query_one(Statement::from_string(DbBackend::Postgres, sql))
            .await
            .expect("count query")
            .expect("count row")
            .try_get_by_index::<i64>(0)
            .expect("count value")
    }

    pub async fn drop(self) {
        self.admin
            .execute_unprepared(&format!("DROP DATABASE \"{}\" WITH (FORCE)", self.name))
            .await
            .expect("drop the test database");
    }
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
