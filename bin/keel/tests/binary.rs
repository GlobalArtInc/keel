const KEEL: &str = env!("CARGO_BIN_EXE_keel");

mod privileged {
    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use super::KEEL;

    const START_TIMEOUT: Duration = Duration::from_secs(20);

    struct Tree {
        dir: tempfile::TempDir,
        uid: u32,
    }

    impl Tree {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("a temporary directory");
            let root = dir.path();
            let meta = fs::metadata(root).expect("metadata");
            let (uid, gid) = (meta.uid(), meta.gid());
            fs::create_dir_all(root.join("zones")).expect("the zones root");
            fs::create_dir_all(root.join("templates")).expect("the templates directory");
            fs::write(
                root.join("passwd"),
                format!(
                    "root:x:0:0:root:/root:/bin/sh\ntester:x:{uid}:{gid}::/home/tester:/bin/sh\n"
                ),
            )
            .expect("passwd");
            let getent = root.join("getent");
            fs::write(
                &getent,
                format!(
                    "#!/bin/sh\ndb=$1; shift\n[ \"${{1:-}}\" = -- ] && shift\n[ \"$db\" = passwd ] || exit 1\nif [ $# -eq 0 ]; then cat {passwd}; exit 0; fi\nline=$(awk -F: -v k=\"$1\" '$1 == k || $3 == k' {passwd})\n[ -n \"$line\" ] || exit 2\nprintf '%s\\n' \"$line\"\n",
                    passwd = root.join("passwd").display()
                ),
            )
            .expect("the getent stub");
            fs::set_permissions(&getent, fs::Permissions::from_mode(0o755)).expect("executable");
            fs::write(
                root.join("agent.yaml"),
                format!(
                    "policy:\n  uid_range: {{ from: {from}, to: {to} }}\n  file_roots: {{ zones: {zones} }}\n  products:\n    dns: {{ drop_in_prefix: keel-dns- }}\n  tools: {{ getent: {getent} }}\n",
                    from = uid.min(gid),
                    to = uid.max(gid),
                    zones = root.join("zones").display(),
                    getent = getent.display()
                ),
            )
            .expect("the policy");
            Self { dir, uid }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.dir.path().join(name)
        }

        fn arg(&self, name: &str) -> String {
            self.path(name).display().to_string()
        }
    }

    fn wait_for(socket: &Path, log: &Path) {
        let started = Instant::now();
        while UnixStream::connect(socket).is_err() {
            assert!(
                started.elapsed() < START_TIMEOUT,
                "the agent did not listen within {START_TIMEOUT:?}: {}",
                fs::read_to_string(log).unwrap_or_default()
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    #[test]
    fn keel_privileged_serves_the_host_policy_on_the_socket_and_audits_to_stderr() {
        let tree = Tree::new();
        let socket = tree.path("run/agent.sock");
        let log = tree.path("privileged.log");
        let stderr = fs::File::create(&log).expect("the log file");
        let mut child = Command::new(KEEL)
            .args([
                "privileged",
                "--socket",
                &tree.arg("run/agent.sock"),
                "--policy",
                &tree.arg("agent.yaml"),
                "--user",
                "tester",
                "--templates",
                &tree.arg("templates"),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr)
            .spawn()
            .expect("keel privileged starts");
        wait_for(&socket, &log);
        assert_eq!(
            fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
            0o666
        );

        let mut stream = UnixStream::connect(&socket).expect("a connection");
        stream
            .write_all(b"{\"id\":1,\"op\":\"host.fs.list\",\"actor\":{\"login\":\"alice\",\"level\":\"admin\"},\"request_id\":\"bin-1\",\"product\":\"dns\",\"args\":{\"root\":\"zones\"}}\n")
            .expect("the request is sent");
        let mut answer = String::new();
        BufReader::new(&stream)
            .read_line(&mut answer)
            .expect("the answer is read");
        assert_eq!(
            answer.trim_end(),
            r#"{"id":1,"ok":true,"data":{"entries":[]}}"#
        );
        stream
            .write_all(b"{\"id\":2,\"op\":\"host.fs.list\",\"actor\":{\"login\":\"alice\",\"level\":\"admin\"},\"request_id\":\"bin-2\",\"product\":\"bill\",\"args\":{\"root\":\"zones\"}}\n")
            .expect("the request is sent");
        let mut refused = String::new();
        BufReader::new(&stream)
            .read_line(&mut refused)
            .expect("the answer is read");
        assert!(
            refused.contains(r#""key":"host.error.policy""#),
            "{refused}"
        );
        drop(stream);

        let pid = child.id().to_string();
        assert!(
            Command::new("kill")
                .args(["-TERM", &pid])
                .status()
                .unwrap()
                .success()
        );
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().expect("the child is polled") {
                break status;
            }
            assert!(
                started.elapsed() < START_TIMEOUT,
                "keel privileged did not stop on SIGTERM"
            );
            std::thread::sleep(Duration::from_millis(25));
        };
        assert!(status.success(), "{status}");
        assert!(!socket.exists());
        let text = fs::read_to_string(&log).unwrap();
        assert!(text.contains("serves the host policy"), "{text}");
        assert!(
            text.contains(&format!("admitted_uid={}", tree.uid)),
            "{text}"
        );
        let accepted = text
            .lines()
            .find(|line| line.contains("request_id=bin-1"))
            .unwrap_or_else(|| {
                panic!(
                    "no audit line for bin-1 in:
{text}"
                )
            });
        assert!(accepted.contains("keel_host_agent::audit"), "{accepted}");
        assert!(accepted.contains("op=host.fs.list"), "{accepted}");
        assert!(accepted.contains("actor=alice"), "{accepted}");
        assert!(accepted.contains("level=admin"), "{accepted}");
        assert!(accepted.contains("product=dns"), "{accepted}");
        assert!(accepted.contains("outcome=ok"), "{accepted}");
        let refused = text
            .lines()
            .find(|line| line.contains("request_id=bin-2"))
            .unwrap_or_else(|| {
                panic!(
                    "no audit line for bin-2 in:
{text}"
                )
            });
        assert!(refused.contains("outcome=host.error.policy"), "{refused}");
        assert!(refused.contains("product=bill"), "{refused}");
        assert!(text.contains("the agent stopped"), "{text}");
    }

    #[cfg(feature = "demo")]
    #[test]
    fn keel_privileged_hands_the_compiled_templates_and_agent_operations_to_the_agent() {
        let tree = Tree::new();
        fs::create_dir_all(tree.path("notes")).expect("the notes root");
        let check = tree.path("digest-check");
        fs::write(
            &check,
            format!("#!/bin/sh\necho checked >> {}\n", tree.arg("checks.log")),
        )
        .expect("the check stub");
        fs::set_permissions(&check, fs::Permissions::from_mode(0o755)).expect("executable");
        fs::write(
            tree.path("demo.yaml"),
            format!(
                "policy:\n  uid_range: {{ from: {uid}, to: {uid} }}\n  file_roots: {{ notes: {notes} }}\n  checks: {{ note.digest: [\"{check}\"] }}\n  products:\n    demo: {{ drop_in_prefix: keel-demo-, classes: {{ note: {{ root: notes }} }} }}\n  tools: {{ getent: {getent} }}\n",
                uid = tree.uid,
                notes = tree.arg("notes"),
                check = check.display(),
                getent = tree.arg("getent"),
            ),
        )
        .expect("the policy");
        let socket = tree.path("run/agent.sock");
        let log = tree.path("privileged.log");
        let mut child = Command::new(KEEL)
            .args([
                "privileged",
                "--socket",
                &tree.arg("run/agent.sock"),
                "--policy",
                &tree.arg("demo.yaml"),
                "--user",
                "tester",
                "--templates",
                &tree.arg("templates"),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(fs::File::create(&log).expect("the log file"))
            .spawn()
            .expect("keel privileged starts");
        wait_for(&socket, &log);
        let stream = UnixStream::connect(&socket).expect("a connection");
        let exchange = |frame: &str| {
            (&stream)
                .write_all(format!("{frame}\n").as_bytes())
                .expect("the request is sent");
            let mut answer = String::new();
            BufReader::new(&stream)
                .read_line(&mut answer)
                .expect("the answer is read");
            answer.trim_end().to_owned()
        };
        let applied = exchange(
            r#"{"id":1,"op":"host.template.apply","actor":{"login":"alice","level":"admin"},"request_id":"demo-1","product":"demo","args":{"template":"note/digest.j2","vars":{"title":"Notes","notes":["a","b"]},"dest":{"root":"notes","name":"keel-demo-digest.txt"}}}"#,
        );
        assert_eq!(applied, r#"{"id":1,"ok":true,"data":{"changed":true}}"#);
        assert_eq!(
            fs::read_to_string(tree.path("notes/keel-demo-digest.txt")).expect("the digest"),
            "Notes\n- a\n- b\n"
        );
        assert_eq!(
            fs::read_to_string(tree.path("checks.log")).expect("the check ran"),
            "checked\n"
        );
        let stat = exchange(
            r#"{"id":2,"op":"note.digest.stat","actor":{"login":"alice","level":"admin"},"request_id":"demo-2","product":"demo","args":{"name":"keel-demo-digest.txt"}}"#,
        );
        assert_eq!(
            stat,
            r#"{"id":2,"ok":true,"data":{"product":"demo","name":"keel-demo-digest.txt","lines":3,"bytes":14}}"#
        );
        drop(stream);
        child.kill().expect("the agent is stopped");
        child.wait().expect("the agent is reaped");
        let text = fs::read_to_string(&log).unwrap_or_default();
        let templates = 1
            + if cfg!(feature = "dns") { 3 } else { 0 }
            + if cfg!(feature = "panel") { 2 } else { 0 };
        assert!(
            text.contains(&format!("operations=1 templates={templates}")),
            "{text}"
        );
        let line = text
            .lines()
            .find(|line| line.contains("request_id=demo-2"))
            .unwrap_or_else(|| panic!("no audit line for demo-2 in:\n{text}"));
        assert!(line.contains("class=note"), "{line}");
    }

    #[test]
    fn keel_privileged_refuses_to_start_without_a_policy_or_the_admitted_account() {
        let tree = Tree::new();
        let output = Command::new(KEEL)
            .args([
                "privileged",
                "--socket",
                &tree.arg("run/agent.sock"),
                "--policy",
                &tree.arg("missing.yaml"),
            ])
            .output()
            .expect("the binary runs");
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("could not read the policy"), "{stderr}");
        let output = Command::new(KEEL)
            .args([
                "privileged",
                "--socket",
                &tree.arg("run/agent.sock"),
                "--policy",
                &tree.arg("agent.yaml"),
                "--user",
                "nobody-here",
            ])
            .output()
            .expect("the binary runs");
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("nobody-here is not an account"), "{stderr}");
        assert!(!tree.path("run/agent.sock").exists());
    }

    #[test]
    fn agent_exec_refuses_a_uid_outside_the_range_and_changes_credentials_only_on_linux() {
        let tree = Tree::new();
        let outside = Command::new(KEEL)
            .args([
                "agent-exec",
                "--uid",
                "0",
                "--policy",
                &tree.arg("agent.yaml"),
                "--",
                "/bin/sh",
                "-c",
                "echo hi",
            ])
            .output()
            .expect("the binary runs");
        assert_eq!(outside.status.code(), Some(126));
        let stderr = String::from_utf8_lossy(&outside.stderr);
        assert!(stderr.contains("outside the uid_range"), "{stderr}");
        assert!(outside.stdout.is_empty());

        let unknown = Command::new(KEEL)
            .args([
                "agent-exec",
                "--uid",
                &tree.uid.to_string(),
                "--policy",
                &tree.arg("missing.yaml"),
                "--",
                "/bin/sh",
                "-c",
                "echo hi",
            ])
            .output()
            .expect("the binary runs");
        assert_eq!(unknown.status.code(), Some(126));

        let own = Command::new(KEEL)
            .args([
                "agent-exec",
                "--uid",
                &tree.uid.to_string(),
                "--policy",
                &tree.arg("agent.yaml"),
                "--",
                "/bin/sh",
                "-c",
                "echo hi",
            ])
            .output()
            .expect("the binary runs");
        let stderr = String::from_utf8_lossy(&own.stderr);
        if cfg!(target_os = "linux") {
            if own.status.success() {
                assert_eq!(String::from_utf8_lossy(&own.stdout), "hi\n");
            } else {
                assert_eq!(own.status.code(), Some(126), "{stderr}");
                assert!(stderr.contains("setgroups"), "{stderr}");
                assert!(stderr.contains("not permitted"), "{stderr}");
            }
        } else {
            assert_eq!(own.status.code(), Some(126), "{stderr}");
            assert!(stderr.contains("Linux operation"), "{stderr}");
            assert!(own.stdout.is_empty());
        }
    }
}

#[cfg(not(any(feature = "demo", feature = "dns", feature = "panel")))]
#[test]
fn the_binary_without_the_demo_feature_carries_no_product_bundle() {
    let output = std::process::Command::new(KEEL)
        .args([
            "--config",
            "/nonexistent/keel/config.yaml",
            "product",
            "enable",
            "demo",
        ])
        .output()
        .expect("the binary runs");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "the product demo is not carried by this binary; it carries no product bundle"
        ),
        "{stderr}"
    );
}

#[cfg(feature = "demo")]
mod demo {
    use std::net::{SocketAddr, TcpListener};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output, Stdio};
    use std::time::{Duration, Instant};

    use keel_testkit::TestDatabase;
    use keel_testkit::tokio;
    use rustls::pki_types::pem::PemObject;
    use serde_json::{Value, json};
    use tokio::net::UnixStream;

    use super::KEEL;

    struct Site {
        database: TestDatabase,
        dir: tempfile::TempDir,
        config: PathBuf,
        socket: PathBuf,
        listen: SocketAddr,
    }

    async fn site() -> Site {
        let database = TestDatabase::create().await;
        let dir = tempfile::tempdir().expect("a temporary directory");
        let listen = TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .expect("a free port");
        let socket = dir.path().join("run").join("core.sock");
        let config = dir.path().join("config.yaml");
        std::fs::write(
            &config,
            format!(
                "listen: {listen}\nsocket: {}\ndata_dir: {}\ndatabase: {}\nlog: {{ level: info, format: text }}\n",
                socket.display(),
                dir.path().join("lib").display(),
                database.config().database.expose(),
            ),
        )
        .expect("the config file is written");
        Site {
            database,
            dir,
            config,
            socket,
            listen,
        }
    }

    impl Site {
        fn command(&self, args: &[&str]) -> Command {
            let mut command = Command::new(KEEL);
            command.arg("--config").arg(&self.config).args(args);
            command
        }

        fn run(&self, args: &[&str]) -> Output {
            self.command(args).output().expect("the binary runs")
        }

        fn ok(&self, args: &[&str]) -> String {
            let output = self.run(args);
            assert!(
                output.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).into_owned()
        }

        fn failed(&self, args: &[&str]) -> String {
            let output = self.run(args);
            assert!(!output.status.success(), "{args:?} succeeded");
            String::from_utf8_lossy(&output.stderr).into_owned()
        }

        fn cert(&self) -> PathBuf {
            self.dir.path().join("lib").join("tls").join("cert.pem")
        }

        async fn finish(self) {
            drop(self.dir);
            self.database.finish().await;
        }
    }

    async fn wait_for_socket(path: &Path, serve: &mut tokio::process::Child) {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(30) {
            if UnixStream::connect(path).await.is_ok() {
                return;
            }
            if let Ok(Some(status)) = serve.try_wait() {
                panic!("keel serve exited early with {status}");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("the socket {} never came up", path.display());
    }

    fn client(site: &Site) -> reqwest::Client {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let pem = std::fs::read(site.cert()).expect("the site generated cert.pem");
        reqwest::Client::builder()
            .no_proxy()
            .tls_certs_only([reqwest::Certificate::from_pem(&pem).expect("a certificate")])
            .timeout(Duration::from_secs(10))
            .build()
            .expect("a client")
    }

    async fn fetch(
        site: &Site,
        method: &str,
        path: &str,
        body: &str,
    ) -> (u16, reqwest::header::HeaderMap, String) {
        let method: reqwest::Method = method.parse().expect("a method");
        let response = client(site)
            .request(method, format!("https://{}{path}", site.listen))
            .header("host", "keel.test")
            .header("content-type", "application/json")
            .body(body.to_owned())
            .send()
            .await
            .expect("the listener answers over TLS");
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let text = response.text().await.expect("the body is read");
        (status, headers, text)
    }

    async fn http(site: &Site, method: &str, path: &str, body: &str) -> (u16, Value) {
        let (status, _, body) = fetch(site, method, path, body).await;
        (status, serde_json::from_str(&body).expect("a JSON body"))
    }

    fn header_of<'a>(head: &'a reqwest::header::HeaderMap, name: &str) -> Option<&'a str> {
        head.get(name).and_then(|value| value.to_str().ok())
    }

    fn fingerprint_of(pem: &[u8]) -> String {
        let der = rustls::pki_types::CertificateDer::from_pem_slice(pem).expect("a certificate");
        let digest = <sha2::Sha256 as sha2::Digest>::digest(der.as_ref());
        digest
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(":")
    }

    fn shell_file(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../frontend/dist/shell")
            .join(name);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()))
    }

    fn elements(html: &str) -> Vec<&str> {
        html.match_indices('<')
            .map(|(start, _)| {
                let rest = &html[start..];
                &rest[..rest.find('>').map_or(rest.len(), |end| end + 1)]
            })
            .collect()
    }

    fn inline_handler(element: &str) -> bool {
        element.split_whitespace().any(|attribute| {
            attribute
                .strip_prefix("on")
                .and_then(|rest| rest.split_once('='))
                .is_some_and(|(name, _)| {
                    !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_alphabetic())
                })
        })
    }

    fn foreign_origin(element: &str) -> bool {
        ["href=\"", "src=\""].iter().any(|attribute| {
            element
                .split(attribute)
                .skip(1)
                .filter_map(|rest| rest.split('"').next())
                .any(|value| value.contains("://") || value.starts_with("//"))
        })
    }

    fn policy_violations(index: &str) -> Vec<String> {
        let mut violations = Vec::new();
        for element in elements(index) {
            if element.starts_with("<script") && !element.contains(" src=\"") {
                violations.push(format!("an inline script: {element}"));
            }
            if element.starts_with("<style") {
                violations.push(format!("a style element: {element}"));
            }
            if inline_handler(element) {
                violations.push(format!("an inline handler: {element}"));
            }
            if foreign_origin(element) {
                violations.push(format!("a foreign origin: {element}"));
            }
        }
        violations
    }

    fn hashed_main(index: &str) -> String {
        elements(index)
            .into_iter()
            .filter(|element| element.starts_with("<script"))
            .filter_map(|element| element.split("src=\"").nth(1)?.split('"').next())
            .find(|src| src.starts_with("main-") && src.ends_with(".js"))
            .expect("the index loads a hashed main")
            .to_owned()
    }

    #[test]
    fn the_policy_scan_catches_what_the_content_security_policy_refuses() {
        let clean = "<!doctype html><html><head><script src=\"theme-boot.js\"></script><link rel=\"stylesheet\" href=\"styles-X.css\"></head><body><keel-shell></keel-shell><script src=\"main-X.js\" type=\"module\"></script></body></html>";
        assert_eq!(policy_violations(clean), Vec::<String>::new());
        let inlined = "<head><style>body{margin:0}</style><link rel=\"stylesheet\" href=\"styles-X.css\" media=\"print\" onload=\"this.media='all'\"><script>window.x=1</script><script src=\"https://cdn.example/x.js\"></script></head>";
        let violations = policy_violations(inlined);
        assert_eq!(violations.len(), 4, "{violations:?}");
        assert!(
            violations[0].starts_with("a style element"),
            "{violations:?}"
        );
        assert!(
            violations[1].starts_with("an inline handler"),
            "{violations:?}"
        );
        assert!(
            violations[2].starts_with("an inline script"),
            "{violations:?}"
        );
        assert!(
            violations[3].starts_with("a foreign origin"),
            "{violations:?}"
        );
    }

    #[tokio::test]
    async fn the_binary_enables_demo_serves_and_answers_ctl_and_http_with_the_envelope() {
        let site = site().await;
        let unknown = site.failed(&["product", "enable", "bill"]);
        let carried = [
            Some("demo"),
            cfg!(feature = "dns").then_some("dns"),
            cfg!(feature = "panel").then_some("panel"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");
        assert!(
            unknown.contains(&format!(
                "the product bill is not carried by this binary; it carries {carried}"
            )),
            "{unknown}"
        );
        assert_eq!(
            site.ok(&["product", "enable", "demo"]).trim(),
            "demo: enabled"
        );
        let mut listed = format!("demo\tenabled\t{}", env!("CARGO_PKG_VERSION"));
        if cfg!(feature = "dns") {
            listed.push_str(&format!("\ndns\tdisabled\t{}", env!("CARGO_PKG_VERSION")));
        }
        if cfg!(feature = "panel") {
            listed.push_str(&format!("\npanel\tdisabled\t{}", env!("CARGO_PKG_VERSION")));
        }
        assert_eq!(site.ok(&["product", "list"]).trim(), listed);
        assert_eq!(site.ok(&["product", "features"]), "");
        let migrated = site.ok(&["migrate"]);
        assert!(migrated.contains("note: applied"), "{migrated}");

        let log_file = std::fs::File::create(site.dir.path().join("serve.log"))
            .expect("the log file is created");
        let stderr = log_file.try_clone().expect("the log file handle is cloned");
        let mut serve = tokio::process::Command::from(site.command(&["serve"]))
            .stdout(Stdio::from(log_file))
            .stderr(stderr)
            .kill_on_drop(true)
            .spawn()
            .expect("keel serve starts");
        wait_for_socket(&site.socket, &mut serve).await;

        let edited = site.ok(&["ctl", "note.edit", "title=x"]);
        let envelope: Value = serde_json::from_str(&edited).expect("an envelope");
        assert_eq!(envelope["ok"], json!(true), "{edited}");
        assert_eq!(envelope["data"]["title"], json!("x"));
        assert_eq!(envelope["meta"]["principal"]["login"], json!("owner"));
        let table = site.ok(&["ctl", "note.list", "--out", "table", "--lang", "ru"]);
        let lines: Vec<&str> = table.lines().collect();
        let ru: Value =
            serde_json::from_str(include_str!("../../../products/demo/i18n/note/ru.json"))
                .expect("the note ru dictionary");
        let header = |key: &str| ru["note"]["column"][key].as_str().expect("a label");
        assert_eq!(
            lines[0].split_whitespace().collect::<Vec<_>>(),
            [
                header("title"),
                header("kind"),
                header("tags"),
                header("due"),
                header("updatedAt"),
                header("accountId"),
            ],
            "{table}"
        );
        assert!(lines[1].starts_with("x "), "{table}");
        assert_eq!(
            site.ok(&["product", "enable", "demo"]).trim(),
            "demo: already enabled",
            "the switch goes over the control socket while the service runs"
        );

        let (status, body) = http(&site, "GET", "/healthz", "").await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body, json!({ "status": "ok" }));
        let log = std::fs::read_to_string(site.dir.path().join("serve.log")).expect("the log");
        let fingerprint =
            fingerprint_of(&std::fs::read(site.cert()).expect("the site generated cert.pem"));
        assert!(
            log.contains(&fingerprint),
            "the log names the certificate fingerprint {fingerprint}: {log}"
        );
        assert!(
            !log.contains("PRIVATE KEY"),
            "the log never carries the key: {log}"
        );
        let (status, body) = http(&site, "GET", "/readyz", "").await;
        assert_eq!(status, 200, "{body}");
        let (status, body) = http(&site, "POST", "/api/v1/call/note.list", "{}").await;
        assert_eq!(status, 401, "{body}");
        assert_eq!(body["ok"], json!(false));
        assert_eq!(body["error"]["code"], json!("unauthenticated"));
        let (status, body) = http(&site, "GET", "/api/v1/boot", "").await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["products"][0]["id"], json!("demo"));

        let index = shell_file("index.html");
        let (status, head, served) = fetch(&site, "GET", "/", "").await;
        assert_eq!(status, 200, "{served}");
        assert_eq!(served, index);
        assert_eq!(header_of(&head, "cache-control"), Some("no-store"));
        assert_eq!(
            header_of(&head, "content-security-policy"),
            Some(
                "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'"
            )
        );
        assert_eq!(header_of(&head, "x-content-type-options"), Some("nosniff"));
        assert_eq!(header_of(&head, "referrer-policy"), Some("same-origin"));
        assert_eq!(policy_violations(&served), Vec::<String>::new());
        assert!(served.contains("<keel-shell>"), "{served}");
        assert!(
            served.contains("<script src=\"theme-boot.js\"></script>"),
            "{served}"
        );
        let (status, _, fallback) = fetch(&site, "GET", "/f/note.list", "").await;
        assert_eq!(status, 200);
        assert_eq!(fallback, index);
        let main = hashed_main(&served);
        let (status, head, script) = fetch(&site, "GET", &format!("/{main}"), "").await;
        assert_eq!(status, 200, "{main}");
        assert_eq!(script, shell_file(&main));
        assert_eq!(
            header_of(&head, "cache-control"),
            Some("public, max-age=31536000, immutable"),
            "{main}"
        );
        assert_eq!(
            header_of(&head, "content-type"),
            Some("text/javascript; charset=utf-8")
        );
        let (status, head, _) = fetch(&site, "GET", "/theme-boot.js", "").await;
        assert_eq!(status, 200);
        assert_eq!(header_of(&head, "cache-control"), Some("no-store"));
        let (status, body) = http(&site, "GET", "/missing.png", "").await;
        assert_eq!(status, 404, "{body}");
        assert_eq!(body["error"]["key"], json!("keel.error.route_not_found"));

        assert_eq!(
            site.ok(&["product", "disable", "demo"]).trim(),
            "demo: disabled; the service restarts without its modules"
        );
        let status = tokio::time::timeout(Duration::from_secs(60), serve.wait())
            .await
            .expect("keel serve exits for the restart")
            .expect("the exit status is read");
        assert_eq!(status.code(), Some(75), "{status}");
        assert!(!site.socket.exists());
        assert!(
            site.ok(&["product", "list"])
                .lines()
                .any(|line| line.starts_with("demo\tdisabled\t")),
            "the enabled set changed before the exit"
        );
        assert_eq!(
            site.ok(&["product", "enable", "demo"]).trim(),
            "demo: enabled",
            "the stopped service takes the direct path"
        );
        let migrated = site.ok(&["migrate"]);
        assert!(migrated.contains("note: up to date"), "{migrated}");
        let again = std::fs::File::create(site.dir.path().join("serve-again.log"))
            .expect("the log file is created");
        let stderr = again.try_clone().expect("the log file handle is cloned");
        let mut serve = tokio::process::Command::from(site.command(&["serve"]))
            .stdout(Stdio::from(again))
            .stderr(stderr)
            .kill_on_drop(true)
            .spawn()
            .expect("keel serve starts again");
        wait_for_socket(&site.socket, &mut serve).await;
        let listed = site.ok(&["ctl", "note.list", "--out", "json"]);
        assert!(listed.contains("\"ok\": true"), "{listed}");

        let pid = serve.id().expect("the service has a pid").to_string();
        let signalled = Command::new("kill")
            .args(["-TERM", &pid])
            .status()
            .expect("kill runs");
        assert!(signalled.success());
        let status = tokio::time::timeout(Duration::from_secs(30), serve.wait())
            .await
            .expect("keel serve stops on SIGTERM")
            .expect("the exit status is read");
        assert!(status.success(), "{status}");
        assert!(!site.socket.exists());
        assert_eq!(
            site.ok(&["product", "disable", "demo"]).trim(),
            "demo: disabled"
        );
        site.finish().await;
    }
}
