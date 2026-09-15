#![cfg(unix)]

use std::fs;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const SPIKE: &str = env!("CARGO_BIN_EXE_keel-spike");
const TEMPLATES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/templates");
const START_TIMEOUT: Duration = Duration::from_secs(10);
const VHOST: &str = "keel-demo-spike.conf";
const RENDERED: &str = "server {\n    listen 80;\n    server_name spike.example;\n    root /var/www/keel-demo-spike;\n    index index.html;\n\n    location / {\n        try_files $uri $uri/ =404;\n    }\n}\n";
const NGINX_MESSAGE: &str = "2026/09/15 07:01:51 [emerg] 20641#20641: host not found in \"eighty\" of the \"listen\" directive in /etc/nginx/conf.d/keel-demo-spike.conf:2";
const SYSTEMCTL_MESSAGE: &str =
    "Job for nginx.service failed because the control process exited with error code.";
const USERADD_MESSAGE: &str = "useradd: cannot lock /etc/passwd; try again later.";

struct Agent {
    child: Child,
    dir: tempfile::TempDir,
    socket: PathBuf,
    next_id: u64,
}

impl Agent {
    fn start(user_prefix: &str) -> Self {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let uid = fs::metadata(dir.path())
            .expect("the directory's metadata")
            .uid();
        Self::spawn(dir, uid, user_prefix)
    }

    fn start_for_another_uid() -> Self {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let uid = fs::metadata(dir.path())
            .expect("the directory's metadata")
            .uid();
        Self::spawn(dir, uid + 1, "kd_")
    }

    fn spawn(dir: tempfile::TempDir, uid: u32, user_prefix: &str) -> Self {
        let root = dir.path();
        let vhosts = root.join("vhosts");
        let bin = root.join("bin");
        fs::create_dir_all(&vhosts).expect("the vhosts root");
        fs::create_dir_all(&bin).expect("the fake bin");
        let policy = root.join("spike.yaml");
        fs::write(
            &policy,
            format!(
                "policy:\n  file_roots: {{ vhosts: {} }}\n  services: [nginx]\n  checks: {{ nginx.test: [\"nginx\", \"-t\"] }}\n  products:\n    demo: {{ user_prefix: {user_prefix}, drop_in_prefix: keel-demo- }}\n    dns: {{ drop_in_prefix: keel-dns- }}\n",
                vhosts.display()
            ),
        )
        .expect("the policy");
        let log = root.join("calls.log");
        let seen = root.join("seen.conf");
        fake(
            &bin.join("nginx"),
            &format!(
                "echo \"nginx $*\" >> {log}\ncat {vhosts}/*.conf > {seen} 2>/dev/null || :\nif [ -e {root}/fail-nginx ]; then rm -f {root}/fail-nginx; echo '{NGINX_MESSAGE}' >&2; echo 'nginx: configuration file /etc/nginx/nginx.conf test failed' >&2; exit 1; fi\necho 'nginx: the configuration file /etc/nginx/nginx.conf syntax is ok' >&2\n",
                log = log.display(),
                vhosts = vhosts.display(),
                seen = seen.display(),
                root = root.display(),
            ),
        );
        fake(
            &bin.join("systemctl"),
            &format!(
                "echo \"systemctl $*\" >> {log}\nif [ -e {root}/fail-systemctl ]; then rm -f {root}/fail-systemctl; echo '{SYSTEMCTL_MESSAGE}' >&2; exit 1; fi\n",
                log = log.display(),
                root = root.display(),
            ),
        );
        fake(
            &bin.join("useradd"),
            &format!(
                "echo \"useradd $*\" >> {log}\nif [ -e {root}/fail-useradd ]; then rm -f {root}/fail-useradd; echo '{USERADD_MESSAGE}' >&2; exit 1; fi\n",
                log = log.display(),
                root = root.display(),
            ),
        );
        let socket = root.join("agent.sock");
        let stderr = fs::File::create(root.join("agent.log")).expect("the agent log");
        let path = format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let child = Command::new(SPIKE)
            .args(["agent", "--socket"])
            .arg(&socket)
            .arg("--uid")
            .arg(uid.to_string())
            .arg("--policy")
            .arg(&policy)
            .arg("--templates")
            .arg(TEMPLATES)
            .env("PATH", path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr)
            .spawn()
            .expect("the agent starts");
        let started = Instant::now();
        while UnixStream::connect(&socket).is_err() {
            assert!(
                started.elapsed() < START_TIMEOUT,
                "the agent did not listen within {START_TIMEOUT:?}: {}",
                fs::read_to_string(root.join("agent.log")).unwrap_or_default()
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        Self {
            child,
            dir,
            socket,
            next_id: 1,
        }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn vhost(&self, name: &str) -> PathBuf {
        self.root().join("vhosts").join(name)
    }

    fn fail_next(&self, program: &str) {
        fs::write(self.root().join(format!("fail-{program}")), b"").expect("the failure marker");
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.root().join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn seen(&self) -> String {
        fs::read_to_string(self.root().join("seen.conf")).unwrap_or_default()
    }

    fn log(&self) -> String {
        fs::read_to_string(self.root().join("agent.log")).unwrap_or_default()
    }

    fn call(&mut self, op: &str, product: &str, args: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({
            "id": id,
            "op": op,
            "actor": { "login": "alice", "level": "user" },
            "request_id": format!("req-{id}"),
            "product": product,
            "args": args,
        });
        let answer = self.exchange(&request.to_string()).expect("an answer");
        assert_eq!(answer["id"], id, "{answer}");
        answer
    }

    fn exchange(&self, line: &str) -> Option<Value> {
        let mut stream = UnixStream::connect(&self.socket).expect("a connection");
        stream
            .write_all(format!("{line}\n").as_bytes())
            .expect("the request is sent");
        let mut reader = BufReader::new(stream);
        let mut answer = String::new();
        let read = match reader.read_line(&mut answer) {
            Ok(read) => read,
            Err(error) if error.kind() == ErrorKind::ConnectionReset => 0,
            Err(error) => panic!("the answer is read: {error}"),
        };
        (read > 0).then(|| serde_json::from_str(answer.trim_end()).expect("the answer is JSON"))
    }

    fn apply(&mut self, listen: &str, name: &str) -> Value {
        self.call(
            "host.template.apply",
            "demo",
            json!({
                "template": "vhost.conf.j2",
                "vars": { "server_name": "spike.example", "listen": listen, "root": "/var/www/keel-demo-spike" },
                "dest": { "root": "vhosts", "name": name },
                "mode": "0644",
                "validate": "nginx.test",
                "reload": "nginx",
            }),
        )
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn fake(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}")).expect("the fake is written");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("the fake is executable");
}

fn refused(answer: &Value, key: &str) -> String {
    assert_eq!(answer["ok"], false, "{answer}");
    assert_eq!(answer["error"]["key"], key, "{answer}");
    assert!(answer.get("data").is_none(), "{answer}");
    answer["error"]["detail"]
        .as_str()
        .unwrap_or_else(|| panic!("a detail in {answer}"))
        .to_owned()
}

#[test]
fn a_valid_template_is_rendered_swapped_in_validated_and_reloaded_and_a_repeat_changes_nothing() {
    let mut agent = Agent::start("kd_");
    let answer = agent.apply("80", VHOST);
    assert_eq!(
        answer,
        json!({ "id": 1, "ok": true, "data": { "changed": true } })
    );
    let target = agent.vhost(VHOST);
    assert_eq!(fs::read_to_string(&target).unwrap(), RENDERED);
    assert_eq!(fs::metadata(&target).unwrap().mode() & 0o7777, 0o644);
    assert_eq!(agent.calls(), ["nginx -t", "systemctl reload nginx"]);
    assert_eq!(agent.seen(), RENDERED);
    let names: Vec<String> = fs::read_dir(agent.root().join("vhosts"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, [VHOST]);

    let again = agent.apply("80", VHOST);
    assert_eq!(
        again,
        json!({ "id": 2, "ok": true, "data": { "changed": false } })
    );
    assert_eq!(agent.calls(), ["nginx -t", "systemctl reload nginx"]);
    assert_eq!(fs::read_to_string(&target).unwrap(), RENDERED);

    let log = agent.log();
    assert!(
        log.contains("audit op=host.template.apply actor=alice level=user request_id=req-1 product=demo outcome=ok"),
        "{log}"
    );
    assert!(
        log.contains("request_id=req-2 product=demo outcome=ok"),
        "{log}"
    );
}

#[test]
fn a_failed_check_restores_the_previous_file_and_answers_validate_failed_with_the_detail() {
    let mut agent = Agent::start("kd_");
    assert_eq!(agent.apply("80", VHOST)["data"]["changed"], true);
    let target = agent.vhost(VHOST);

    agent.fail_next("nginx");
    let answer = agent.apply("eighty", VHOST);
    let detail = refused(&answer, "host.error.validate_failed");
    assert!(detail.starts_with(NGINX_MESSAGE), "{detail}");
    assert!(detail.contains("[emerg]"), "{detail}");
    assert!(detail.contains("host not found in \"eighty\""), "{detail}");
    assert!(detail.contains("test failed"), "{detail}");
    assert_eq!(fs::read_to_string(&target).unwrap(), RENDERED);
    assert_eq!(fs::metadata(&target).unwrap().mode() & 0o7777, 0o644);
    assert!(agent.seen().contains("listen eighty;"), "{}", agent.seen());
    assert_eq!(
        agent.calls(),
        ["nginx -t", "systemctl reload nginx", "nginx -t"]
    );

    agent.fail_next("nginx");
    let fresh = "keel-demo-fresh.conf";
    let answer = agent.apply("eighty", fresh);
    refused(&answer, "host.error.validate_failed");
    assert!(!agent.vhost(fresh).exists());
    assert!(!agent.vhost(".keel-demo-fresh.conf.tmp").exists());
    assert_eq!(
        agent.calls(),
        ["nginx -t", "systemctl reload nginx", "nginx -t", "nginx -t"]
    );
    let log = agent.log();
    assert!(
        log.contains("request_id=req-2 product=demo outcome=host.error.validate_failed"),
        "{log}"
    );
}

#[test]
fn a_failed_reload_restores_the_file_reloads_again_and_answers_reload_failed() {
    let mut agent = Agent::start("kd_");
    assert_eq!(agent.apply("80", VHOST)["data"]["changed"], true);
    let target = agent.vhost(VHOST);

    agent.fail_next("systemctl");
    let answer = agent.apply("8080", VHOST);
    let detail = refused(&answer, "host.error.reload_failed");
    assert_eq!(detail, SYSTEMCTL_MESSAGE);
    assert_eq!(fs::read_to_string(&target).unwrap(), RENDERED);
    assert_eq!(
        agent.calls(),
        [
            "nginx -t",
            "systemctl reload nginx",
            "nginx -t",
            "systemctl reload nginx",
            "systemctl reload nginx"
        ]
    );
    assert!(agent.seen().contains("listen 8080;"), "{}", agent.seen());

    agent.fail_next("systemctl");
    let fresh = "keel-demo-fresh.conf";
    refused(&agent.apply("8080", fresh), "host.error.reload_failed");
    assert!(!agent.vhost(fresh).exists());
    let log = agent.log();
    assert!(
        log.contains("request_id=req-2 product=demo outcome=host.error.reload_failed"),
        "{log}"
    );
}

#[test]
fn a_foreign_root_an_unprefixed_name_or_user_and_an_unknown_product_are_refused_as_policy() {
    let mut agent = Agent::start("kd_");
    let template = |root: &str, name: &str, validate: &str, reload: &str| {
        json!({
            "template": "vhost.conf.j2",
            "vars": { "server_name": "spike.example", "listen": "80", "root": "/var/www/keel-demo-spike" },
            "dest": { "root": root, "name": name },
            "validate": validate,
            "reload": reload,
        })
    };
    let policy = "host.error.policy";

    let answer = agent.call(
        "host.template.apply",
        "demo",
        template("etc", VHOST, "nginx.test", "nginx"),
    );
    assert!(refused(&answer, policy).contains("etc is not a file root"));
    let answer = agent.call(
        "host.template.apply",
        "demo",
        template("vhosts", "spike.conf", "nginx.test", "nginx"),
    );
    assert!(refused(&answer, policy).contains("spike.conf"));
    let answer = agent.call(
        "host.template.apply",
        "demo",
        template("vhosts", "keel-demo-../spike.conf", "nginx.test", "nginx"),
    );
    refused(&answer, policy);
    let answer = agent.call(
        "host.template.apply",
        "demo",
        template("vhosts", VHOST, "other.test", "nginx"),
    );
    assert!(refused(&answer, policy).contains("other.test"));
    let answer = agent.call(
        "host.template.apply",
        "demo",
        template("vhosts", VHOST, "nginx.test", "sshd"),
    );
    assert!(refused(&answer, policy).contains("sshd"));
    let answer = agent.call(
        "host.template.apply",
        "other",
        template("vhosts", VHOST, "nginx.test", "nginx"),
    );
    assert!(refused(&answer, policy).contains("other has no section"));
    let answer = agent.call(
        "host.template.apply",
        "dns",
        template("vhosts", "keel-demo-x.conf", "nginx.test", "nginx"),
    );
    refused(&answer, policy);
    let mut escaped = template("vhosts", VHOST, "nginx.test", "nginx");
    escaped["template"] = json!("../Cargo.toml");
    refused(&agent.call("host.template.apply", "demo", escaped), policy);

    let answer = agent.call("host.user.create", "demo", json!({ "name": "spike" }));
    assert!(refused(&answer, policy).contains("kd_"));
    let answer = agent.call("host.user.create", "demo", json!({ "name": "kd_../x" }));
    refused(&answer, policy);
    let answer = agent.call("host.user.create", "dns", json!({ "name": "kd_spike" }));
    assert!(refused(&answer, policy).contains("user_prefix"));
    let answer = agent.call("host.user.create", "other", json!({ "name": "kd_spike" }));
    refused(&answer, policy);

    let answer = agent.call("host.service.reload", "demo", json!({ "name": "nginx" }));
    refused(&answer, "host.error.unknown_op");
    let answer = agent.call("host.user.create", "demo", json!({ "login": "kd_spike" }));
    refused(&answer, "host.error.args");

    assert!(agent.calls().is_empty());
    assert!(
        fs::read_dir(agent.root().join("vhosts"))
            .unwrap()
            .next()
            .is_none()
    );
    let log = agent.log();
    assert!(
        log.contains("audit op=host.template.apply actor=alice level=user request_id=req-1 product=demo outcome=host.error.policy"),
        "{log}"
    );
    assert!(
        log.contains("op=host.service.reload") && log.contains("outcome=host.error.unknown_op"),
        "{log}"
    );
}

#[test]
fn a_user_that_exists_answers_created_false() {
    let mut agent = Agent::start("ro");
    let answer = agent.call("host.user.create", "demo", json!({ "name": "root" }));
    assert_eq!(
        answer,
        json!({ "id": 1, "ok": true, "data": { "uid": 0, "created": false } })
    );
    assert!(agent.calls().is_empty());
}

#[test]
fn useradd_runs_with_the_shell_and_its_failure_answers_exec_failed_with_its_output() {
    let mut agent = Agent::start("kd_");
    agent.fail_next("useradd");
    let answer = agent.call("host.user.create", "demo", json!({ "name": "kd_spike" }));
    assert_eq!(refused(&answer, "host.error.exec_failed"), USERADD_MESSAGE);
    assert_eq!(
        agent.calls(),
        ["useradd --shell /usr/sbin/nologin kd_spike"]
    );

    let answer = agent.call(
        "host.user.create",
        "demo",
        json!({ "name": "kd_spike", "shell": "/bin/bash" }),
    );
    let detail = refused(&answer, "host.error.exec_failed");
    assert!(
        detail.contains("useradd exited 0 but kd_spike is not in /etc/passwd"),
        "{detail}"
    );
    assert_eq!(
        agent.calls(),
        [
            "useradd --shell /usr/sbin/nologin kd_spike",
            "useradd --shell /bin/bash kd_spike"
        ]
    );
    let answer = agent.call(
        "host.user.create",
        "demo",
        json!({ "name": "kd_spike", "shell": "bash" }),
    );
    refused(&answer, "host.error.policy");
}

#[test]
fn a_peer_with_another_uid_is_closed_without_an_answer_and_the_refusal_is_logged() {
    let agent = Agent::start_for_another_uid();
    let uid = fs::metadata(agent.root()).unwrap().uid();
    let request = json!({
        "id": 1,
        "op": "host.user.create",
        "actor": { "login": "nobody", "level": "user" },
        "request_id": "req-1",
        "product": "demo",
        "args": { "name": "kd_spike" },
    });
    assert_eq!(agent.exchange(&request.to_string()), None);
    let started = Instant::now();
    let expected = format!("refused peer_uid={uid} admitted_uid={}", uid + 1);
    while !agent.log().contains(&expected) {
        assert!(started.elapsed() < START_TIMEOUT, "{}", agent.log());
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(!agent.log().contains("audit op="), "{}", agent.log());
}

#[test]
fn a_line_that_is_not_a_request_is_answered_as_a_frame_error_and_the_socket_admits_every_uid() {
    let agent = Agent::start("kd_");
    assert_eq!(fs::metadata(&agent.socket).unwrap().mode() & 0o777, 0o666);
    let answer = agent
        .exchange(r#"{"id":9,"op":"host.user.create"}"#)
        .unwrap();
    assert_eq!(answer["id"], 9);
    refused(&answer, "host.error.frame");
    let answer = agent.exchange("nonsense").unwrap();
    assert_eq!(answer["id"], 0);
    refused(&answer, "host.error.frame");
    let log = agent.log();
    assert!(
        log.contains("audit op=- actor=- level=- request_id=- product=- outcome=host.error.frame"),
        "{log}"
    );
}

#[test]
fn the_client_prints_the_answer_and_exits_non_zero_on_a_refusal() {
    let agent = Agent::start("ro");
    let call = |op: &str, args: &str, product: &str| {
        Command::new(SPIKE)
            .args(["call", "--socket"])
            .arg(&agent.socket)
            .args([
                "--op",
                op,
                "--args",
                args,
                "--actor",
                "keel",
                "--product",
                product,
            ])
            .output()
            .expect("the client runs")
    };
    let output = call("host.user.create", r#"{"name":"root"}"#, "demo");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim_end(),
        r#"{"id":1,"ok":true,"data":{"uid":0,"created":false}}"#
    );

    let output = call("host.user.create", r#"{"name":"root"}"#, "other");
    assert_eq!(output.status.code(), Some(1));
    let printed: Value = serde_json::from_slice(&output.stdout).unwrap();
    refused(&printed, "host.error.policy");

    let output = call("host.user.create", "{", "demo");
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--args is not JSON"));

    let output = Command::new(SPIKE)
        .args([
            "call",
            "--socket",
            "/nonexistent/agent.sock",
            "--op",
            "host.user.create",
            "--actor",
            "keel",
            "--product",
            "demo",
        ])
        .output()
        .expect("the client runs");
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("could not connect"));
    let log = agent.log();
    assert!(
        log.contains("actor=keel level=user request_id=spike-"),
        "{log}"
    );
}

#[test]
fn the_shipped_unit_and_policy_name_the_runtime_directory_the_socket_and_the_prefixes() {
    let policy =
        fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/deploy/spike.yaml")).unwrap();
    assert!(policy.contains("vhosts: /etc/nginx/conf.d"));
    assert!(policy.contains("user_prefix: kd_, drop_in_prefix: keel-demo-"));
    let unit = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/deploy/keel-spike-agent.service"
    ))
    .unwrap();
    assert!(unit.contains("RuntimeDirectory=keel-spike"));
    assert!(unit.contains("RuntimeDirectoryMode=0755"));
    assert!(unit.contains("--socket /run/keel-spike/agent.sock"));
    assert!(unit.contains("--uid @UID@"));
}
