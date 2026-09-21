use std::fs;
use std::net::SocketAddr;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use keel::{ExitCode, Runtime};
use keel_release::{
    Channel, Digest, Manifest, SecretKey, Signed, Version, archive_path, manifest_path,
};
use tempfile::TempDir;

const KERNEL_UNIT: &str =
    "[Unit]\nDescription=keel\n[Service]\nExecStart=/opt/keel/bin/current serve\n";
const PRIVILEGED_UNIT: &str =
    "[Unit]\nDescription=keel privileged\n[Service]\nExecStart=/opt/keel/bin/current privileged\n";

struct Site {
    dir: TempDir,
    key: SecretKey,
}

impl Site {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        for sub in [
            "opt/versions",
            "opt/bin",
            "units",
            "lib",
            "channel/stable",
            "channel/beta",
        ] {
            fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        let key = SecretKey::generate("test").unwrap();
        key.public().write(&dir.path().join("release.pub")).unwrap();
        let log = dir.path().join("systemctl.log");
        let stub = dir.path().join("systemctl");
        fs::write(
            &stub,
            format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\n", log.display()),
        )
        .unwrap();
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
        Self { dir, key }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.dir.path().join(relative)
    }

    fn opt(&self) -> PathBuf {
        self.path("opt")
    }

    fn version_dir(&self, version: &str) -> PathBuf {
        self.opt().join("versions").join(version)
    }

    fn binary_of(&self, version: &str) -> PathBuf {
        self.version_dir(version).join("keel")
    }

    fn signed(
        &self,
        version: &str,
        contract: u32,
        min_upgrade_from: &str,
        archive: &[u8],
    ) -> Vec<u8> {
        self.signed_with(&self.key, version, contract, min_upgrade_from, archive)
    }

    fn signed_with(
        &self,
        key: &SecretKey,
        version: &str,
        contract: u32,
        min_upgrade_from: &str,
        archive: &[u8],
    ) -> Vec<u8> {
        let manifest = Manifest {
            version: Version::parse(version).unwrap(),
            sha256: Digest::of(archive).hex(),
            min_upgrade_from: Version::parse(min_upgrade_from).unwrap(),
            contract,
            notes: format!("release {version}"),
        };
        Signed::sign(&manifest, key)
            .unwrap()
            .to_json()
            .unwrap()
            .into_bytes()
    }

    fn install_current(&self, version: &str, contract: u32) {
        let dir = self.version_dir(version);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("keel"), format!("keel {version}\n")).unwrap();
        fs::write(
            dir.join("manifest.json"),
            self.signed(version, contract, "0.0.1", b"installed by hand"),
        )
        .unwrap();
        let link = self.opt().join("bin/current");
        if link.is_symlink() {
            fs::remove_file(&link).unwrap();
        }
        symlink(self.binary_of(version), link).unwrap();
    }

    fn archive(&self, version: &str, with_units: bool) -> Vec<u8> {
        let encoder = zstd::Encoder::new(Vec::new(), 3).unwrap();
        let mut builder = tar::Builder::new(encoder);
        let binary = format!("#!/bin/sh\necho keel {version}\n");
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o755);
        header.set_size(binary.len() as u64);
        builder
            .append_data(&mut header, "keel", binary.as_bytes())
            .unwrap();
        if with_units {
            for (name, content) in [
                ("keel.service", format!("{KERNEL_UNIT}# {version}\n")),
                (
                    "keel-privileged.service",
                    format!("{PRIVILEGED_UNIT}# {version}\n"),
                ),
                (
                    "keel-update.path",
                    format!("[Path]\nPathExists=/var/lib/keel/update.request\n# {version}\n"),
                ),
                ("README.md", format!("release {version}\n")),
            ] {
                let mut header = tar::Header::new_gnu();
                header.set_mode(0o644);
                header.set_size(content.len() as u64);
                builder
                    .append_data(
                        &mut header,
                        format!("deploy/systemd/{name}"),
                        content.as_bytes(),
                    )
                    .unwrap();
            }
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn publish(
        &self,
        version: &str,
        contract: u32,
        min_upgrade_from: &str,
        channel: Channel,
    ) -> Vec<u8> {
        let archive = self.archive(version, true);
        self.publish_bytes(
            version,
            contract,
            min_upgrade_from,
            channel,
            &archive,
            &self.key,
        );
        archive
    }

    fn publish_bytes(
        &self,
        version: &str,
        contract: u32,
        min_upgrade_from: &str,
        channel: Channel,
        archive: &[u8],
        key: &SecretKey,
    ) {
        let parsed = Version::parse(version).unwrap();
        let channel_dir = self.path("channel");
        let manifest = self.signed_with(key, version, contract, min_upgrade_from, archive);
        let archive_file = channel_dir.join(archive_path(&parsed));
        fs::create_dir_all(archive_file.parent().unwrap()).unwrap();
        fs::write(&archive_file, archive).unwrap();
        fs::write(channel_dir.join(manifest_path(&parsed)), &manifest).unwrap();
        fs::write(channel_dir.join(channel.head_path()), &manifest).unwrap();
    }

    fn request(&self, body: &str) {
        fs::write(self.path("lib/update.request"), body).unwrap();
    }

    fn run(&self, channel: &str, tail: &[&str]) -> ExitCode {
        let request = self.path("lib/update.request");
        let opt = self.opt();
        let units = self.path("units");
        let systemctl = self.path("systemctl");
        let key = self.path("release.pub");
        let mut args: Vec<String> = [
            "keel",
            "privileged",
            "update",
            "--request",
            request.to_str().unwrap(),
            "--channel-url",
            channel,
            "--opt",
            opt.to_str().unwrap(),
            "--unit-dir",
            units.to_str().unwrap(),
            "--systemctl",
            systemctl.to_str().unwrap(),
            "--release-key",
            key.to_str().unwrap(),
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        args.extend(tail.iter().map(|arg| (*arg).to_owned()));
        Runtime::new().run_from(args)
    }

    fn update(&self) -> ExitCode {
        self.run(self.path("channel").to_str().unwrap(), &["--from-request"])
    }

    fn rollback(&self, restore: Option<&Path>) -> ExitCode {
        let mut tail = vec!["rollback"];
        if let Some(archive) = restore {
            tail.push("--restore");
            tail.push(archive.to_str().unwrap());
        }
        self.run(self.path("channel").to_str().unwrap(), &tail)
    }

    fn current(&self) -> PathBuf {
        fs::read_link(self.opt().join("bin/current")).unwrap()
    }

    fn previous(&self) -> Option<PathBuf> {
        fs::read_link(self.opt().join("bin/previous")).ok()
    }

    fn systemctl_calls(&self) -> Vec<String> {
        fs::read_to_string(self.path("systemctl.log"))
            .map(|text| text.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    fn clear_systemctl_calls(&self) {
        let _ = fs::remove_file(self.path("systemctl.log"));
    }

    fn unit(&self, name: &str) -> Option<String> {
        fs::read_to_string(self.path("units").join(name)).ok()
    }
}

fn restart_sequence() -> Vec<String> {
    [
        "daemon-reload",
        "restart keel-privileged.service",
        "restart keel.service",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

#[test]
fn an_update_request_installs_the_signed_version_switches_current_and_restarts() {
    let site = Site::new();
    site.install_current("0.1.0", 1);
    let archive = site.publish("0.2.0", 1, "0.1.0", Channel::Stable);
    site.request(r#"{"version":"0.2.0","channel":"stable","requested_by":"admin"}"#);

    assert_eq!(site.update(), ExitCode::SUCCESS);

    assert_eq!(site.current(), site.binary_of("0.2.0"));
    assert_eq!(site.previous(), Some(site.binary_of("0.1.0")));
    assert_eq!(
        fs::read_to_string(site.binary_of("0.2.0")).unwrap(),
        "#!/bin/sh\necho keel 0.2.0\n"
    );
    assert_eq!(
        fs::metadata(site.binary_of("0.2.0"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    let installed_manifest = fs::read(site.version_dir("0.2.0").join("manifest.json")).unwrap();
    assert_eq!(
        installed_manifest,
        fs::read(site.path("channel/versions/0.2.0/manifest.json")).unwrap()
    );
    assert_eq!(Digest::of(&archive).hex(), {
        let signed = Signed::parse(&installed_manifest).unwrap();
        let ring = keel_release::KeyRing::new(site.key.public(), Vec::new());
        signed.verify(&ring).unwrap().sha256
    });
    assert_eq!(
        site.unit("keel.service").unwrap(),
        format!("{KERNEL_UNIT}# 0.2.0\n")
    );
    assert_eq!(
        site.unit("keel-privileged.service").unwrap(),
        format!("{PRIVILEGED_UNIT}# 0.2.0\n")
    );
    assert!(site.unit("keel-update.path").is_some());
    assert!(site.unit("README.md").is_none());
    assert_eq!(site.systemctl_calls(), restart_sequence());
    assert!(!site.path("lib/update.request").exists());
    assert!(
        fs::read_dir(site.opt().join("versions"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".update-"))
    );
    assert!(site.version_dir("0.1.0").join("keel").is_file());
}

#[test]
fn a_request_naming_an_unsigned_or_tampered_version_leaves_current_untouched() {
    let site = Site::new();
    site.install_current("0.1.0", 1);
    let archive = site.archive("0.2.0", true);
    let other = SecretKey::generate("other").unwrap();
    site.publish_bytes("0.2.0", 1, "0.1.0", Channel::Stable, &archive, &other);
    site.request(r#"{"version":"0.2.0","channel":"stable","requested_by":"admin"}"#);
    assert_eq!(site.update(), ExitCode::FAILURE);
    assert_eq!(site.current(), site.binary_of("0.1.0"));
    assert_eq!(site.previous(), None);
    assert!(!site.version_dir("0.2.0").exists());
    assert!(site.systemctl_calls().is_empty());
    assert!(!site.path("lib/update.request").exists());

    site.publish_bytes("0.2.0", 1, "0.1.0", Channel::Stable, &archive, &site.key);
    let tampered = site.path("channel/versions/0.2.0/keel-0.2.0-x86_64-linux-musl.tar.zst");
    fs::write(&tampered, site.archive("0.2.0", false)).unwrap();
    site.request(r#"{"version":"0.2.0","channel":"stable","requested_by":"admin"}"#);
    assert_eq!(site.update(), ExitCode::FAILURE);
    assert_eq!(site.current(), site.binary_of("0.1.0"));
    assert!(!site.version_dir("0.2.0").exists());
    assert!(site.systemctl_calls().is_empty());

    fs::write(&tampered, &archive).unwrap();
    let manifest_file = site.path("channel/versions/0.2.0/manifest.json");
    let mut envelope: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_file).unwrap()).unwrap();
    let mut payload = envelope["payload"].as_str().unwrap().to_owned();
    let flipped = if payload.starts_with('e') { 'f' } else { 'e' };
    payload.replace_range(0..1, &flipped.to_string());
    envelope["payload"] = serde_json::Value::String(payload);
    fs::write(&manifest_file, envelope.to_string()).unwrap();
    site.request(r#"{"version":"0.2.0","channel":"stable","requested_by":"admin"}"#);
    assert_eq!(site.update(), ExitCode::FAILURE);
    assert_eq!(site.current(), site.binary_of("0.1.0"));
    assert!(site.systemctl_calls().is_empty());

    site.publish("0.1.5", 1, "0.1.0", Channel::Beta);
    site.publish("0.2.0", 1, "0.1.0", Channel::Stable);
    site.publish("0.3.0", 1, "0.2.0", Channel::Stable);
    site.request(r#"{"version":"0.1.5","channel":"stable","requested_by":"admin"}"#);
    assert_eq!(site.update(), ExitCode::FAILURE);
    assert_eq!(site.current(), site.binary_of("0.1.0"));
    assert!(!site.version_dir("0.1.5").exists());
    assert!(site.systemctl_calls().is_empty());

    site.request(r#"{"version":"0.2.0","channel":"stable","requested_by":"admin"}"#);
    assert_eq!(site.update(), ExitCode::SUCCESS);
    assert_eq!(site.current(), site.binary_of("0.2.0"));
    assert_eq!(site.systemctl_calls(), restart_sequence());
}

#[test]
fn rollback_switches_to_the_previous_version_within_the_contract_and_refuses_across_it() {
    let site = Site::new();
    site.install_current("0.1.0", 1);
    assert_eq!(site.rollback(None), ExitCode::FAILURE);
    assert_eq!(site.current(), site.binary_of("0.1.0"));

    site.publish("0.2.0", 1, "0.1.0", Channel::Stable);
    site.request(r#"{"version":"0.2.0","channel":"stable","requested_by":"admin"}"#);
    assert_eq!(site.update(), ExitCode::SUCCESS);
    site.clear_systemctl_calls();

    assert_eq!(site.rollback(None), ExitCode::SUCCESS);
    assert_eq!(site.current(), site.binary_of("0.1.0"));
    assert_eq!(site.previous(), Some(site.binary_of("0.2.0")));
    assert_eq!(site.systemctl_calls(), restart_sequence());
    assert!(site.version_dir("0.2.0").join("keel").is_file());
    site.clear_systemctl_calls();

    site.publish("1.0.0", 2, "0.2.0", Channel::Stable);
    site.request(r#"{"version":"1.0.0","channel":"stable","requested_by":"admin"}"#);
    assert_eq!(site.update(), ExitCode::FAILURE);
    assert_eq!(site.current(), site.binary_of("0.1.0"));
    site.request(r#"{"version":"0.2.0","channel":"stable","requested_by":"admin"}"#);
    assert_eq!(site.update(), ExitCode::SUCCESS);
    assert_eq!(site.current(), site.binary_of("0.2.0"));
    site.request(r#"{"version":"1.0.0","channel":"stable","requested_by":"admin"}"#);
    assert_eq!(site.update(), ExitCode::SUCCESS);
    assert_eq!(site.current(), site.binary_of("1.0.0"));
    assert_eq!(site.previous(), Some(site.binary_of("0.2.0")));
    site.clear_systemctl_calls();

    assert_eq!(site.rollback(None), ExitCode::FAILURE);
    assert_eq!(site.current(), site.binary_of("1.0.0"));
    assert_eq!(site.previous(), Some(site.binary_of("0.2.0")));
    assert!(site.systemctl_calls().is_empty());

    let missing = site.path("lib/backups/none.tar.zst");
    assert_eq!(site.rollback(Some(&missing)), ExitCode::FAILURE);
    assert_eq!(site.current(), site.binary_of("1.0.0"));
    assert!(site.systemctl_calls().is_empty());

    let kept = fs::read(site.binary_of("0.2.0")).unwrap();
    fs::remove_file(site.binary_of("0.2.0")).unwrap();
    assert_eq!(site.rollback(None), ExitCode::FAILURE);
    assert_eq!(site.current(), site.binary_of("1.0.0"));
    assert!(site.systemctl_calls().is_empty());
    fs::write(site.binary_of("0.2.0"), kept).unwrap();

    let backup = site.path("lib/backups/pre-update.tar.zst");
    fs::create_dir_all(backup.parent().unwrap()).unwrap();
    fs::write(&backup, b"backup").unwrap();
    assert_eq!(site.rollback(Some(&backup)), ExitCode::SUCCESS);
    assert_eq!(site.current(), site.binary_of("0.2.0"));
    assert_eq!(site.previous(), Some(site.binary_of("1.0.0")));
    assert_eq!(
        site.systemctl_calls(),
        [
            "daemon-reload",
            "restart keel-privileged.service",
            "stop keel.service"
        ]
    );
    assert_eq!(
        site.unit("keel.service").unwrap(),
        format!("{KERNEL_UNIT}# 0.2.0\n")
    );
}

#[test]
fn the_request_carries_no_command_the_root_side_obeys() {
    let site = Site::new();
    site.install_current("0.1.0", 1);
    site.publish("0.2.0", 1, "0.1.0", Channel::Stable);
    let elsewhere = site.path("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    site.request(&format!(
        r#"{{"version":"0.2.0","channel":"stable","requested_by":"admin","command":"rm -rf /","channel_url":"file://{}","systemctl":"/bin/false","opt":"{}"}}"#,
        elsewhere.display(),
        elsewhere.display()
    ));
    assert_eq!(site.update(), ExitCode::SUCCESS);
    assert_eq!(site.current(), site.binary_of("0.2.0"));
    assert_eq!(site.systemctl_calls(), restart_sequence());
    assert!(fs::read_dir(&elsewhere).unwrap().next().is_none());

    site.request(r#"{"version":"0.2.0","channel":"stable","requested_by":"admin"}"#);
    site.clear_systemctl_calls();
    assert_eq!(site.update(), ExitCode::FAILURE);
    assert!(site.systemctl_calls().is_empty());

    site.request(r#"{"version":"0.3.0","channel":"nightly","requested_by":"admin"}"#);
    assert_eq!(site.update(), ExitCode::FAILURE);
    assert!(!site.path("lib/update.request").exists());
    assert_eq!(site.current(), site.binary_of("0.2.0"));

    assert_eq!(site.update(), ExitCode::FAILURE);
}

#[test]
fn a_channel_served_over_http_installs_the_same_way() {
    let site = Site::new();
    site.install_current("0.1.0", 1);
    site.publish("0.2.0", 1, "0.1.0", Channel::Beta);
    let root = site.path("channel");
    let (addr, stop) = serve_directory(root);
    site.request(r#"{"version":"0.2.0","channel":"beta","requested_by":"admin"}"#);
    let code = site.run(&format!("http://{addr}/keel"), &["--from-request"]);
    assert_eq!(code, ExitCode::SUCCESS);
    assert_eq!(site.current(), site.binary_of("0.2.0"));
    assert_eq!(site.systemctl_calls(), restart_sequence());

    site.request(r#"{"version":"0.2.0","channel":"stable","requested_by":"admin"}"#);
    site.clear_systemctl_calls();
    let code = site.run(&format!("http://{addr}/keel"), &["--from-request"]);
    assert_eq!(code, ExitCode::FAILURE);
    assert!(site.systemctl_calls().is_empty());
    let _ = stop.send(());
}

fn serve_directory(root: PathBuf) -> (SocketAddr, tokio::sync::oneshot::Sender<()>) {
    use axum::Router;
    use axum::body::Body;
    use axum::extract::State;
    use axum::http::{StatusCode, Uri};
    use axum::response::Response;

    async fn file(State(root): State<PathBuf>, uri: Uri) -> Response {
        let relative = uri.path().trim_start_matches("/keel/");
        match tokio::fs::read(root.join(relative)).await {
            Ok(bytes) => Response::new(Body::from(bytes)),
            Err(_) => Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::empty())
                .unwrap(),
        }
    }

    let (addr_tx, addr_rx) = std::sync::mpsc::channel();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            addr_tx.send(listener.local_addr().unwrap()).unwrap();
            let app = Router::new().fallback(file).with_state(root);
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stop_rx.await;
                })
                .await
                .unwrap();
        });
    });
    (addr_rx.recv().unwrap(), stop_tx)
}
