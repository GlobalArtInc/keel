use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};

use keel_kernel::dispatcher::{CallOptions, Dispatcher, Reply};
use keel_kernel::http::guest;
use keel_kernel::keel::{SETUP_KEY_PREFIX, SETUP_PATH, SetupHandle};
use keel_kernel::{Composition, Kernel, Storage};
use keel_sdk::Registrar;
use keel_sdk::prelude::*;
use keel_sdk::runtime::Db;
use keel_sdk::system::PasswordStore;
use keel_testkit::TestDatabase;
use keel_types::ErrorCode;

const SETUP_ADMIN: &str = "keel.setup.admin";
const LISTEN: &str = "127.0.0.1:1500";
const REFUSED_PASSWORD: &str = "short";

#[derive(Default)]
struct StoreDouble {
    stored: Mutex<Vec<(Id, String, String)>>,
}

impl StoreDouble {
    fn stored(&self) -> Vec<(Id, String, String)> {
        self.stored
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl PasswordStore for StoreDouble {
    async fn set(&self, principal: Id, login: &str, password: &Secret, _: &Db) -> Result<()> {
        if password.expose() == REFUSED_PASSWORD {
            return Err(KeelError::validation("store.error.password_short").field("password"));
        }
        self.stored
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((principal, login.to_owned(), password.expose().to_owned()));
        Ok(())
    }
}

static STORE: std::sync::OnceLock<Arc<StoreDouble>> = std::sync::OnceLock::new();

fn store() -> Arc<StoreDouble> {
    STORE.get_or_init(Arc::default).clone()
}

fn init(r: &mut Registrar) -> Result<()> {
    r.provide::<dyn PasswordStore>(store());
    Ok(())
}

static STORE_MODULE: ModuleDef = ModuleDef {
    id: "store",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init,
};

struct Bench {
    database: TestDatabase,
    storage: Storage,
    kernel: Kernel,
    dispatcher: Dispatcher,
}

impl Bench {
    async fn boot(database: TestDatabase, composition: Composition) -> Self {
        let storage = database.storage().await;
        let kernel = Kernel::boot(composition, &storage)
            .await
            .expect("the kernel boots");
        let dispatcher = Dispatcher::new(kernel.registry().clone(), storage.db().clone())
            .expect("every schema compiles");
        Self {
            database,
            storage,
            kernel,
            dispatcher,
        }
    }

    fn link(&self) -> Option<String> {
        let listen: SocketAddr = LISTEN.parse().expect("an address");
        self.kernel
            .registry()
            .services()
            .get::<SetupHandle>()
            .expect("the setup handle is a platform service")
            .link(listen, None)
    }

    fn key(&self) -> String {
        let link = self.link().expect("a setup link is held");
        let prefix = format!("https://{LISTEN}{SETUP_PATH}?key=");
        link.strip_prefix(&prefix)
            .unwrap_or_else(|| panic!("{link} does not start with {prefix}"))
            .to_owned()
    }

    async fn setup(&self, key: &str, login: &str, password: &str) -> Reply {
        self.dispatcher
            .call(
                guest("ru"),
                SETUP_ADMIN,
                json!({ "key": key, "login": login, "password": password }),
                CallOptions::default(),
            )
            .await
    }

    async fn principal_of(&self, login: &str) -> Value {
        let reply = self
            .dispatcher
            .call(
                keel_kernel::socket::owner(),
                "keel.principal.list",
                json!({ "search": login }),
                CallOptions::default(),
            )
            .await;
        let page = reply.data().expect("the owner lists principals").clone();
        let rows = page["data"].as_array().expect("rows");
        assert_eq!(rows.len(), 1, "{rows:?}");
        rows[0].clone()
    }

    async fn stop(self) -> TestDatabase {
        self.kernel.stop().await.expect("the kernel stops");
        self.storage.close().await.expect("the pool closes");
        self.database
    }
}

fn error_of(reply: &Reply) -> (ErrorCode, String) {
    let error = reply
        .error()
        .unwrap_or_else(|| panic!("an error was expected, got {:?}", reply.data()));
    (error.code, error.key.clone())
}

#[tokio::test]
async fn the_bare_kernel_issues_no_setup_key_without_a_password_store_and_the_call_names_the_missing_service()
 {
    let database = TestDatabase::create().await;
    let bench = Bench::boot(database, Composition::new()).await;
    assert_eq!(bench.link(), None);
    let reply = bench.setup("kok_nothing", "first", "correct-horse").await;
    assert_eq!(
        error_of(&reply),
        (ErrorCode::Internal, "keel.error.service_missing".to_owned())
    );
    bench.stop().await.finish().await;
}

#[tokio::test]
async fn the_setup_key_is_issued_while_no_administrator_exists_admits_its_exact_text_once_and_the_function_is_gone_after()
 {
    let database = TestDatabase::create().await;
    let bench = Bench::boot(database, Composition::new().module(&STORE_MODULE)).await;
    let key = bench.key();
    assert!(key.starts_with(SETUP_KEY_PREFIX), "{key}");

    let wrong = bench
        .setup(&key[..key.len() - 1], "first", "correct-horse")
        .await;
    assert_eq!(
        error_of(&wrong),
        (
            ErrorCode::Unauthenticated,
            "keel.setup.error.key".to_owned()
        )
    );
    assert_eq!(bench.key(), key, "a wrong key leaves the link in place");

    let refused = bench.setup(&key, "first", REFUSED_PASSWORD).await;
    assert_eq!(error_of(&refused).0, ErrorCode::Validation);
    assert_eq!(bench.key(), key, "a refused password gives the key back");
    assert!(
        store().stored().is_empty(),
        "nothing is stored for a refused password"
    );
    let none = bench
        .dispatcher
        .call(
            keel_kernel::socket::owner(),
            "keel.principal.list",
            json!({ "search": "first" }),
            CallOptions::default(),
        )
        .await;
    assert_eq!(none.data().expect("a page")["total"], json!(0));

    let created = bench.setup(&key, "first", "correct-horse").await;
    let data = created
        .data()
        .unwrap_or_else(|| panic!("the setup creates the administrator: {:?}", created.error()));
    assert_eq!(data["login"], json!("first"));
    let id: Id = data["id"].as_str().expect("an id").parse().expect("a ulid");
    let row = bench.principal_of("first").await;
    assert_eq!(row["id"], json!(id.to_string()));
    assert_eq!(row["level"], json!("admin"));
    assert_eq!(row["accountId"], json!(Id::ROOT_ACCOUNT.to_string()));
    assert_eq!(row["role"], json!("owner"));
    assert_eq!(row["lang"], json!("ru"));
    assert_eq!(
        store().stored(),
        vec![(id, "first".to_owned(), "correct-horse".to_owned())]
    );
    assert_eq!(bench.link(), None, "the key is consumed");

    let again = bench.setup(&key, "second", "correct-horse").await;
    assert_eq!(
        error_of(&again),
        (
            ErrorCode::NotFound,
            "keel.error.function_not_found".to_owned()
        )
    );
    let database = bench.stop().await;

    let rebooted = Bench::boot(database, Composition::new().module(&STORE_MODULE)).await;
    assert_eq!(
        rebooted.link(),
        None,
        "an installation with an administrator issues no key"
    );
    let gone = rebooted
        .setup("kok_anything", "third", "correct-horse")
        .await;
    assert_eq!(error_of(&gone).0, ErrorCode::NotFound);
    rebooted.stop().await.finish().await;
}
