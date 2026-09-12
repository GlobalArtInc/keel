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

    async fn http(addr: SocketAddr, method: &str, path: &str, body: &str) -> (u16, Value) {
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
        (status, serde_json::from_str(body).expect("a JSON body"))
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
        assert_eq!(
            lines[0].split_whitespace().collect::<Vec<_>>(),
            ["Заголовок", "Обновлена"],
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
