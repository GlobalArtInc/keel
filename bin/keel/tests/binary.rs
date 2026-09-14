const KEEL: &str = env!("CARGO_BIN_EXE_keel");

#[cfg(not(feature = "demo"))]
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
    use serde_json::{Value, json};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpStream, UnixStream};

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
                "listen: {listen}\nsocket: {}\ndata_dir: {}\ndatabase: {}\nlog: {{ level: warn, format: text }}\n",
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

    async fn fetch(
        addr: SocketAddr,
        method: &str,
        path: &str,
        body: &str,
    ) -> (u16, String, String) {
        let mut stream = TcpStream::connect(addr)
            .await
            .expect("the listener answers");
        stream
            .write_all(
                format!(
                    "{method} {path} HTTP/1.1\r\nHost: keel.test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .expect("the request is sent");
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .await
            .expect("the response is read");
        let text = String::from_utf8_lossy(&response);
        let (head, body) = text.split_once("\r\n\r\n").expect("a head and a body");
        let status: u16 = head
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse().ok())
            .expect("a status");
        (status, head.to_owned(), body.to_owned())
    }

    async fn http(addr: SocketAddr, method: &str, path: &str, body: &str) -> (u16, Value) {
        let (status, _, body) = fetch(addr, method, path, body).await;
        (status, serde_json::from_str(&body).expect("a JSON body"))
    }

    fn header_of<'a>(head: &'a str, name: &str) -> Option<&'a str> {
        head.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name).then(|| value.trim())
        })
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
        let unknown = site.failed(&["product", "enable", "dns"]);
        assert!(
            unknown.contains("the product dns is not carried by this binary; it carries demo"),
            "{unknown}"
        );
        assert_eq!(
            site.ok(&["product", "enable", "demo"]).trim(),
            "demo: enabled"
        );
        assert_eq!(
            site.ok(&["product", "list"]).trim(),
            format!("demo\tenabled\t{}", env!("CARGO_PKG_VERSION"))
        );
        let migrated = site.ok(&["migrate"]);
        assert!(migrated.contains("note: applied"), "{migrated}");

        let stderr = std::fs::File::create(site.dir.path().join("serve.log"))
            .expect("the log file is created");
        let mut serve = tokio::process::Command::from(site.command(&["serve"]))
            .stdout(Stdio::null())
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
        let refused = site.failed(&["product", "disable", "demo"]);
        assert!(refused.contains("the service is running"), "{refused}");

        let (status, body) = http(site.listen, "GET", "/readyz", "").await;
        assert_eq!(status, 200, "{body}");
        let (status, body) = http(site.listen, "POST", "/api/v1/call/note.list", "{}").await;
        assert_eq!(status, 401, "{body}");
        assert_eq!(body["ok"], json!(false));
        assert_eq!(body["error"]["code"], json!("unauthenticated"));
        let (status, body) = http(site.listen, "GET", "/api/v1/boot", "").await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["products"][0]["id"], json!("demo"));

        let index = shell_file("index.html");
        let (status, head, served) = fetch(site.listen, "GET", "/", "").await;
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
        let (status, _, fallback) = fetch(site.listen, "GET", "/f/note.list", "").await;
        assert_eq!(status, 200);
        assert_eq!(fallback, index);
        let main = hashed_main(&served);
        let (status, head, script) = fetch(site.listen, "GET", &format!("/{main}"), "").await;
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
        let (status, head, _) = fetch(site.listen, "GET", "/theme-boot.js", "").await;
        assert_eq!(status, 200);
        assert_eq!(header_of(&head, "cache-control"), Some("no-store"));
        let (status, body) = http(site.listen, "GET", "/missing.png", "").await;
        assert_eq!(status, 404, "{body}");
        assert_eq!(body["error"]["key"], json!("keel.error.route_not_found"));

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
