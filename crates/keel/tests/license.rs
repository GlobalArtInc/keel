use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use keel::sdk::prelude::*;
use keel::types::Envelope;
use keel::{Config, ExitCode, Runtime};
use keel_kernel::dispatcher::CallOptions;
use keel_kernel::socket::Client;
use keel_system::license::{GET, MODULE};
use keel_testkit::TestDatabase;
use keel_testkit::license::Issuer;
use tempfile::TempDir;

const AUDITED: &str = "SELECT function || ':' || coalesce(error_code, '') FROM audit_call WHERE function = 'license.install'";

struct Site {
    database: TestDatabase,
    dir: TempDir,
    config: Config,
    config_path: PathBuf,
}

async fn site() -> Site {
    let database = TestDatabase::create().await;
    let dir = tempfile::tempdir().expect("a temporary directory");
    let mut config = database.config();
    config.listen = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    config.socket = dir.path().join("run").join("core.sock");
    config.data_dir = dir.path().join("lib");
    let config_path = dir.path().join("config.yaml");
    std::fs::write(
        &config_path,
        format!(
            "listen: {}\nsocket: {}\ndata_dir: {}\ndatabase: {}\nlog: {{ level: warn, format: text }}\n",
            config.listen,
            config.socket.display(),
            config.data_dir.display(),
            config.database.expose(),
        ),
    )
    .expect("the config file is written");
    config.file = config_path.clone();
    Site {
        database,
        dir,
        config,
        config_path,
    }
}

async fn license(socket: &std::path::Path) -> Value {
    let mut client = Client::connect(socket)
        .await
        .expect("the socket accepts the owner");
    match client
        .call(GET, json!({}), CallOptions::default())
        .await
        .expect("the socket answers")
    {
        Envelope::Ok { data, .. } => data,
        other => panic!("license.get answers {other:?}"),
    }
}

#[tokio::test]
async fn keel_license_install_sends_the_file_over_the_socket_and_the_compiled_key_refuses_a_foreign_one()
 {
    let site = site().await;
    let runtime = Runtime::new()
        .module(&keel_system::audit::MODULE)
        .module(&MODULE);
    let running = runtime.start(&site.config).await.expect("keel starts");
    let instance = std::fs::read_to_string(site.config.data_dir.join("instance.id"))
        .expect("the instance id")
        .trim()
        .to_owned();
    let before = license(running.socket()).await;
    assert_eq!(before["state"], json!("none"));
    assert_eq!(before["instanceId"], json!(instance));

    let issuer = Issuer::generate().expect("a throwaway license key");
    let lease = site.dir.path().join("license.lease");
    std::fs::write(
        &lease,
        issuer
            .sign(&json!({
                "instance_id": instance,
                "issued_at": "2026-09-25T00:00:00Z",
                "expires_at": "2027-09-25T00:00:00Z",
                "grace_days": 30,
                "products": [],
            }))
            .expect("the lease is signed"),
    )
    .expect("the lease file is written");
    let args = vec![
        "keel".to_owned(),
        "--config".to_owned(),
        site.config_path.display().to_string(),
        "license".to_owned(),
        "install".to_owned(),
        lease.display().to_string(),
    ];
    let cli = runtime.clone();
    let exit = tokio::task::spawn_blocking(move || cli.run_from(args))
        .await
        .expect("the command finishes");
    assert_eq!(exit, ExitCode::FAILURE);
    assert_eq!(license(running.socket()).await, before);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while site.database.rows(AUDITED).await != ["license.install:validation"] {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the refused install is not audited"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let missing = vec![
        "keel".to_owned(),
        "license".to_owned(),
        "install".to_owned(),
        site.dir.path().join("missing.lease").display().to_string(),
        "--socket".to_owned(),
        site.config.socket.display().to_string(),
    ];
    let exit = tokio::task::spawn_blocking(move || runtime.run_from(missing))
        .await
        .expect("the command finishes");
    assert_eq!(exit, ExitCode::FAILURE);

    running.stop().await.expect("keel stops");
    drop(site.dir);
    site.database.finish().await;
}
