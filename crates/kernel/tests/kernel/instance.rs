use std::fs;
use std::os::unix::fs::PermissionsExt;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::SigningKey;
use keel_kernel::instance::{
    INSTANCE_ID_FILE, KV_INSTANCE_ID, KV_INSTANCE_KEY, KV_SECRET_KEY, SECRET_KEY_FILE,
};
use keel_kernel::secret::{Key, Sealed};
use keel_kernel::{Instance, InstanceError};
use keel_sdk::InstanceInfoSource;
use keel_sdk::prelude::*;
use keel_testkit::{Harness, TestDatabase, Who};
use keel_types::{InstanceInfo, PROTOCOL};
use sea_orm::{ConnectionTrait, DbBackend, Statement};

async fn kv(storage: &keel_kernel::Storage, key: &str) -> Option<serde_json::Value> {
    storage
        .db()
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT value FROM keel_kv WHERE key = $1",
            [key.into()],
        ))
        .await
        .unwrap()
        .map(|row| row.try_get::<serde_json::Value>("", "value").unwrap())
}

#[tokio::test]
async fn the_first_start_creates_the_data_dir_the_secret_key_with_mode_0600_and_the_instance_id() {
    let test = TestDatabase::create().await;
    let storage = test.storage().await;
    storage.migrate().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let data_dir = root
        .path()
        .join("var")
        .join("lib")
        .join("keel")
        .join("demo");
    assert!(!data_dir.exists());

    let instance = Instance::establish(&data_dir, storage.db()).await.unwrap();

    let secret_path = data_dir.join(SECRET_KEY_FILE);
    let mode = fs::metadata(&secret_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let file_key = Key::parse_hex(&fs::read_to_string(&secret_path).unwrap()).unwrap();
    let id_text = fs::read_to_string(data_dir.join(INSTANCE_ID_FILE)).unwrap();
    assert_eq!(id_text.trim(), instance.id().to_string());

    assert_eq!(
        kv(&storage, KV_INSTANCE_ID).await,
        Some(serde_json::Value::String(instance.id().to_string()))
    );
    let sealed: Sealed =
        serde_json::from_value(kv(&storage, KV_SECRET_KEY).await.unwrap()).unwrap();
    assert_eq!(sealed.alg, "A256GCM");
    let unwrapped = file_key.open(&sealed).unwrap();
    assert_eq!(unwrapped.as_slice(), instance.key().as_bytes());
    assert_ne!(instance.key(), &file_key);

    storage.close().await.unwrap();
    test.finish().await;
}

#[tokio::test]
async fn the_secret_key_is_reused_and_the_instance_id_is_stable_across_restarts() {
    let test = TestDatabase::create().await;
    let storage = test.storage().await;
    storage.migrate().await.unwrap();
    let data_dir = tempfile::tempdir().unwrap();

    let first = Instance::establish(data_dir.path(), storage.db())
        .await
        .unwrap();
    let key_file = fs::read(data_dir.path().join(SECRET_KEY_FILE)).unwrap();
    let id_file = fs::read(data_dir.path().join(INSTANCE_ID_FILE)).unwrap();
    let wrapped = kv(&storage, KV_SECRET_KEY).await;

    let second = Instance::establish(data_dir.path(), storage.db())
        .await
        .unwrap();
    assert_eq!(second.id(), first.id());
    assert_eq!(second.key(), first.key());
    assert_eq!(
        fs::read(data_dir.path().join(SECRET_KEY_FILE)).unwrap(),
        key_file
    );
    assert_eq!(
        fs::read(data_dir.path().join(INSTANCE_ID_FILE)).unwrap(),
        id_file
    );
    assert_eq!(kv(&storage, KV_SECRET_KEY).await, wrapped);
    assert_eq!(
        test.count(&storage, "SELECT count(*) FROM keel_kv").await,
        3
    );

    storage.close().await.unwrap();
    test.finish().await;
}

#[tokio::test]
async fn a_secret_key_that_does_not_match_the_wrapped_key_fails_start() {
    let test = TestDatabase::create().await;
    let storage = test.storage().await;
    storage.migrate().await.unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    Instance::establish(data_dir.path(), storage.db())
        .await
        .unwrap();

    let secret_path = data_dir.path().join(SECRET_KEY_FILE);
    fs::remove_file(&secret_path).unwrap();
    let error = Instance::establish(data_dir.path(), storage.db())
        .await
        .unwrap_err();
    assert!(
        matches!(error, InstanceError::SecretKeyMismatch { .. }),
        "{error}"
    );
    assert!(error.to_string().contains("secret.key"), "{error}");

    storage.close().await.unwrap();
    test.finish().await;
}

#[tokio::test]
async fn a_secret_key_readable_by_others_is_refused() {
    let test = TestDatabase::create().await;
    let storage = test.storage().await;
    storage.migrate().await.unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    Instance::establish(data_dir.path(), storage.db())
        .await
        .unwrap();

    let secret_path = data_dir.path().join(SECRET_KEY_FILE);
    fs::set_permissions(&secret_path, fs::Permissions::from_mode(0o644)).unwrap();
    let error = Instance::establish(data_dir.path(), storage.db())
        .await
        .unwrap_err();
    assert!(
        matches!(error, InstanceError::SecretKeyMode { mode: 0o644, .. }),
        "{error}"
    );
    assert!(error.to_string().contains("0644"), "{error}");

    fs::set_permissions(&secret_path, fs::Permissions::from_mode(0o600)).unwrap();
    Instance::establish(data_dir.path(), storage.db())
        .await
        .unwrap();

    storage.close().await.unwrap();
    test.finish().await;
}

#[tokio::test]
async fn a_lost_instance_id_file_is_restored_from_the_database_and_a_foreign_one_is_refused() {
    let test = TestDatabase::create().await;
    let storage = test.storage().await;
    storage.migrate().await.unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let first = Instance::establish(data_dir.path(), storage.db())
        .await
        .unwrap();

    let id_path = data_dir.path().join(INSTANCE_ID_FILE);
    fs::remove_file(&id_path).unwrap();
    let restored = Instance::establish(data_dir.path(), storage.db())
        .await
        .unwrap();
    assert_eq!(restored.id(), first.id());
    assert_eq!(
        fs::read_to_string(&id_path).unwrap().trim(),
        first.id().to_string()
    );

    let foreign = keel_types::Id::generate();
    fs::write(&id_path, format!("{foreign}\n")).unwrap();
    let error = Instance::establish(data_dir.path(), storage.db())
        .await
        .unwrap_err();
    assert!(
        matches!(error, InstanceError::InstanceIdMismatch { .. }),
        "{error}"
    );
    assert!(error.to_string().contains(&foreign.to_string()), "{error}");
    assert!(
        error.to_string().contains(&first.id().to_string()),
        "{error}"
    );

    storage.close().await.unwrap();
    test.finish().await;
}

const SPKI_PREFIX: &str = "ed25519:MCowBQYDK2VwAyEA";

macro_rules! contributing {
    ($module:ident, $init:ident, $id:literal, $product:ident, $i18n:literal, $source:expr) => {
        fn $init(r: &mut Registrar) -> keel_sdk::Result<()> {
            r.contribute($source);
            Ok(())
        }

        static $module: ModuleDef = ModuleDef {
            id: $id,
            version: "0.1.0",
            sdk: keel_sdk::VERSION,
            requires: &[],
            migrations: None,
            init: $init,
        };

        static $product: ProductManifest = ProductManifest {
            id: $id,
            name: concat!("product.", $id, ".brand"),
            version: "0.1.0",
            api: keel_types::DESCRIPTOR_VERSION,
            brand: Brand {
                logo: concat!("product.", $id, ".logo"),
                hue: 200,
                chroma: 0.1,
            },
            modules: &[&$module],
            bridges: &[],
            menu: &[],
            levels: LevelLabels {
                labels: &[],
                disabled: &[],
            },
            account_kinds: &["user"],
            editions: &[Edition {
                id: "lite",
                features: &[],
            }],
            i18n: &[("en", $i18n)],
            assets: &[],
        };
    };
}

contributing!(
    SKY_MODULE,
    sky_init,
    "sky",
    SKY,
    r#"{ "product": { "sky": { "brand": "Sky" } } }"#,
    InstanceInfoSource::new()
        .feature("tasks")
        .feature("events")
        .service_kind("sky.zones")
);
contributing!(
    SEA_MODULE,
    sea_init,
    "sea",
    SEA,
    r#"{ "product": { "sea": { "brand": "Sea" } } }"#,
    InstanceInfoSource::new()
        .feature("events")
        .service_kind("sea.boxes")
);
contributing!(
    MOON_MODULE,
    moon_init,
    "moon",
    MOON,
    r#"{ "product": { "moon": { "brand": "Moon" } } }"#,
    InstanceInfoSource::new()
        .feature("craters")
        .service_kind("moon.craters")
);

fn relay_init(r: &mut Registrar) -> keel_sdk::Result<()> {
    r.contribute(InstanceInfoSource::new().feature("peers"));
    Ok(())
}

static RELAY: ModuleDef = ModuleDef {
    id: "relay",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: relay_init,
};

#[tokio::test]
async fn the_handshake_answers_the_instance_key_products_and_contributed_features() {
    let harness = Harness::new()
        .module(&RELAY)
        .product(&SKY)
        .product(&SEA)
        .product(&MOON)
        .enabled(["sky", "sea"])
        .boot()
        .await;
    let http = harness.http().await;

    let answer = http.get(Who::Guest, "/api/v1/instance").await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    let raw: Value = serde_json::from_str(&answer.body).unwrap();
    let mut members: Vec<&str> = raw
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    members.sort_unstable();
    assert_eq!(
        members,
        [
            "features",
            "instanceId",
            "keel",
            "products",
            "protocol",
            "publicKey",
            "serviceKinds"
        ]
    );
    let info: InstanceInfo = serde_json::from_value(raw).unwrap();
    assert_eq!(
        info,
        InstanceInfo {
            instance_id: harness.instance(),
            keel: harness.registry().platform().version.clone(),
            protocol: PROTOCOL,
            products: vec!["sea".to_owned(), "sky".to_owned()],
            public_key: harness.instance_key().public_key(),
            features: vec!["events".to_owned(), "peers".to_owned(), "tasks".to_owned()],
            service_kinds: vec!["sea.boxes".to_owned(), "sky.zones".to_owned()],
        }
    );
    assert!(
        info.public_key.starts_with(SPKI_PREFIX),
        "{}",
        info.public_key
    );

    let signed_in = http.get(Who::Admin, "/api/v1/instance").await;
    assert_eq!(signed_in.status, 200);
    assert_eq!(
        serde_json::from_str::<InstanceInfo>(&signed_in.body).unwrap(),
        info
    );

    drop(http);
    harness.finish().await;
}

#[tokio::test]
async fn the_instance_key_survives_a_restart_and_is_sealed() {
    let test = TestDatabase::create().await;
    let storage = test.storage().await;
    storage.migrate().await.unwrap();
    let data_dir = tempfile::tempdir().unwrap();

    let first = Instance::establish(data_dir.path(), storage.db())
        .await
        .unwrap();
    let stored = kv(&storage, KV_INSTANCE_KEY).await.unwrap();
    let public_key = first.instance_key().public_key();
    assert!(public_key.starts_with(SPKI_PREFIX), "{public_key}");
    assert_eq!(
        format!("{:?}", first.instance_key()),
        format!("InstanceKey({public_key})")
    );

    let second = Instance::establish(data_dir.path(), storage.db())
        .await
        .unwrap();
    assert_eq!(second.instance_key().public_key(), public_key);
    assert_eq!(kv(&storage, KV_INSTANCE_KEY).await, Some(stored.clone()));

    let sealed: Sealed = serde_json::from_value(stored).unwrap();
    assert_eq!(sealed.alg, "A256GCM");
    let seed: [u8; 32] = first.key().open(&sealed).unwrap().try_into().unwrap();
    let published = STANDARD
        .decode(public_key.strip_prefix("ed25519:").unwrap())
        .unwrap();
    assert_eq!(published.len(), 44);
    assert_eq!(
        &published[12..],
        SigningKey::from_bytes(&seed).verifying_key().as_bytes()
    );
    let file_key =
        Key::parse_hex(&fs::read_to_string(data_dir.path().join(SECRET_KEY_FILE)).unwrap())
            .unwrap();
    assert!(file_key.open(&sealed).is_err());

    storage.close().await.unwrap();
    test.finish().await;
}
