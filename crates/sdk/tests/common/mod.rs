use keel_sdk::sea_orm::{ConnectionTrait, Database, DatabaseConnection};
use keel_sdk::types::{Scope, Via};
use keel_sdk::{Id, Level, Principal};

pub const ROOT: &str = "01J8A0000000000000000000A0";
pub const RESELLER: &str = "01J8A0000000000000000000A1";
pub const CUSTOMER: &str = "01J8A0000000000000000000A2";
pub const OTHER: &str = "01J8A0000000000000000000A3";

pub struct TestDatabase {
    admin: DatabaseConnection,
    pub db: DatabaseConnection,
    name: String,
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
    let db = Database::connect(&with_database(&url, &name))
        .await
        .expect("connect to the test database");
    TestDatabase { admin, db, name }
}

impl TestDatabase {
    pub async fn with_accounts(self) -> Self {
        self.db
            .execute_unprepared(
                r#"CREATE TABLE keel_account (
                    id TEXT PRIMARY KEY,
                    kind TEXT NOT NULL,
                    parent_id TEXT REFERENCES keel_account(id),
                    path TEXT NOT NULL,
                    state TEXT NOT NULL DEFAULT 'active',
                    external_id TEXT,
                    origin TEXT,
                    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                    UNIQUE (origin, external_id)
                )"#,
            )
            .await
            .expect("create keel_account");
        for (id, kind, parent, path) in [
            (ROOT, "system", None, "/".to_owned()),
            (RESELLER, "reseller", Some(ROOT), format!("/{RESELLER}/")),
            (
                CUSTOMER,
                "user",
                Some(RESELLER),
                format!("/{RESELLER}/{CUSTOMER}/"),
            ),
            (OTHER, "user", Some(ROOT), format!("/{OTHER}/")),
        ] {
            let parent = parent.map_or("NULL".to_owned(), |parent| format!("'{parent}'"));
            self.db
                .execute_unprepared(&format!(
                    "INSERT INTO keel_account (id, kind, parent_id, path) VALUES ('{id}', '{kind}', {parent}, '{path}')"
                ))
                .await
                .expect("seed an account");
        }
        self
    }

    pub async fn drop(self) {
        self.db.close().await.expect("close the test connection");
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

pub fn principal(level: Level, account: &str) -> Principal {
    Principal {
        id: Id::generate(),
        login: format!("{level}-{account}"),
        level,
        account_id: account.parse().expect("account id"),
        role: "owner".into(),
        actor: None,
        scope: Scope::Full,
        via: Via::Http,
        external_id: None,
        origin: None,
        lang: "en".into(),
    }
}

#[test]
fn the_database_name_replaces_the_path_of_the_url() {
    assert_eq!(
        with_database("postgres://u@localhost/postgres", "x"),
        "postgres://u@localhost/x"
    );
    assert_eq!(
        with_database("postgres://u:p@h:5432/db?sslmode=disable", "x"),
        "postgres://u:p@h:5432/x?sslmode=disable"
    );
    assert_eq!(
        with_database("postgres://u@localhost", "x"),
        "postgres://u@localhost/x"
    );
}
