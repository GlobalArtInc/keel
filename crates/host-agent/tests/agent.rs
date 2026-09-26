use std::fs;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, Once, OnceLock};
use std::time::{Duration, Instant};

use keel_host_agent::{Agent, Policy};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tracing_subscriber::fmt::MakeWriter;

const ZONE_TEMPLATE: &str = "$ORIGIN {{ origin }}.\n$TTL 3600\n@ IN SOA ns1.{{ origin }}. hostmaster.{{ origin }}. ( {{ serial }} 3600 900 604800 300 )\n@ IN NS ns1.{{ origin }}.\n{% for record in records %}{{ record.name }} IN {{ record.type }} {{ record.value }}\n{% endfor %}";
const ZONE_FILE: &str = "keel-dns-a.org.zone";
const CHECK_MESSAGE: &str = "zone a.org/IN: NS ns1.a.org has no address records (A or AAAA)\nzone a.org/IN: not loaded due to errors.";
const SYSTEMCTL_MESSAGE: &str =
    "Job for named.service failed because the control process exited with error code.";

static JOURNAL: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static INIT: Once = Once::new();

struct Capture;

struct CaptureWriter;

impl Write for CaptureWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        JOURNAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = CaptureWriter;

    fn make_writer(&'a self) -> Self::Writer {
        CaptureWriter
    }
}

fn capture_journal() {
    INIT.call_once(|| {
        tracing_subscriber::fmt()
            .with_writer(Capture)
            .with_ansi(false)
            .without_time()
            .with_max_level(tracing::Level::DEBUG)
            .init();
    });
}

fn journal() -> String {
    String::from_utf8_lossy(
        &JOURNAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
    .into_owned()
}

struct Site {
    dir: tempfile::TempDir,
    socket: PathBuf,
    _runtime: tokio::runtime::Runtime,
    stop: Option<oneshot::Sender<()>>,
    next_id: u64,
    prefix: String,
}

impl Site {
    fn start(prefix: &str) -> Self {
        Self::start_for(prefix, own_uid())
    }

    fn start_for(prefix: &str, admitted: u32) -> Self {
        capture_journal();
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path();
        for sub in ["zones", "www", "bin", "templates/bind"] {
            fs::create_dir_all(root.join(sub)).expect("the tree");
        }
        fs::write(root.join("templates/bind/zone.j2"), ZONE_TEMPLATE).expect("the template");
        let uid = own_uid();
        let gid = fs::metadata(root).expect("metadata").gid();
        fs::write(
            root.join("passwd"),
            format!(
                "root:x:0:0:root:/root:/bin/sh\nbind:x:104:110::/var/cache/bind:/usr/sbin/nologin\ntester:x:{uid}:{gid}::/home/tester:/bin/sh\n"
            ),
        )
        .expect("passwd");
        fs::write(
            root.join("group"),
            format!("root:x:0:\nbind:x:110:\ntester:x:{gid}:\n"),
        )
        .expect("group");
        fs::write(root.join("shells"), "# login shells\n/bin/sh\n/bin/bash\n").expect("shells");
        write_stubs(root);
        let policy = format!(
            "policy:\n  uid_range: {{ from: 100000, to: 100003 }}\n  file_roots: {{ zones: {root}/zones, www: {root}/www }}\n  services: [named, php*-fpm]\n  checks: {{ named.check: [\"{root}/bin/named-checkconf\", \"-z\"] }}\n  exec_allow: [certbot]\n  products:\n    dns: {{ drop_in_prefix: keel-dns- }}\n    panel: {{ user_prefix: hp_, drop_in_prefix: keel-panel- }}\n  tools:\n    systemctl: {root}/bin/systemctl\n    getent: {root}/bin/getent\n    useradd: {root}/bin/useradd\n    userdel: {root}/bin/userdel\n    usermod: {root}/bin/usermod\n    groupadd: {root}/bin/groupadd\n    groupdel: {root}/bin/groupdel\n    shells: {root}/shells\n",
            root = root.display()
        );
        fs::write(root.join("agent.yaml"), policy).expect("the policy");
        let policy = Policy::load(&root.join("agent.yaml")).expect("the policy parses");
        let socket = root.join("run/agent.sock");
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("a runtime");
        let agent = Agent::new(policy, admitted, root.join("templates"));
        let bound = runtime
            .block_on(agent.bind(&socket))
            .expect("the agent binds");
        let (stop, stopped) = oneshot::channel();
        runtime.spawn(bound.serve(async {
            let _ = stopped.await;
        }));
        Self {
            dir,
            socket,
            _runtime: runtime,
            stop: Some(stop),
            next_id: 1,
            prefix: prefix.to_owned(),
        }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn zone(&self, name: &str) -> PathBuf {
        self.root().join("zones").join(name)
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
        fs::read_to_string(self.root().join("seen.zone")).unwrap_or_default()
    }

    fn passwd(&self) -> String {
        fs::read_to_string(self.root().join("passwd")).unwrap_or_default()
    }

    fn group(&self) -> String {
        fs::read_to_string(self.root().join("group")).unwrap_or_default()
    }

    fn request_id(&self, id: u64) -> String {
        format!("{}-{id}", self.prefix)
    }

    fn call(&mut self, op: &str, product: &str, args: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({
            "id": id,
            "op": op,
            "actor": { "login": "alice", "level": "user" },
            "request_id": self.request_id(id),
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

    fn apply(&mut self, serial: u64, records: Value, name: &str) -> Value {
        self.call(
            "host.template.apply",
            "dns",
            json!({
                "template": "bind/zone.j2",
                "vars": { "origin": "a.org", "serial": serial, "records": records },
                "dest": { "root": "zones", "name": name },
                "mode": "0640",
                "owner": "tester",
                "validate": "named.check",
                "reload": "named",
            }),
        )
    }

    fn write(&mut self, root: &str, name: &str) -> Value {
        self.call(
            "host.fs.write",
            "dns",
            json!({ "dest": { "root": root, "name": name }, "content": "x\n" }),
        )
    }

    fn audit_line(&self, id: u64) -> String {
        let request_id = format!("request_id={}", self.request_id(id));
        let text = journal();
        text.lines()
            .find(|line| line.contains(&request_id))
            .unwrap_or_else(|| panic!("no journal line with {request_id} in:\n{text}"))
            .to_owned()
    }
}

impl Drop for Site {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let started = Instant::now();
        while self.socket.exists() && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn own_uid() -> u32 {
    static UID: OnceLock<u32> = OnceLock::new();
    *UID.get_or_init(|| {
        let dir = tempfile::tempdir().expect("a temporary directory");
        fs::metadata(dir.path()).expect("metadata").uid()
    })
}

fn stub(root: &Path, name: &str, body: &str) {
    let path = root.join("bin").join(name);
    let body = body
        .replace("ROOT", &root.display().to_string())
        .replace("LOG", &root.join("calls.log").display().to_string());
    fs::write(&path, format!("#!/bin/sh\n{body}")).expect("the stub is written");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("the stub is executable");
}

fn write_stubs(root: &Path) {
    stub(
        root,
        "getent",
        "db=$1; shift\n[ \"${1:-}\" = -- ] && shift\ncase $db in passwd) file=ROOT/passwd;; group) file=ROOT/group;; *) exit 1;; esac\nif [ $# -eq 0 ]; then cat \"$file\"; exit 0; fi\nline=$(awk -F: -v k=\"$1\" '$1 == k || $3 == k' \"$file\")\n[ -n \"$line\" ] || exit 2\nprintf '%s\\n' \"$line\"\n",
    );
    stub(
        root,
        "useradd",
        "echo \"useradd $*\" >> LOG\nif [ -e ROOT/fail-useradd ]; then rm -f ROOT/fail-useradd; echo 'useradd: cannot lock /etc/passwd; try again later.' >&2; exit 1; fi\nuid=; gid=; shell=; home=\nwhile [ $# -gt 1 ]; do\n  case $1 in\n    --uid) uid=$2; shift 2;;\n    --gid) gid=$2; shift 2;;\n    --shell) shell=$2; shift 2;;\n    --home-dir) home=$2; shift 2;;\n    --create-home) mkdir -p \"$home\"; shift;;\n    --no-create-home) shift;;\n    *) echo \"useradd: unexpected $1\" >&2; exit 3;;\n  esac\ndone\nprintf '%s:x:%s:%s::%s:%s\\n' \"$1\" \"$uid\" \"$gid\" \"$home\" \"$shell\" >> ROOT/passwd\n",
    );
    stub(
        root,
        "userdel",
        "echo \"userdel $*\" >> LOG\nfor last; do :; done\nawk -F: -v k=\"$last\" '$1 != k' ROOT/passwd > ROOT/passwd.new && mv ROOT/passwd.new ROOT/passwd\n",
    );
    stub(
        root,
        "usermod",
        "echo \"usermod $*\" >> LOG\nshell=\nwhile [ $# -gt 1 ]; do\n  case $1 in\n    --shell) shell=$2; shift 2;;\n    --expiredate) shift 2;;\n    --lock|--unlock) shift;;\n    *) echo \"usermod: unexpected $1\" >&2; exit 3;;\n  esac\ndone\nif [ -n \"$shell\" ]; then awk -F: -v OFS=: -v k=\"$1\" -v s=\"$shell\" '$1 == k { $7 = s } { print }' ROOT/passwd > ROOT/passwd.new && mv ROOT/passwd.new ROOT/passwd; fi\n",
    );
    stub(
        root,
        "groupadd",
        "echo \"groupadd $*\" >> LOG\nif [ -e ROOT/fail-groupadd ]; then rm -f ROOT/fail-groupadd; echo 'groupadd: cannot lock /etc/group; try again later.' >&2; exit 10; fi\nprintf '%s:x:%s:\\n' \"$3\" \"$2\" >> ROOT/group\n",
    );
    stub(
        root,
        "groupdel",
        "echo \"groupdel $*\" >> LOG\nawk -F: -v k=\"$1\" '$1 != k' ROOT/group > ROOT/group.new && mv ROOT/group.new ROOT/group\n",
    );
    stub(
        root,
        "systemctl",
        &format!(
            "echo \"systemctl $*\" >> LOG\nif [ \"$1\" = list-unit-files ]; then if [ -f ROOT/unit-files ]; then cat ROOT/unit-files; fi; exit 0; fi\nif [ \"$1\" = is-active ]; then shift; for unit; do if [ -e ROOT/inactive-$unit ]; then echo inactive; else echo active; fi; done; exit 0; fi\nif [ -e ROOT/fail-systemctl ]; then rm -f ROOT/fail-systemctl; echo '{SYSTEMCTL_MESSAGE}' >&2; exit 1; fi\n"
        ),
    );
    stub(
        root,
        "named-checkconf",
        &format!(
            "echo \"named-checkconf $*\" >> LOG\ncat ROOT/zones/*.zone > ROOT/seen.zone 2>/dev/null || :\nif [ -e ROOT/fail-check ]; then rm -f ROOT/fail-check; echo '{CHECK_MESSAGE}' >&2; exit 1; fi\n"
        ),
    );
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

fn records(address: &str) -> Value {
    json!([
        { "name": "ns1", "type": "A", "value": address },
        { "name": "www", "type": "A", "value": address },
    ])
}

fn rendered(serial: u64, address: &str) -> String {
    format!(
        "$ORIGIN a.org.\n$TTL 3600\n@ IN SOA ns1.a.org. hostmaster.a.org. ( {serial} 3600 900 604800 300 )\n@ IN NS ns1.a.org.\nns1 IN A {address}\nwww IN A {address}\n"
    )
}

#[test]
fn a_peer_that_is_not_the_configured_uid_is_refused() {
    let other = Site::start_for("peer-other", own_uid() + 1);
    let request = json!({
        "id": 1,
        "op": "host.service.list",
        "actor": { "login": "alice", "level": "user" },
        "request_id": "peer-other-1",
        "product": "dns",
        "args": {},
    })
    .to_string();
    assert_eq!(other.exchange(&request), None);
    let mode = fs::metadata(&other.socket).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o666);
    let text = journal();
    let refusal = text
        .lines()
        .find(|line| {
            line.contains(&format!("peer_uid={}", own_uid()))
                && line.contains(&format!("admitted_uid={}", own_uid() + 1))
        })
        .unwrap_or_else(|| panic!("no refusal in:\n{text}"));
    assert!(refusal.contains("outcome=refused"), "{refusal}");
    assert!(refusal.contains("keel_host_agent::audit"), "{refusal}");
    assert!(!text.contains("request_id=peer-other-1"), "{text}");
    drop(other);

    let mut own = Site::start("peer-own");
    let answer = own.call("host.service.list", "dns", json!({}));
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(
        answer["data"],
        json!({ "services": [{ "name": "named", "state": "active" }] })
    );
    assert_eq!(
        own.calls(),
        [
            "systemctl list-unit-files --no-legend --type=service php*-fpm.service",
            "systemctl is-active named"
        ]
    );
    let line = own.audit_line(1);
    assert!(line.contains("op=host.service.list"), "{line}");
    assert!(line.contains("actor=alice"), "{line}");
    assert!(line.contains("level=user"), "{line}");
    assert!(line.contains("product=dns"), "{line}");
    assert!(line.contains("outcome=ok"), "{line}");
    assert!(line.contains(&format!("peer_uid={}", own_uid())), "{line}");
}

#[test]
fn a_file_operation_outside_file_roots_is_refused_and_audited() {
    let mut site = Site::start("outside");
    let detail = refused(&site.write("etc", "keel-dns-a.conf"), "host.error.policy");
    assert!(detail.contains("etc is not a file root"), "{detail}");
    let detail = refused(
        &site.write("zones", "../keel-dns-a.conf"),
        "host.error.policy",
    );
    assert!(detail.contains("../keel-dns-a.conf"), "{detail}");
    refused(
        &site.write("zones", "keel-dns-../a.conf"),
        "host.error.policy",
    );
    refused(
        &site.write("zones", "keel-dns-a/b.conf"),
        "host.error.policy",
    );
    refused(&site.write("zones", "a.org.zone"), "host.error.policy");
    refused(&site.write("zones", ".keel-dns-a"), "host.error.policy");
    let detail = refused(&site.write("zones", "keel-dns-"), "host.error.policy");
    assert!(detail.contains("keel-dns-"), "{detail}");

    let link = site.zone("keel-dns-link.zone");
    std::os::unix::fs::symlink(site.root().join("passwd"), &link).unwrap();
    let detail = refused(
        &site.write("zones", "keel-dns-link.zone"),
        "host.error.policy",
    );
    assert!(detail.contains("symbolic link"), "{detail}");
    assert!(site.passwd().contains("root:x:0:0"));

    refused(
        &site.call(
            "host.fs.read",
            "dns",
            json!({ "dest": { "root": "www", "name": "keel-panel-x" } }),
        ),
        "host.error.policy",
    );
    refused(
        &site.call(
            "host.fs.remove",
            "dns",
            json!({ "dest": { "root": "zones", "name": "keel-dns-../x" } }),
        ),
        "host.error.policy",
    );
    refused(
        &site.call(
            "host.fs.mkdir",
            "dns",
            json!({ "dest": { "root": "home", "name": "keel-dns-x" } }),
        ),
        "host.error.policy",
    );
    refused(
        &site.call("host.fs.list", "dns", json!({ "root": "etc" })),
        "host.error.policy",
    );
    refused(
        &site.call(
            "host.template.apply",
            "dns",
            json!({ "template": "bind/zone.j2", "dest": { "root": "etc", "name": "keel-dns-a" } }),
        ),
        "host.error.policy",
    );
    refused(
        &site.call(
            "host.template.apply",
            "dns",
            json!({ "template": "../passwd", "dest": { "root": "zones", "name": "keel-dns-a" } }),
        ),
        "host.error.policy",
    );

    let names: Vec<String> = fs::read_dir(site.root().join("zones"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["keel-dns-link.zone"]);
    assert!(
        fs::read_dir(site.root().join("www"))
            .unwrap()
            .next()
            .is_none()
    );
    assert!(site.calls().is_empty());

    for id in 1..site.next_id {
        let line = site.audit_line(id);
        assert!(line.contains("outcome=host.error.policy"), "{line}");
        assert!(line.contains("actor=alice"), "{line}");
        assert!(line.contains("level=user"), "{line}");
        assert!(line.contains("product=dns"), "{line}");
        assert!(line.contains("WARN"), "{line}");
    }
    let first = site.audit_line(1);
    assert!(first.contains("op=host.fs.write"), "{first}");
    assert!(
        first.contains("detail=\"etc is not a file root of the policy\""),
        "{first}"
    );
}

#[test]
fn template_apply_renders_swaps_validates_reloads_and_reports_changed() {
    let mut site = Site::start("apply");
    let answer = site.apply(1, records("192.0.2.1"), ZONE_FILE);
    assert_eq!(
        answer,
        json!({ "id": 1, "ok": true, "data": { "changed": true } })
    );
    let target = site.zone(ZONE_FILE);
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        rendered(1, "192.0.2.1")
    );
    let meta = fs::metadata(&target).unwrap();
    assert_eq!(meta.mode() & 0o7777, 0o640);
    assert_eq!(meta.uid(), own_uid());
    assert_eq!(
        site.calls(),
        ["named-checkconf -z", "systemctl reload named"]
    );
    assert_eq!(site.seen(), rendered(1, "192.0.2.1"));
    let names: Vec<String> = fs::read_dir(site.root().join("zones"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, [ZONE_FILE]);

    let answer = site.apply(2, records("192.0.2.2"), ZONE_FILE);
    assert_eq!(answer["data"], json!({ "changed": true }));
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        rendered(2, "192.0.2.2")
    );
    assert_eq!(
        site.calls(),
        [
            "named-checkconf -z",
            "systemctl reload named",
            "named-checkconf -z",
            "systemctl reload named"
        ]
    );
    let line = site.audit_line(1);
    assert!(line.contains("op=host.template.apply"), "{line}");
    assert!(line.contains("outcome=ok"), "{line}");
    assert!(line.contains("INFO"), "{line}");
    let line = site.audit_line(2);
    assert!(line.contains("outcome=ok"), "{line}");

    let plain = site.call(
        "host.template.apply",
        "dns",
        json!({
            "template": "bind/zone.j2",
            "vars": { "origin": "a.org", "serial": 3, "records": [] },
            "dest": { "root": "zones", "name": "keel-dns-b.org.zone" },
        }),
    );
    assert_eq!(plain["data"], json!({ "changed": true }));
    let plain_target = site.zone("keel-dns-b.org.zone");
    assert_eq!(fs::metadata(&plain_target).unwrap().mode() & 0o7777, 0o644);
    assert_eq!(site.calls().len(), 4);
}

#[test]
fn a_failing_check_rolls_the_file_back_and_a_failing_reload_reloads_again() {
    let mut site = Site::start("rollback");
    assert_eq!(
        site.apply(1, records("192.0.2.1"), ZONE_FILE)["data"]["changed"],
        true
    );
    let target = site.zone(ZONE_FILE);
    let before = fs::read(&target).unwrap();

    site.fail_next("check");
    let answer = site.apply(2, records("192.0.2.9"), ZONE_FILE);
    let detail = refused(&answer, "host.error.validate_failed");
    assert_eq!(detail, CHECK_MESSAGE);
    assert_eq!(fs::read(&target).unwrap(), before);
    assert_eq!(fs::metadata(&target).unwrap().mode() & 0o7777, 0o640);
    assert!(site.seen().contains("192.0.2.9"), "{}", site.seen());
    assert_eq!(
        site.calls(),
        [
            "named-checkconf -z",
            "systemctl reload named",
            "named-checkconf -z"
        ]
    );
    let line = site.audit_line(2);
    assert!(
        line.contains("outcome=host.error.validate_failed"),
        "{line}"
    );
    assert!(line.contains("detail=\"zone a.org/IN: NS"), "{line}");

    site.fail_next("check");
    let fresh = "keel-dns-fresh.zone";
    refused(
        &site.apply(3, records("192.0.2.9"), fresh),
        "host.error.validate_failed",
    );
    assert!(!site.zone(fresh).exists());
    assert!(!site.zone(".keel-dns-fresh.zone.tmp").exists());

    site.fail_next("systemctl");
    let answer = site.apply(4, records("192.0.2.4"), ZONE_FILE);
    let detail = refused(&answer, "host.error.reload_failed");
    assert_eq!(detail, SYSTEMCTL_MESSAGE);
    assert_eq!(fs::read(&target).unwrap(), before);
    assert_eq!(
        site.calls(),
        [
            "named-checkconf -z",
            "systemctl reload named",
            "named-checkconf -z",
            "named-checkconf -z",
            "named-checkconf -z",
            "systemctl reload named",
            "systemctl reload named"
        ]
    );
    assert!(site.seen().contains("192.0.2.4"), "{}", site.seen());

    site.fail_next("systemctl");
    refused(
        &site.apply(5, records("192.0.2.4"), fresh),
        "host.error.reload_failed",
    );
    assert!(!site.zone(fresh).exists());
    let line = site.audit_line(4);
    assert!(line.contains("outcome=host.error.reload_failed"), "{line}");

    let names: Vec<String> = fs::read_dir(site.root().join("zones"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, [ZONE_FILE]);
}

#[test]
fn an_unchanged_render_answers_changed_false_and_touches_nothing() {
    let mut site = Site::start("unchanged");
    assert_eq!(
        site.apply(1, records("192.0.2.1"), ZONE_FILE)["data"]["changed"],
        true
    );
    let target = site.zone(ZONE_FILE);
    let before = fs::metadata(&target).unwrap();
    std::thread::sleep(Duration::from_millis(20));

    let again = site.apply(1, records("192.0.2.1"), ZONE_FILE);
    assert_eq!(
        again,
        json!({ "id": 2, "ok": true, "data": { "changed": false } })
    );
    let after = fs::metadata(&target).unwrap();
    assert_eq!(after.ino(), before.ino());
    assert_eq!(after.modified().unwrap(), before.modified().unwrap());
    assert_eq!(after.mode(), before.mode());
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        rendered(1, "192.0.2.1")
    );
    assert_eq!(
        site.calls(),
        ["named-checkconf -z", "systemctl reload named"]
    );
    let names: Vec<String> = fs::read_dir(site.root().join("zones"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, [ZONE_FILE]);
    let line = site.audit_line(2);
    assert!(line.contains("outcome=ok"), "{line}");

    let mode = site.call(
        "host.template.apply",
        "dns",
        json!({
            "template": "bind/zone.j2",
            "vars": { "origin": "a.org", "serial": 1, "records": records("192.0.2.1") },
            "dest": { "root": "zones", "name": ZONE_FILE },
            "mode": "0600",
            "owner": "tester",
        }),
    );
    assert_eq!(mode["data"], json!({ "changed": true }));
    assert_eq!(fs::metadata(&target).unwrap().mode() & 0o7777, 0o600);
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        rendered(1, "192.0.2.1")
    );

    let written = site.call(
        "host.fs.write",
        "dns",
        json!({ "dest": { "root": "zones", "name": ZONE_FILE }, "content": rendered(1, "192.0.2.1"), "mode": "0600" }),
    );
    assert_eq!(written["data"], json!({ "changed": false }));
}

#[test]
fn a_request_whose_product_has_no_section_is_refused() {
    let mut site = Site::start("product");
    let answer = site.call("host.service.list", "bill", json!({}));
    let detail = refused(&answer, "host.error.policy");
    assert_eq!(detail, "the product bill has no section in the policy");
    let answer = site.call("host.user.create", "dns", json!({ "name": "hp_x" }));
    let detail = refused(&answer, "host.error.policy");
    assert!(detail.contains("user_prefix"), "{detail}");
    let answer = site.call("host.user.create", "other", json!({ "name": "hp_x" }));
    assert_eq!(
        refused(&answer, "host.error.policy"),
        "the product other has no section in the policy"
    );
    let answer = site.call("host.fs.list", "other", json!({ "root": "zones" }));
    refused(&answer, "host.error.policy");
    assert!(site.calls().is_empty());
    let line = site.audit_line(1);
    assert!(line.contains("op=host.service.list"), "{line}");
    assert!(line.contains("product=bill"), "{line}");
    assert!(line.contains("outcome=host.error.policy"), "{line}");
    let line = site.audit_line(3);
    assert!(line.contains("product=other"), "{line}");
    assert!(line.contains("outcome=host.error.policy"), "{line}");

    let answer = site.call("host.vm.create", "dns", json!({ "name": "vm1" }));
    refused(&answer, "host.error.unknown_op");
    let answer = site.call(
        "host.fs.write",
        "dns",
        json!({ "dest": { "root": "zones", "name": "keel-dns-x" } }),
    );
    let detail = refused(&answer, "host.error.args");
    assert!(detail.contains("content"), "{detail}");
    let answer = site.call(
        "host.fs.list",
        "dns",
        json!({ "root": "zones", "extra": 1 }),
    );
    refused(&answer, "host.error.args");

    let broken = site.exchange(r#"{"id":9,"op":"host.fs.list"}"#).unwrap();
    assert_eq!(broken["id"], 9);
    assert_eq!(broken["error"]["key"], "host.error.frame");
    let text = journal();
    assert!(
        text.lines()
            .any(|line| line.contains("id=9") && line.contains("outcome=host.error.frame")),
        "{text}"
    );
}

#[test]
fn users_and_groups_are_created_inside_the_uid_range_under_the_product_prefix() {
    let mut site = Site::start("users");
    let answer = site.call("host.user.create", "panel", json!({ "name": "hp_site" }));
    assert_eq!(
        answer["data"],
        json!({ "name": "hp_site", "uid": 100000, "gid": 100000, "home": "/nonexistent", "shell": "/usr/sbin/nologin", "created": true })
    );
    assert_eq!(
        site.calls(),
        [
            "groupadd --gid 100000 hp_site",
            "useradd --uid 100000 --gid 100000 --shell /usr/sbin/nologin --home-dir /nonexistent --no-create-home hp_site"
        ]
    );
    let again = site.call("host.user.create", "panel", json!({ "name": "hp_site" }));
    assert_eq!(again["data"]["created"], false);
    assert_eq!(again["data"]["uid"], 100000);
    assert_eq!(site.calls().len(), 2);

    let home = site.call(
        "host.user.create",
        "panel",
        json!({ "name": "hp_web", "shell": "/bin/sh", "home": { "root": "www", "name": "keel-panel-web" } }),
    );
    assert_eq!(home["data"]["uid"], 100001);
    assert_eq!(home["data"]["shell"], "/bin/sh");
    assert_eq!(
        home["data"]["home"],
        site.root()
            .join("www/keel-panel-web")
            .canonicalize()
            .unwrap()
            .display()
            .to_string()
    );
    assert!(site.root().join("www/keel-panel-web").is_dir());

    let group = site.call("host.group.create", "panel", json!({ "name": "hp_shared" }));
    assert_eq!(
        group["data"],
        json!({ "name": "hp_shared", "gid": 100002, "members": [], "created": true })
    );
    assert_eq!(
        site.calls().last().unwrap(),
        "groupadd --gid 100002 hp_shared"
    );
    let full = site.call("host.user.create", "panel", json!({ "name": "hp_last" }));
    assert_eq!(full["data"]["uid"], 100003);
    let none = site.call("host.user.create", "panel", json!({ "name": "hp_more" }));
    let detail = refused(&none, "host.error.policy");
    assert!(detail.contains("no free id"), "{detail}");

    let listed = site.call("host.user.list", "panel", json!({}));
    let names: Vec<&str> = listed["data"]["users"]
        .as_array()
        .unwrap()
        .iter()
        .map(|user| user["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["hp_site", "hp_web", "hp_last"]);
    let got = site.call("host.user.get", "panel", json!({ "name": "hp_web" }));
    assert_eq!(got["data"]["uid"], 100001);
    refused(
        &site.call("host.user.get", "panel", json!({ "name": "hp_absent" })),
        "host.error.not_found",
    );
    let groups = site.call("host.group.list", "panel", json!({}));
    assert_eq!(groups["data"]["groups"].as_array().unwrap().len(), 4);
    let got = site.call("host.group.get", "panel", json!({ "name": "hp_shared" }));
    assert_eq!(got["data"]["gid"], 100002);

    refused(
        &site.call("host.user.create", "panel", json!({ "name": "site" })),
        "host.error.policy",
    );
    refused(
        &site.call("host.user.create", "panel", json!({ "name": "hp_Site" })),
        "host.error.policy",
    );
    refused(
        &site.call(
            "host.user.create",
            "panel",
            json!({ "name": "hp_x", "shell": "sh" }),
        ),
        "host.error.policy",
    );
    let detail = refused(
        &site.call(
            "host.user.create",
            "panel",
            json!({ "name": "hp_x", "shell": "/tmp/x" }),
        ),
        "host.error.policy",
    );
    assert!(detail.contains("/tmp/x"), "{detail}");
    refused(
        &site.call(
            "host.user.create",
            "panel",
            json!({ "name": "hp_x", "shell": "/bin/sh -c id" }),
        ),
        "host.error.policy",
    );
    refused(
        &site.call("host.user.delete", "panel", json!({ "name": "root" })),
        "host.error.policy",
    );
    fs::write(
        site.root().join("passwd"),
        format!("{}hp_old:x:999:999::/nonexistent:/bin/sh\n", site.passwd()),
    )
    .unwrap();
    let detail = refused(
        &site.call("host.user.delete", "panel", json!({ "name": "hp_old" })),
        "host.error.policy",
    );
    assert!(detail.contains("999"), "{detail}");
    refused(
        &site.call("host.user.get", "panel", json!({ "name": "hp_old" })),
        "host.error.policy",
    );
    let detail = refused(
        &site.call("host.user.create", "panel", json!({ "name": "hp_old" })),
        "host.error.policy",
    );
    assert!(detail.contains("999"), "{detail}");
    let calls_before = site.calls().len();

    let removed = site.call(
        "host.user.delete",
        "panel",
        json!({ "name": "hp_web", "remove_home": true }),
    );
    assert_eq!(removed["data"], json!({ "removed": true }));
    assert_eq!(
        site.calls()[calls_before..],
        ["userdel --remove hp_web", "groupdel hp_web"]
    );
    assert!(!site.passwd().contains("hp_web:"));
    assert!(!site.group().contains("hp_web:"));
    let removed = site.call("host.user.delete", "panel", json!({ "name": "hp_web" }));
    assert_eq!(removed["data"], json!({ "removed": false }));
    let removed = site.call("host.group.delete", "panel", json!({ "name": "hp_shared" }));
    assert_eq!(removed["data"], json!({ "removed": true }));
    assert_eq!(site.calls().last().unwrap(), "groupdel hp_shared");

    site.fail_next("useradd");
    let failed = site.call("host.user.create", "panel", json!({ "name": "hp_web" }));
    let detail = refused(&failed, "host.error.exec_failed");
    assert!(detail.contains("cannot lock"), "{detail}");
    assert!(!site.group().contains("hp_web:"));
    assert_eq!(site.calls().last().unwrap(), "groupdel hp_web");
    let line = site.audit_line(site.next_id - 1);
    assert!(line.contains("outcome=host.error.exec_failed"), "{line}");

    let mut group = fs::OpenOptions::new()
        .append(true)
        .open(site.root().join("group"))
        .unwrap();
    writeln!(group, "hp_out:x:1000:").unwrap();
    let outside = site.call("host.group.create", "panel", json!({ "name": "hp_out" }));
    let detail = refused(&outside, "host.error.policy");
    assert!(detail.contains("hp_out"), "{detail}");
}

#[test]
fn files_are_written_through_a_temporary_and_listed_under_the_product_prefix() {
    let mut site = Site::start("files");
    let dest = json!({ "root": "www", "name": "keel-panel-site" });
    let created = site.call(
        "host.fs.mkdir",
        "panel",
        json!({ "dest": dest, "mode": "0750", "owner": "tester" }),
    );
    assert_eq!(created["data"], json!({ "created": true }));
    let dir = site.root().join("www/keel-panel-site");
    assert_eq!(fs::metadata(&dir).unwrap().mode() & 0o7777, 0o750);
    let created = site.call("host.fs.mkdir", "panel", json!({ "dest": dest }));
    assert_eq!(created["data"], json!({ "created": false }));

    let file = json!({ "root": "www", "name": "keel-panel-index.html" });
    let written = site.call(
        "host.fs.write",
        "panel",
        json!({ "dest": file, "content": "<h1>hi</h1>\n", "mode": "0640", "owner": "tester", "group": "tester" }),
    );
    assert_eq!(written["data"], json!({ "changed": true }));
    let path = site.root().join("www/keel-panel-index.html");
    assert_eq!(fs::read_to_string(&path).unwrap(), "<h1>hi</h1>\n");
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o7777, 0o640);
    let read = site.call("host.fs.read", "panel", json!({ "dest": file }));
    assert_eq!(read["data"]["content"], "<h1>hi</h1>\n");
    assert_eq!(read["data"]["mode"], "0640");
    assert_eq!(read["data"]["uid"], own_uid());
    assert_eq!(read["data"]["size"], 12);
    refused(
        &site.call(
            "host.fs.read",
            "panel",
            json!({ "dest": { "root": "www", "name": "keel-panel-absent" } }),
        ),
        "host.error.not_found",
    );
    refused(
        &site.call(
            "host.fs.write",
            "panel",
            json!({ "dest": file, "content": "x", "mode": "0999" }),
        ),
        "host.error.args",
    );
    for mode in ["4755", "2755"] {
        refused(
            &site.call(
                "host.fs.write",
                "panel",
                json!({ "dest": file, "content": "#!/bin/sh\n", "owner": "root", "mode": mode }),
            ),
            "host.error.args",
        );
        let line = site.audit_line(site.next_id - 1);
        assert!(line.contains("op=host.fs.write"), "{line}");
        assert!(line.contains("outcome=host.error.args"), "{line}");
    }
    assert_eq!(fs::read_to_string(&path).unwrap(), "<h1>hi</h1>\n");
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o7777, 0o640);
    refused(
        &site.call(
            "host.fs.mkdir",
            "panel",
            json!({ "dest": { "root": "www", "name": "keel-panel-suid" }, "mode": "4755" }),
        ),
        "host.error.args",
    );
    assert!(!site.root().join("www/keel-panel-suid").exists());
    let shared = site.call(
        "host.fs.mkdir",
        "panel",
        json!({ "dest": { "root": "www", "name": "keel-panel-shared" }, "mode": "2770" }),
    );
    assert_eq!(shared["data"], json!({ "created": true }));
    assert_eq!(
        fs::metadata(site.root().join("www/keel-panel-shared"))
            .unwrap()
            .mode()
            & 0o7777,
        0o2770
    );
    fs::remove_dir(site.root().join("www/keel-panel-shared")).unwrap();
    refused(
        &site.call(
            "host.fs.write",
            "panel",
            json!({ "dest": file, "content": "x", "owner": "nobody-here" }),
        ),
        "host.error.policy",
    );
    refused(
        &site.call(
            "host.fs.write",
            "panel",
            json!({ "dest": file, "content": "x", "group": "tester" }),
        ),
        "host.error.args",
    );
    fs::write(site.root().join("www/other.txt"), "not ours").unwrap();
    fs::write(site.root().join("www/keel-dns-x"), "another product").unwrap();
    let listed = site.call("host.fs.list", "panel", json!({ "root": "www" }));
    let entries = listed["data"]["entries"].as_array().unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["keel-panel-index.html", "keel-panel-site"]
    );
    assert_eq!(entries[0]["kind"], "file");
    assert_eq!(entries[0]["mode"], "0640");
    assert_eq!(entries[1]["kind"], "dir");
    let listed = site.call("host.fs.list", "dns", json!({ "root": "www" }));
    assert_eq!(listed["data"]["entries"].as_array().unwrap().len(), 1);

    refused(
        &site.call(
            "host.fs.remove",
            "panel",
            json!({ "dest": { "root": "www", "name": "other.txt" } }),
        ),
        "host.error.policy",
    );
    fs::write(dir.join("inner"), "x").unwrap();
    let failed = site.call("host.fs.remove", "panel", json!({ "dest": dest }));
    refused(&failed, "host.error.io");
    let removed = site.call(
        "host.fs.remove",
        "panel",
        json!({ "dest": dest, "recursive": true }),
    );
    assert_eq!(removed["data"], json!({ "removed": true }));
    assert!(!dir.exists());
    let removed = site.call("host.fs.remove", "panel", json!({ "dest": file }));
    assert_eq!(removed["data"], json!({ "removed": true }));
    let removed = site.call("host.fs.remove", "panel", json!({ "dest": file }));
    assert_eq!(removed["data"], json!({ "removed": false }));
    assert!(site.root().join("www/other.txt").exists());
    assert!(site.calls().is_empty());
}

#[test]
fn service_operations_reach_listed_units_through_systemctl_alone() {
    let mut site = Site::start("service");
    let answer = site.call("host.service.reload", "dns", json!({ "name": "named" }));
    assert_eq!(answer["data"], json!({ "name": "named", "reload": true }));
    let answer = site.call(
        "host.service.restart",
        "dns",
        json!({ "name": "named", "validate": "named.check" }),
    );
    assert_eq!(answer["data"], json!({ "name": "named", "restart": true }));
    assert_eq!(
        site.calls(),
        [
            "systemctl reload named",
            "named-checkconf -z",
            "systemctl restart named"
        ]
    );
    site.fail_next("check");
    let answer = site.call(
        "host.service.reload",
        "dns",
        json!({ "name": "named", "validate": "named.check" }),
    );
    assert_eq!(
        refused(&answer, "host.error.validate_failed"),
        CHECK_MESSAGE
    );
    assert_eq!(site.calls().len(), 4);
    site.fail_next("systemctl");
    let answer = site.call("host.service.reload", "dns", json!({ "name": "named" }));
    assert_eq!(
        refused(&answer, "host.error.reload_failed"),
        SYSTEMCTL_MESSAGE
    );
    refused(
        &site.call("host.service.reload", "dns", json!({ "name": "sshd" })),
        "host.error.policy",
    );
    refused(
        &site.call(
            "host.service.reload",
            "dns",
            json!({ "name": "named", "validate": "other" }),
        ),
        "host.error.policy",
    );
    refused(
        &site.call(
            "host.service.reload",
            "dns",
            json!({ "name": "named; reboot" }),
        ),
        "host.error.policy",
    );
    assert_eq!(site.calls().len(), 5);
    fs::write(site.root().join("inactive-named"), "").unwrap();
    let listed = site.call("host.service.list", "dns", json!({}));
    assert_eq!(
        listed["data"],
        json!({ "services": [{ "name": "named", "state": "inactive" }] })
    );
}

#[test]
fn the_socket_is_bound_again_when_it_disappears() {
    let mut site = Site::start("rebind");
    assert_eq!(site.call("host.service.list", "dns", json!({}))["ok"], true);
    fs::remove_file(&site.socket).unwrap();
    let started = Instant::now();
    while !site.socket.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the socket was not bound again"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(site.call("host.service.list", "dns", json!({}))["ok"], true);
    let text = journal();
    assert!(text.contains("bound again"), "{text}");
}

#[test]
fn a_symlink_planted_at_the_socket_path_leaves_its_target_alone_on_the_rebind() {
    let mut site = Site::start("socket-link");
    let target = site.root().join("sentinel");
    fs::write(&target, "sentinel").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    let planted = site.root().join("run/planted");
    std::os::unix::fs::symlink(&target, &planted).unwrap();
    fs::rename(&planted, &site.socket).unwrap();
    let started = Instant::now();
    while !fs::symlink_metadata(&site.socket)
        .unwrap()
        .file_type()
        .is_socket()
    {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the socket was not bound again"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(fs::metadata(&target).unwrap().mode() & 0o7777, 0o600);
    assert_eq!(fs::read_to_string(&target).unwrap(), "sentinel");
    assert_eq!(
        fs::symlink_metadata(&site.socket).unwrap().mode() & 0o777,
        0o666
    );
    assert_eq!(site.call("host.service.list", "dns", json!({}))["ok"], true);
}

#[test]
fn a_planted_symlink_is_never_followed_by_a_file_write() {
    let mut site = Site::start("file-link");
    let sentinel = site.root().join("sentinel");
    fs::write(&sentinel, "sentinel").unwrap();
    fs::set_permissions(&sentinel, fs::Permissions::from_mode(0o600)).unwrap();
    let temporary = site.zone(".keel-dns-x.tmp");
    std::os::unix::fs::symlink(&sentinel, &temporary).unwrap();
    let dest = json!({ "root": "zones", "name": "keel-dns-x" });
    let written = site.call(
        "host.fs.write",
        "dns",
        json!({ "dest": dest, "content": "zone\n", "mode": "0644" }),
    );
    assert_eq!(written["data"], json!({ "changed": true }), "{written}");
    assert_eq!(
        fs::read_to_string(site.zone("keel-dns-x")).unwrap(),
        "zone\n"
    );
    assert!(fs::symlink_metadata(&temporary).is_err());
    assert_eq!(fs::read_to_string(&sentinel).unwrap(), "sentinel");
    assert_eq!(fs::metadata(&sentinel).unwrap().mode() & 0o7777, 0o600);

    std::os::unix::fs::symlink(&sentinel, site.zone("keel-dns-y")).unwrap();
    let linked = json!({ "root": "zones", "name": "keel-dns-y" });
    refused(
        &site.call("host.fs.read", "dns", json!({ "dest": linked })),
        "host.error.policy",
    );
    refused(
        &site.call(
            "host.fs.write",
            "dns",
            json!({ "dest": linked, "content": "zone\n", "mode": "0644" }),
        ),
        "host.error.policy",
    );
    assert_eq!(fs::read_to_string(&sentinel).unwrap(), "sentinel");
    assert_eq!(fs::metadata(&sentinel).unwrap().mode() & 0o7777, 0o600);
}

#[test]
fn every_catalogue_operation_refuses_an_argument_it_does_not_know() {
    let mut site = Site::start("unknown-argument");
    for op in [
        "host.user.create",
        "host.user.delete",
        "host.user.get",
        "host.user.list",
        "host.group.create",
        "host.group.delete",
        "host.group.get",
        "host.group.list",
        "host.fs.write",
        "host.fs.read",
        "host.fs.remove",
        "host.fs.mkdir",
        "host.fs.list",
        "host.service.reload",
        "host.service.restart",
        "host.service.list",
        "host.template.apply",
        "host.pkg.install",
        "host.pkg.remove",
        "host.pkg.get",
        "host.pkg.refresh",
        "host.cron.write",
        "host.cron.remove",
        "host.cron.list",
        "host.quota.set",
        "host.quota.list",
        "host.exec",
    ] {
        let answer = site.call(op, "panel", json!({ "argv": ["/bin/sh", "-c", "id"] }));
        let detail = refused(&answer, "host.error.args");
        assert!(detail.contains("unknown field `argv`"), "{op}: {detail}");
    }
    assert!(site.calls().is_empty(), "{:?}", site.calls());
}

#[test]
fn a_file_the_agent_places_is_never_writable_by_every_account() {
    let mut site = Site::start("world-write");
    let file = json!({ "root": "zones", "name": "keel-dns-open" });
    let written = site.call(
        "host.fs.write",
        "dns",
        json!({ "dest": file, "content": "x\n", "mode": "0666" }),
    );
    let detail = refused(&written, "host.error.policy");
    assert!(detail.contains("lets every account"), "{detail}");
    let applied = site.call(
        "host.template.apply",
        "dns",
        json!({
            "template": "bind/zone.j2",
            "vars": { "origin": "a.org", "serial": 1, "records": [] },
            "dest": file,
            "mode": "0646",
        }),
    );
    refused(&applied, "host.error.policy");
    assert!(!site.zone("keel-dns-open").exists());
    let dir = json!({ "root": "www", "name": "keel-panel-open" });
    refused(
        &site.call(
            "host.fs.mkdir",
            "panel",
            json!({ "dest": dir, "mode": "0777" }),
        ),
        "host.error.policy",
    );
    assert!(!site.root().join("www/keel-panel-open").exists());
    let sticky = site.call(
        "host.fs.mkdir",
        "panel",
        json!({ "dest": dir, "mode": "1777" }),
    );
    assert_eq!(sticky["data"], json!({ "created": true }), "{sticky}");
    assert_eq!(
        fs::metadata(site.root().join("www/keel-panel-open"))
            .unwrap()
            .mode()
            & 0o7777,
        0o1777
    );
}

#[test]
fn no_setfsuid_and_no_credential_change_on_a_worker_thread() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect(&src, &mut files);
    assert!(files.len() > 5, "{files:?}");
    let mut credential_files = Vec::new();
    for file in &files {
        let text = fs::read_to_string(file).unwrap();
        assert!(
            !text.contains("setfsuid"),
            "{} names setfsuid",
            file.display()
        );
        assert!(
            !text.contains("setfsgid"),
            "{} names setfsgid",
            file.display()
        );
        let changes_credentials = [
            "setresuid",
            "setresgid",
            "setuid(",
            "seteuid",
            "setreuid",
            "setgroups",
        ]
        .iter()
        .any(|call| text.contains(call));
        if changes_credentials {
            credential_files.push(file.clone());
            assert!(
                !text.contains("spawn_blocking"),
                "{} changes credentials and names spawn_blocking",
                file.display()
            );
            assert!(
                !text.contains("std::thread") && !text.contains("thread::spawn"),
                "{} changes credentials and spawns a thread",
                file.display()
            );
            assert!(
                text.contains("pre_exec"),
                "{} changes credentials outside pre_exec",
                file.display()
            );
            assert!(
                text.contains("cfg(target_os = \"linux\")"),
                "{} changes credentials without the Linux gate",
                file.display()
            );
        }
    }
    assert_eq!(credential_files, [src.join("exec.rs")]);
    let worker_files: Vec<&PathBuf> = files
        .iter()
        .filter(|file| fs::read_to_string(file).unwrap().contains("spawn_blocking"))
        .collect();
    assert_eq!(worker_files, [&src.join("agent.rs")]);
}

fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    files.sort();
}

#[test]
fn a_service_pattern_lists_and_reloads_the_installed_versions_alone() {
    let mut site = Site::start("pattern");
    fs::write(
        site.root().join("unit-files"),
        "php8.2-fpm.service enabled enabled\nphp8.4-fpm.service disabled enabled\nphp-fpm-evil.service enabled enabled\nnginx.service enabled enabled\n",
    )
    .unwrap();
    fs::write(site.root().join("inactive-php8.4-fpm"), "").unwrap();
    let listed = site.call("host.service.list", "panel", json!({}));
    assert_eq!(
        listed["data"],
        json!({ "services": [
            { "name": "named", "state": "active" },
            { "name": "php8.2-fpm", "state": "active" },
            { "name": "php8.4-fpm", "state": "inactive" }
        ] }),
        "{listed}"
    );
    assert_eq!(
        site.call(
            "host.service.reload",
            "panel",
            json!({ "name": "php8.4-fpm" })
        )["ok"],
        true
    );
    for name in ["php-fpm-evil", "php*-fpm", "nginx"] {
        refused(
            &site.call("host.service.reload", "panel", json!({ "name": name })),
            "host.error.policy",
        );
    }
    assert_eq!(
        site.calls(),
        [
            "systemctl list-unit-files --no-legend --type=service php*-fpm.service",
            "systemctl is-active named php8.2-fpm php8.4-fpm",
            "systemctl reload php8.4-fpm"
        ]
    );
}

#[test]
fn a_user_inside_the_range_is_locked_unlocked_and_given_a_shell() {
    let mut site = Site::start("modify");
    assert_eq!(
        site.call("host.user.create", "panel", json!({ "name": "hp_site" }))["data"]["created"],
        true
    );
    let locked = site.call(
        "host.user.modify",
        "panel",
        json!({ "name": "hp_site", "locked": true }),
    );
    assert_eq!(locked["data"]["locked"], true, "{locked}");
    assert_eq!(locked["data"]["uid"], 100000);
    let shell = site.call(
        "host.user.modify",
        "panel",
        json!({ "name": "hp_site", "shell": "/bin/bash" }),
    );
    assert_eq!(shell["data"]["shell"], "/bin/bash", "{shell}");
    assert!(shell["data"].get("locked").is_none(), "{shell}");
    let unlocked = site.call(
        "host.user.modify",
        "panel",
        json!({ "name": "hp_site", "locked": false, "shell": "/usr/sbin/nologin" }),
    );
    assert_eq!(unlocked["data"]["locked"], false, "{unlocked}");
    assert_eq!(unlocked["data"]["shell"], "/usr/sbin/nologin");
    assert_eq!(
        site.calls()[2..],
        [
            "usermod --lock --expiredate 1 hp_site",
            "usermod --shell /bin/bash hp_site",
            "usermod --shell /usr/sbin/nologin --unlock --expiredate  hp_site"
        ]
    );

    refused(
        &site.call("host.user.modify", "panel", json!({ "name": "hp_site" })),
        "host.error.args",
    );
    refused(
        &site.call(
            "host.user.modify",
            "panel",
            json!({ "name": "hp_site", "shell": "/bin/zsh" }),
        ),
        "host.error.policy",
    );
    refused(
        &site.call(
            "host.user.modify",
            "panel",
            json!({ "name": "tester", "locked": true }),
        ),
        "host.error.policy",
    );
    refused(
        &site.call(
            "host.user.modify",
            "panel",
            json!({ "name": "hp_absent", "locked": true }),
        ),
        "host.error.not_found",
    );
    fs::write(
        site.root().join("passwd"),
        format!("{}hp_old:x:999:999::/nonexistent:/bin/sh\n", site.passwd()),
    )
    .unwrap();
    refused(
        &site.call(
            "host.user.modify",
            "panel",
            json!({ "name": "hp_old", "locked": true }),
        ),
        "host.error.policy",
    );
    assert_eq!(site.calls().len(), 5);
}
