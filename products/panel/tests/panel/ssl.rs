use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use keel::prelude::*;
use keel::sdk::chrono::{DateTime, Datelike, Days, Utc};
use keel::sdk::system::SecretCipher;
use keel::{ExitCode, Runtime};
use keel_kernel::socket::Socket;
use keel_kernel::{Log, LogFormat, logging, support};
use keel_testkit::{Harness, Who, tokio};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

use crate::mail::{domain_of_a1, mail};
use crate::support::{Site, composed, create, data, eventually, rows, run_job, state_becomes};

const DIRECTORY_VARIABLE: &str = "KEEL_TEST_ACME_URL";
const MANAGEMENT_VARIABLE: &str = "KEEL_TEST_ACME_MANAGEMENT";
const IMAGE: &str = "ghcr.io/letsencrypt/pebble";
const TAG: &str = "2.10.1";
const DIRECTORY_PORT: u16 = 14000;
const MANAGEMENT_PORT: u16 = 15000;
const MINICA: &str = "tests/panel/pebble-minica.pem";
const DOMAIN: &str = "shop.example.net";
const ALIAS: &str = "www.shop.example.net";
const RENEW_JOB: &str = "hosting.certificate.renew";
const POLL_JOB: &str = "notify.problems.poll";

struct Pebble {
    directory: String,
    management: String,
    _container: Option<ContainerAsync<GenericImage>>,
}

async fn pebble() -> Pebble {
    if let (Ok(directory), Ok(management)) = (
        std::env::var(DIRECTORY_VARIABLE),
        std::env::var(MANAGEMENT_VARIABLE),
    ) {
        return Pebble {
            directory,
            management: management.trim_end_matches('/').to_owned(),
            _container: None,
        };
    }
    let container = GenericImage::new(IMAGE, TAG)
        .with_exposed_port(ContainerPort::Tcp(DIRECTORY_PORT))
        .with_exposed_port(ContainerPort::Tcp(MANAGEMENT_PORT))
        .with_wait_for(WaitFor::message_on_either_std("ACME directory available"))
        .with_env_var("PEBBLE_VA_ALWAYS_VALID", "1")
        .with_env_var("PEBBLE_VA_NOSLEEP", "1")
        .with_env_var("PEBBLE_WFE_NONCEREJECT", "0")
        .start()
        .await
        .unwrap_or_else(|error| {
            panic!("{DIRECTORY_VARIABLE} is not set and the {IMAGE}:{TAG} container could not be started: {error}")
        });
    let directory = container
        .get_host_port_ipv4(DIRECTORY_PORT)
        .await
        .expect("the directory port");
    let management = container
        .get_host_port_ipv4(MANAGEMENT_PORT)
        .await
        .expect("the management port");
    Pebble {
        directory: format!("https://127.0.0.1:{directory}/dir"),
        management: format!("https://127.0.0.1:{management}"),
        _container: Some(container),
    }
}

fn minica() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(MINICA)
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn roots(pem: &[u8]) -> rustls::RootCertStore {
    let mut roots = rustls::RootCertStore::empty();
    for certificate in CertificateDer::pem_slice_iter(pem) {
        roots
            .add(certificate.expect("a PEM certificate"))
            .expect("a trust anchor");
    }
    roots
}

fn client(roots: rustls::RootCertStore) -> Arc<rustls::ClientConfig> {
    Arc::new(
        rustls::ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .expect("the protocol versions")
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}

fn issuing_root(management: &str) -> Vec<u8> {
    let authority = management
        .strip_prefix("https://")
        .expect("an https management address");
    let trusted = fs::read(minica()).expect("the pebble test authority");
    let connection = rustls::ClientConnection::new(
        client(roots(&trusted)),
        ServerName::try_from("127.0.0.1").expect("a server name"),
    )
    .expect("a client connection");
    let socket = TcpStream::connect(authority).expect("the management port answers");
    let mut stream = rustls::StreamOwned::new(connection, socket);
    stream
        .write_all(
            format!("GET /roots/0 HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .expect("the request is sent");
    let mut answer = Vec::new();
    match stream.read_to_end(&mut answer) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {}
        Err(error) => panic!("the management answer: {error}"),
    }
    let text = String::from_utf8(answer).expect("a text answer");
    let (_, body) = text.split_once("\r\n\r\n").expect("a body");
    body.as_bytes().to_vec()
}

fn served_leaf(chain: &Path, key: &Path, root: &[u8], name: &str) -> Vec<u8> {
    let certificates: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(chain)
        .expect("the chain file")
        .map(|certificate| certificate.expect("a certificate"))
        .collect();
    let key = PrivateKeyDer::from_pem_file(key).expect("the key file");
    let server = Arc::new(
        rustls::ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .expect("the protocol versions")
            .with_no_client_auth()
            .with_single_cert(certificates, key)
            .expect("the files are a pair"),
    );
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
    let address = listener.local_addr().expect("the address");
    let serving = std::thread::spawn(move || {
        let (socket, _) = listener.accept().expect("a client");
        let connection = rustls::ServerConnection::new(server).expect("a server connection");
        let mut stream = rustls::StreamOwned::new(connection, socket);
        stream.write_all(b"ok").expect("the greeting");
        stream.flush().expect("the greeting is flushed");
        let mut rest = [0_u8; 1];
        let _ = stream.read(&mut rest);
    });
    let connection = rustls::ClientConnection::new(
        client(roots(root)),
        ServerName::try_from(name.to_owned()).expect("a server name"),
    )
    .expect("a client connection");
    let socket = TcpStream::connect(address).expect("the server answers");
    let mut stream = rustls::StreamOwned::new(connection, socket);
    let mut greeting = [0_u8; 2];
    stream
        .read_exact(&mut greeting)
        .expect("the handshake completes against the issuing root");
    assert_eq!(&greeting, b"ok");
    let leaf = stream
        .conn
        .peer_certificates()
        .and_then(|chain| chain.first())
        .expect("the served leaf")
        .to_vec();
    drop(stream);
    serving.join().expect("the server thread");
    leaf
}

async fn panel(site: &Site, pebble: Option<&Pebble>) -> Harness {
    let harness = composed(site)
        .section(
            "hosting_certificate",
            json!({
                "certs_dir": site.path("certs").display().to_string(),
                "challenges_dir": site.path("acme").display().to_string(),
                "directory_ca": minica().display().to_string(),
            }),
        )
        .boot()
        .await;
    if let Some(pebble) = pebble {
        data(
            &harness
                .call(
                    Who::Admin,
                    "settings.set",
                    json!({ "key": "hosting.certificate.directory", "value": pebble.directory }),
                )
                .await,
        );
    }
    harness
}

async fn site_for(harness: &Harness, lets_encrypt: bool) -> String {
    let user = create(harness, Who::A1, "shop").await;
    state_becomes(harness, &user, "active").await;
    let reply = harness
        .call(
            Who::A1,
            "hosting.site.create",
            json!({
                "user": user,
                "domain": DOMAIN,
                "aliases": [ALIAS],
                "letsEncrypt": lets_encrypt,
            }),
        )
        .await;
    data(&reply)["id"].as_str().expect("a site id").to_owned()
}

async fn certificate(harness: &Harness) -> (String, String, DateTime<Utc>) {
    let sql = "SELECT state || ' ' || coalesce(installed, '-') || ' ' || coalesce(to_char(not_after AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"'), '-') || ' ' || coalesce(error, '-') FROM hosting_certificate";
    let row = rows(harness, sql).await;
    let row = row.first().expect("a certificate row");
    let mut parts = row.splitn(4, ' ');
    let state = parts.next().unwrap_or_default().to_owned();
    let installed = parts.next().unwrap_or_default().to_owned();
    let not_after = parts
        .next()
        .and_then(|text| text.parse().ok())
        .unwrap_or_else(|| panic!("the certificate is not installed: {row}"));
    (state, installed, not_after)
}

async fn installed_becomes_other_than(harness: &Harness, site: &Site, previous: &str) -> String {
    let sql = "SELECT state || ' ' || coalesce(installed, '-') || ' ' || coalesce(error, '-') FROM hosting_certificate";
    let mut seen = String::new();
    eventually("a certificate is installed", async || {
        seen = rows(harness, sql).await.join("\n");
        let mut parts = seen.split(' ');
        assert_ne!(parts.next(), Some("error"), "{seen}");
        let installed = parts.next().unwrap_or("-");
        installed != "-" && installed != previous
    })
    .await;
    let stem = seen.split(' ').nth(1).unwrap_or_default().to_owned();
    let replaced = site.path("certs").join(format!("{previous}.key"));
    eventually("the server block serves the installed pair", async || {
        fs::read_to_string(site.vhost(DOMAIN))
            .is_ok_and(|vhost| vhost.contains(&format!("{stem}.crt;")))
            && !replaced.exists()
    })
    .await;
    stem
}

async fn issued(harness: &Harness) -> u64 {
    rows(
        harness,
        "SELECT count(*)::text FROM task_job WHERE function = 'hosting.certificate.issue'",
    )
    .await
    .first()
    .and_then(|count| count.parse().ok())
    .expect("a count")
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).expect("the file").permissions().mode() & 0o777
}

#[tokio::test]
async fn a_site_gets_a_certificate_over_http01_and_serves_tls() {
    let pebble = pebble().await;
    let site = Site::new();
    let harness = panel(&site, Some(&pebble)).await;
    site_for(&harness, true).await;
    let stem = installed_becomes_other_than(&harness, &site, "-").await;
    let (state, installed, not_after) = certificate(&harness).await;
    assert_eq!(
        (state.as_str(), installed.as_str()),
        ("active", stem.as_str())
    );
    assert!(not_after > harness.clock().now(), "{not_after}");
    assert_eq!(
        rows(
            &harness,
            "SELECT kind || ' ' || domain FROM hosting_certificate"
        )
        .await,
        [format!("acme {DOMAIN}")]
    );

    let chain = site.path("certs").join(format!("{stem}.crt"));
    let key = site.path("certs").join(format!("{stem}.key"));
    assert_eq!(mode(&chain), 0o644);
    assert_eq!(mode(&key), 0o600);
    let vhost = fs::read_to_string(site.vhost(DOMAIN)).expect("the server block");
    let certs = fs::canonicalize(site.path("certs")).expect("the certs root");
    let acme = fs::canonicalize(site.path("acme")).expect("the challenge root");
    for line in [
        "    listen 443 ssl;\n".to_owned(),
        format!(
            "    ssl_certificate {};\n",
            site.path("certs").join(format!("{stem}.crt")).display()
        ),
        format!(
            "    ssl_certificate_key {};\n",
            site.path("certs").join(format!("{stem}.key")).display()
        ),
        "    location ~ \"^/\\.well-known/acme-challenge/([-_A-Za-z0-9]+)$\" {\n".to_owned(),
        format!(
            "        alias {}/keel-panel-$1;\n",
            site.path("acme").display()
        ),
    ] {
        assert!(vhost.contains(&line), "{line} in {vhost}");
    }
    assert!(certs.join(format!("{stem}.crt")).is_file());
    assert_eq!(
        fs::read_dir(&acme).expect("the challenge root").count(),
        0,
        "the challenge files are removed once the order is valid"
    );
    let calls = site.calls();
    assert!(calls.iter().any(|call| call == "nginx -t"), "{calls:?}");
    assert!(
        calls.iter().any(|call| call == "systemctl reload nginx"),
        "{calls:?}"
    );

    let sealed = rows(
        &harness,
        "SELECT credentials FROM hosting_certificate_account",
    )
    .await;
    assert_eq!(sealed.len(), 1);
    assert!(sealed[0].starts_with("{\"alg\""), "{}", sealed[0]);
    assert!(!sealed[0].contains("key_pkcs8"));

    let root = issuing_root(&pebble.management);
    let served = tokio::task::spawn_blocking(move || {
        [DOMAIN, ALIAS].map(|name| served_leaf(&chain, &key, &root, name))
    })
    .await
    .expect("the handshakes");
    assert_eq!(served[0], served[1]);
    harness.finish().await;
}

#[tokio::test]
async fn renewal_runs_thirty_days_before_expiry() {
    let pebble = pebble().await;
    let site = Site::new();
    let harness = panel(&site, Some(&pebble)).await;
    let site_id = site_for(&harness, false).await;
    data(
        &harness
            .call(
                Who::A1,
                "hosting.certificate.request",
                json!({ "site": site_id }),
            )
            .await,
    );
    let first = installed_becomes_other_than(&harness, &site, "-").await;
    let (_, _, not_after) = certificate(&harness).await;
    assert_eq!(issued(&harness).await, 1);

    harness.clock().set(not_after - Days::new(31));
    run_job(&harness, RENEW_JOB).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(issued(&harness).await, 1, "nothing is due 31 days ahead");
    assert_eq!(certificate(&harness).await.1, first);

    harness.clock().set(not_after - Days::new(29));
    run_job(&harness, RENEW_JOB).await;
    installed_becomes_other_than(&harness, &site, &first).await;
    eventually("the issuance tasks end", async || {
        rows(
            &harness,
            "SELECT count(*)::text FROM task_job WHERE function = 'hosting.certificate.issue' AND state IN ('queued', 'running')",
        )
        .await
            == ["0"]
    })
    .await;
    assert!(issued(&harness).await >= 2);
    let (state, current, _) = certificate(&harness).await;
    assert_eq!(state, "active");
    assert_ne!(current, first);
    let mut kept: Vec<String> = fs::read_dir(site.path("certs"))
        .expect("the certs root")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    kept.sort();
    assert_eq!(kept, [format!("{current}.crt"), format!("{current}.key")]);
    let vhost = fs::read_to_string(site.vhost(DOMAIN)).expect("the server block");
    assert!(vhost.contains(&format!("{current}.crt;")), "{vhost}");
    assert!(!vhost.contains(&first), "{vhost}");
    harness.finish().await;
}

#[tokio::test]
async fn an_uploaded_certificate_is_installed_and_its_near_expiry_is_a_problem() {
    let site = Site::new();
    let harness = panel(&site, None).await;
    let site_id = site_for(&harness, false).await;
    let expiry = harness.clock().now() + Days::new(10);
    let mut params = rcgen::CertificateParams::new(vec![DOMAIN.to_owned(), ALIAS.to_owned()])
        .expect("certificate parameters");
    params.not_after = rcgen::date_time_ymd(
        expiry.year(),
        u8::try_from(expiry.month()).expect("a month"),
        u8::try_from(expiry.day()).expect("a day"),
    );
    let key = rcgen::KeyPair::generate().expect("a key");
    let certificate = params.self_signed(&key).expect("a certificate");

    let refused = harness
        .call(
            Who::A1,
            "hosting.certificate.upload",
            json!({ "site": site_id, "certificate": certificate.pem(), "key": rcgen::KeyPair::generate().expect("a key").serialize_pem() }),
        )
        .await;
    let error = refused.error().expect("a mismatched key is refused");
    assert_eq!(
        error
            .fields
            .iter()
            .map(|field| (field.path.as_str(), field.key.as_deref()))
            .collect::<Vec<_>>(),
        [("key", Some("hosting.certificate.error.pair"))],
        "{error:?}"
    );

    data(
        &harness
            .call(
                Who::A1,
                "hosting.certificate.upload",
                json!({ "site": site_id, "certificate": certificate.pem(), "key": key.serialize_pem() }),
            )
            .await,
    );
    let stem = installed_becomes_other_than(&harness, &site, "-").await;
    assert_eq!(
        rows(
            &harness,
            "SELECT kind || ' ' || state FROM hosting_certificate"
        )
        .await,
        ["uploaded active"]
    );
    assert_eq!(mode(&site.path("certs").join(format!("{stem}.key"))), 0o600);
    let vhost = fs::read_to_string(site.vhost(DOMAIN)).expect("the server block");
    assert!(vhost.contains(&format!("{stem}.key;")), "{vhost}");

    run_job(&harness, POLL_JOB).await;
    assert_eq!(
        rows(
            &harness,
            "SELECT severity || ' ' || (params_json->>'domain') FROM notify_problem WHERE key = 'hosting.certificate.expiring' AND open"
        )
        .await,
        [format!("warning {DOMAIN}")]
    );

    let listed = harness
        .call(
            Who::A1,
            "hosting.certificate.list",
            json!({ "page": 1, "pageSize": 10 }),
        )
        .await;
    let certificate_id = data(&listed)["data"][0]["id"]
        .as_str()
        .expect("a certificate id")
        .to_owned();
    data(
        &harness
            .call(
                Who::A1,
                "hosting.certificate.delete",
                json!({ "ids": [certificate_id] }),
            )
            .await,
    );
    eventually("the key file is removed", async || {
        !site.path("certs").join(format!("{stem}.key")).exists()
    })
    .await;
    let vhost = fs::read_to_string(site.vhost(DOMAIN)).expect("the server block");
    assert!(!vhost.contains("listen 443 ssl;"), "{vhost}");
    harness.finish().await;
}

const MAILBOX_SECRET: &str = "mailbox-secret-5d1f0c";
const DATABASE_SECRET: &str = "database-secret-8c2e41";
const FTP_SECRET: &str = "ftp-secret-3a9b77";

async fn active_row(harness: &Harness, table: &str, id: &str) {
    let sql = format!("SELECT state FROM {table} WHERE id = '{id}'");
    eventually(&format!("{table} {id} active"), async || {
        rows(harness, &sql).await == ["active"]
    })
    .await;
}

async fn created_id(harness: &Harness, function: &str, params: Value) -> String {
    let reply = harness.call(Who::A1, function, params).await;
    data(&reply)["id"].as_str().expect("an id").to_owned()
}

async fn account_key(harness: &Harness) -> String {
    let sealed = rows(
        harness,
        "SELECT credentials FROM hosting_certificate_account",
    )
    .await;
    let sealed = sealed.first().expect("an ACME account");
    let cipher = harness
        .registry()
        .services()
        .get::<dyn SecretCipher>()
        .expect("the cipher");
    let credentials: Value =
        keel::sdk::serde_json::from_slice(&cipher.open(sealed).expect("the credentials open"))
            .expect("the credentials are JSON");
    credentials["key_pkcs8"]
        .as_str()
        .unwrap_or_else(|| {
            let fields: Vec<&String> = credentials
                .as_object()
                .map(|fields| fields.keys().collect())
                .unwrap_or_default();
            panic!("no key among the fields {fields:?}")
        })
        .to_owned()
}

fn bundle_layout(site: &Site) -> Vec<String> {
    let root = site.path("layout");
    fs::create_dir_all(root.join("opt")).expect("the opt directory");
    fs::create_dir_all(root.join("units")).expect("the unit directory");
    [
        ("--opt", root.join("opt")),
        ("--unit-dir", root.join("units")),
        ("--systemctl", root.join("systemctl")),
        ("--agent-socket", root.join("agent.sock")),
    ]
    .into_iter()
    .flat_map(|(flag, path)| [flag.to_owned(), path.display().to_string()])
    .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_support_bundle_carries_no_secret_value() {
    logging::init(&Log {
        level: "info".to_owned(),
        format: LogFormat::Json,
    })
    .expect("the log ring is installed");
    let pebble = pebble().await;
    let site = Site::new();
    let harness = mail(&site, composed(&site))
        .module(&keel_system::audit::MODULE)
        .section(
            "hosting_certificate",
            json!({
                "certs_dir": site.path("certs").display().to_string(),
                "challenges_dir": site.path("acme").display().to_string(),
                "directory_ca": minica().display().to_string(),
            }),
        )
        .section(
            "hosting_database",
            json!({ "dumps_dir": site.path("dumps").display().to_string() }),
        )
        .boot()
        .await;
    data(
        &harness
            .call(
                Who::Admin,
                "settings.set",
                json!({ "key": "hosting.certificate.directory", "value": pebble.directory }),
            )
            .await,
    );
    let web = site_for(&harness, true).await;
    installed_becomes_other_than(&harness, &site, "-").await;
    let key = account_key(&harness).await;
    let url = harness.database().config().database.expose().to_owned();

    let domain = domain_of_a1(&harness, DOMAIN).await;
    active_row(&harness, "hosting_mail_domain", &domain).await;
    let mailbox = created_id(
        &harness,
        "hosting.mail.mailbox.create",
        json!({ "domain": domain, "local": "info", "password": MAILBOX_SECRET }),
    )
    .await;
    active_row(&harness, "hosting_mailbox", &mailbox).await;
    let wp = created_id(
        &harness,
        "hosting.database.create",
        json!({ "name": "hp_shop_wp" }),
    )
    .await;
    active_row(&harness, "hosting_database", &wp).await;
    let login = created_id(
        &harness,
        "hosting.database.user.create",
        json!({ "name": "hp_shop_app", "password": DATABASE_SECRET, "databases": [wp] }),
    )
    .await;
    active_row(&harness, "hosting_database_user", &login).await;
    let ftp = created_id(
        &harness,
        "hosting.ftp.create",
        json!({ "site": web, "name": "upload", "password": FTP_SECRET, "directory": "" }),
    )
    .await;
    active_row(&harness, "hosting_ftp_user", &ftp).await;
    let changed = harness
        .call(
            Who::A1,
            "hosting.database.user.edit",
            json!({ "id": login, "password": format!("{DATABASE_SECRET}-2"), "databases": [wp] }),
        )
        .await;
    data(&changed);
    eventually("the audit trail of the password change", async || {
        !rows(
            &harness,
            "SELECT id::text FROM audit_call WHERE function = 'hosting.database.user.edit'",
        )
        .await
        .is_empty()
    })
    .await;

    let socket = site.path("run").join("core.sock");
    fs::create_dir_all(site.path("run")).expect("the run directory");
    let _serving = Socket::new(harness.dispatcher().clone())
        .start(&socket)
        .await
        .expect("the control socket serves");
    let listen = TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .expect("a free port");
    let config = site.path("config.yaml");
    fs::write(
        &config,
        format!(
            "listen: {listen}\nsocket: {}\ndata_dir: {}\ndatabase: {url}\nlog: {{ level: info, format: json }}\n",
            socket.display(),
            site.path("lib").display(),
        ),
    )
    .expect("the config file");
    let out = site.path("bundle.tar.zst");
    let mut args = vec![
        "keel".to_owned(),
        "--config".to_owned(),
        config.display().to_string(),
        "support-bundle".to_owned(),
        "--out".to_owned(),
        out.display().to_string(),
    ];
    args.extend(bundle_layout(&site));
    let runtime = Runtime::new()
        .module(&keel_system::settings::MODULE)
        .module(&keel_system::audit::MODULE)
        .module(&keel_system::tasks::MODULE)
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .module(&keel_system::stats::MODULE)
        .product(&keel_dns::MANIFEST)
        .product(&keel_panel::MANIFEST);
    let exit = tokio::task::spawn_blocking(move || runtime.run_from(args))
        .await
        .expect("the bundle command finishes");
    assert_eq!(exit, ExitCode::SUCCESS);

    let entries = support::read_entries(&out).expect("the bundle opens");
    let entry = |name: &str| {
        entries
            .iter()
            .find(|(entry, _)| entry == name)
            .map(|(_, bytes)| String::from_utf8_lossy(bytes).into_owned())
            .unwrap_or_else(|| panic!("{name} is in the bundle"))
    };
    assert!(entry("audit.jsonl").contains("hosting.mail.mailbox.create"));
    assert!(entry("audit.jsonl").contains("hosting.database.user.edit"));
    assert!(
        entry("logs.jsonl").contains("hosting"),
        "the ring carries the panel's work"
    );
    let secrets = [
        MAILBOX_SECRET.to_owned(),
        DATABASE_SECRET.to_owned(),
        FTP_SECRET.to_owned(),
        key.clone(),
        key[..32].to_owned(),
    ];
    for (name, bytes) in &entries {
        let text = String::from_utf8_lossy(bytes);
        for secret in &secrets {
            assert!(
                !text.contains(secret.as_str()),
                "{name} carries a secret value"
            );
        }
    }
    harness.finish().await;
}
