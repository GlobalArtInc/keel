use std::fs;
use std::os::unix::fs::PermissionsExt;

use keel_kernel::instance::{INSTANCE_ID_FILE, KV_INSTANCE_ID, KV_SECRET_KEY, SECRET_KEY_FILE};
use keel_kernel::secret::{Key, Sealed};
use keel_kernel::{Instance, InstanceError};
use keel_testkit::TestDatabase;
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
        2
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
