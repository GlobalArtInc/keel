use std::fs;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use keel::sdk::sea_orm::{ConnectionTrait, DbBackend, Statement};
use keel::{ExitCode, Runtime};
use keel_testkit::{TestDatabase, tokio};

const ZONE_FILE: &str = "keel-dns-example.org.zone";
const ZONE_TEXT: &str = "$ORIGIN example.org.\n@ IN NS ns1.example.net.\n";

fn runtime() -> Runtime {
    Runtime::new()
        .module(&keel_system::tasks::MODULE)
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .module(&keel_system::stats::MODULE)
        .product(&keel_dns::MANIFEST)
}

fn entry_of(archive: &std::path::Path, wanted: &str) -> Option<String> {
    let file = fs::File::open(archive).expect("the archive opens");
    let decoder = zstd::Decoder::new(file).expect("a zstd stream");
    let mut tar = tar::Archive::new(decoder);
    for entry in tar.entries().expect("entries") {
        let mut entry = entry.expect("an entry");
        if entry.path().expect("a path").to_str() == Some(wanted) {
            let mut text = String::new();
            entry.read_to_string(&mut text).expect("the entry reads");
            return Some(text);
        }
    }
    None
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dns_backup_taken_without_the_service_started_carries_the_zone_files() {
    let database = TestDatabase::create().await;
    let dir = tempfile::tempdir().expect("a temporary directory");
    let zones = dir.path().join("zones");
    fs::create_dir(&zones).expect("the zones directory");
    fs::write(zones.join(ZONE_FILE), ZONE_TEXT).expect("a zone file");
    let mut config = database.config();
    config.listen = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    config.socket = dir.path().join("run").join("core.sock");
    config.data_dir = dir.path().join("lib");
    let etc = dir.path().join("etc");
    fs::create_dir_all(&etc).expect("the config directory");
    let config_path = etc.join("config.yaml");
    fs::write(
        &config_path,
        format!(
            "listen: {}\nsocket: {}\ndata_dir: {}\ndatabase: {}\nlog: {{ level: warn, format: text }}\ndns:\n  zones_dir: {}\n",
            config.listen,
            config.socket.display(),
            config.data_dir.display(),
            config.database.expose(),
            zones.display(),
        ),
    )
    .expect("the config file is written");
    config.file = config_path.clone();

    let runtime = runtime();
    runtime
        .enable_product(&config, "dns")
        .await
        .expect("the product is enabled");
    runtime.migrate(&config).await.expect("the schema migrates");
    let storage = database.storage().await;
    for sql in [
        "INSERT INTO keel_account (id, kind, path, created_at, updated_at) VALUES ('acc', 'user', '/acc', now(), now())",
        "INSERT INTO dns_zone (id, account_id, name, serial, refresh, retry, expire, minimum, state, served, created_at, updated_at) VALUES ('z1', 'acc', 'example.org', 1, 10800, 3600, 604800, 3600, 'active', true, now(), now())",
    ] {
        storage
            .db()
            .execute(Statement::from_string(DbBackend::Postgres, sql))
            .await
            .expect("a row is inserted");
    }
    storage.close().await.expect("the storage closes");

    let archive = dir.path().join("dns.tar.zst");
    let args = [
        "keel".to_owned(),
        "--config".to_owned(),
        config_path.display().to_string(),
        "backup".to_owned(),
        "--out".to_owned(),
        archive.display().to_string(),
    ];
    let code = tokio::task::spawn_blocking(move || runtime.run_from(args))
        .await
        .expect("the command finishes");
    assert_eq!(code, ExitCode::SUCCESS);
    assert_eq!(
        entry_of(&archive, &format!("sources/dns.zonefiles/{ZONE_FILE}")).as_deref(),
        Some(ZONE_TEXT)
    );
    drop(dir);
    database.finish().await;
}
