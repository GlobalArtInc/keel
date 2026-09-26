use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, Once, OnceLock};
use std::time::{Duration, Instant};

use keel_host_agent::{Agent, AgentError, Failure, Operation, OperationCall, Policy, Template};
use keel_kernel::Composition;
use keel_sdk::schemars::JsonSchema;
use keel_sdk::{AgentOp, AgentOpError, AgentScope, ConfigTemplate, ModuleDef, Registrar};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tracing_subscriber::fmt::MakeWriter;

const DIGEST_TEMPLATE: &str = "note/digest.j2";
const DIGEST_TEXT: &str = "{{ title }}\n{% for note in notes %}- {{ note }}\n{% endfor %}";
const DIGEST_CHECK: &str = "note.digest";
const DIGEST_STAT: &str = "note.digest.stat";
const DIGEST_CLASS: &str = "note";
const DIGEST_FILE: &str = "keel-demo-digest.txt";
const CRON_FILE: &str = "keel-demo-recompute";

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "keel_sdk::schemars")]
struct StatArgs {
    name: String,
}

#[derive(Serialize, JsonSchema)]
#[schemars(crate = "keel_sdk::schemars")]
struct Stat {
    product: String,
    name: String,
    lines: usize,
    bytes: usize,
    owner: u32,
}

fn stat(scope: &dyn AgentScope, args: StatArgs) -> Result<Stat, AgentOpError> {
    let root = scope
        .policy()
        .get("root")
        .and_then(Value::as_str)
        .ok_or_else(|| AgentOpError::new("host.error.policy", "the class names no root"))?;
    let path = scope.resolve(root, &args.name)?;
    let text = fs::read_to_string(&path).map_err(|error| {
        let key = if error.kind() == std::io::ErrorKind::NotFound {
            "host.error.not_found"
        } else {
            "host.error.io"
        };
        AgentOpError::new(key, error.to_string())
    })?;
    Ok(Stat {
        product: scope.product().to_owned(),
        name: args.name,
        lines: text.lines().count(),
        bytes: text.len(),
        owner: scope.caller_uid(),
    })
}

fn note_init(r: &mut Registrar) -> keel_sdk::Result<()> {
    r.contribute(ConfigTemplate::new(DIGEST_TEMPLATE, DIGEST_TEXT).validate(DIGEST_CHECK))
        .contribute(AgentOp::new::<StatArgs, Stat>(DIGEST_STAT, stat).policy(DIGEST_CLASS));
    Ok(())
}

static NOTE: ModuleDef = ModuleDef {
    id: "note",
    version: "0.0.1",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: note_init,
};

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

fn own_uid() -> u32 {
    static UID: OnceLock<u32> = OnceLock::new();
    *UID.get_or_init(|| {
        let dir = tempfile::tempdir().expect("a temporary directory");
        fs::metadata(dir.path()).expect("metadata").uid()
    })
}

struct Scope<'a, 'b>(&'a OperationCall<'b>);

impl AgentScope for Scope<'_, '_> {
    fn product(&self) -> &str {
        self.0.product()
    }

    fn policy(&self) -> &Value {
        self.0.policy()
    }

    fn resolve(&self, root: &str, name: &str) -> Result<PathBuf, AgentOpError> {
        self.0
            .resolve(root, name)
            .map_err(|failure| AgentOpError::new(failure.key(), failure.detail()))
    }

    fn caller_uid(&self) -> u32 {
        self.0.caller_uid()
    }
}

fn demo_contributions() -> (Vec<AgentOp>, Vec<ConfigTemplate>) {
    let module = &NOTE;
    let mut registrar = Registrar::new(module.id);
    (module.init)(&mut registrar).expect("the demo module registers");
    let mut registrations = registrar
        .into_registrations()
        .expect("the demo registrations are valid");
    (
        registrations.contributions.take::<AgentOp>(),
        registrations.contributions.take::<ConfigTemplate>(),
    )
}

fn demo_catalogue() -> (Vec<Operation>, Vec<Template>) {
    let (ops, templates) = demo_contributions();
    let operations = ops
        .into_iter()
        .map(|op| {
            let name = op.name().to_owned();
            let class = op.class().to_owned();
            Operation::new(&name, &class, move |call, args| {
                op.run(&Scope(call), args)
                    .map_err(|error| Failure::new(error.key(), error.detail()))
            })
        })
        .collect();
    let templates = templates
        .iter()
        .map(|template| Template::new(template.id(), template.text(), template.check()))
        .collect();
    (operations, templates)
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
        capture_journal();
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path();
        for sub in ["zones", "notes", "dumps", "bin", "cron.d", "templates/note"] {
            fs::create_dir_all(root.join(sub)).expect("the tree");
        }
        let uid = own_uid();
        let gid = fs::metadata(root).expect("metadata").gid();
        fs::write(
            root.join("passwd"),
            format!(
                "root:x:0:0:root:/root:/bin/sh\nbind:x:104:110::/var/cache/bind:/usr/sbin/nologin\ntester:x:{uid}:{gid}::/home/tester:/bin/sh\nkd_site:x:100001:100001::/home/kd_site:/bin/sh\nkd_far:x:200001:200001::/home/kd_far:/bin/sh\n"
            ),
        )
        .expect("passwd");
        fs::write(root.join("group"), format!("root:x:0:\ntester:x:{gid}:\n")).expect("group");
        fs::write(root.join("dpkg"), "curl\tii \t8.5.0-2\n").expect("the package state");
        fs::write(
            root.join("archive"),
            "bind9\nbind9-utils\ncurl\ng++\ng++-14\n",
        )
        .expect("the package archive");
        write_stubs(root);
        let policy = format!(
            "policy:\n  uid_range: {{ from: 100000, to: 100003 }}\n  file_roots: {{ zones: {root}/zones, notes: {root}/notes, dumps: {root}/dumps }}\n  services: [named]\n  checks: {{ {DIGEST_CHECK}: [\"{root}/bin/digest-check\"], other.check: [\"{root}/bin/other-check\"] }}\n  exec_allow:\n    - uname\n    - {{ note.recompute: [/opt/keel/bin/current, ctl, note.recompute] }}\n    - {{ note.stamp: [/usr/bin/logger, -t, note, \"it's 100%\"] }}\n    - {{ certbot.renew: [\"{root}/bin/certbot\", renew, --quiet, --deploy-hook, \"systemctl reload nginx\"] }}\n    - {{ broken: [\"{root}/bin/broken\"] }}\n    - {{ mariadb: [\"{root}/bin/mariadb\", --batch] }}\n    - {{ mariadb-dump: [\"{root}/bin/mariadb-dump\", --single-transaction] }}\n  products:\n    demo: {{ user_prefix: kd_, drop_in_prefix: keel-demo-, classes: {{ {DIGEST_CLASS}: {{ root: notes }} }} }}\n    dns: {{ drop_in_prefix: keel-dns- }}\n  tools:\n    systemctl: {root}/bin/systemctl\n    getent: {root}/bin/getent\n    apt_get: {root}/bin/apt-get\n    dpkg_query: {root}/bin/dpkg-query\n    apt_cache: {root}/bin/apt-cache\n    setquota: {root}/bin/setquota\n    repquota: {root}/bin/repquota\n    cron_dir: {root}/cron.d\n",
            root = root.display()
        );
        fs::write(root.join("agent.yaml"), policy).expect("the policy");
        let policy = Policy::load(&root.join("agent.yaml")).expect("the policy parses");
        let (operations, templates) = demo_catalogue();
        let agent = Agent::new(policy, uid, root.join("templates"))
            .extend(operations, templates)
            .expect("the demo catalogue extends the agent");
        let socket = root.join("run/agent.sock");
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("a runtime");
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

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.root().join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn fail_next(&self, program: &str) {
        fs::write(self.root().join(format!("fail-{program}")), b"").expect("the failure marker");
    }

    fn request_id(&self, id: u64) -> String {
        format!("{}-{id}", self.prefix)
    }

    fn last_id(&self) -> u64 {
        self.next_id - 1
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
        let mut stream = UnixStream::connect(&self.socket).expect("a connection");
        stream
            .write_all(format!("{request}\n").as_bytes())
            .expect("the request is sent");
        let mut answer = String::new();
        BufReader::new(stream)
            .read_line(&mut answer)
            .expect("the answer is read");
        let answer: Value = serde_json::from_str(answer.trim_end()).expect("the answer is JSON");
        assert_eq!(answer["id"], id, "{answer}");
        answer
    }

    fn audit_line(&self, id: u64) -> String {
        let request_id = format!("request_id={} ", self.request_id(id));
        let text = String::from_utf8_lossy(
            &JOURNAL
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
        .into_owned();
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
        "certbot",
        "{ printf 'certbot|'; printf '%s|' \"$@\"; printf '\\n'; } >> LOG\necho 'Congratulations, all renewals succeeded'\n",
    );
    stub(
        root,
        "broken",
        "echo broken >> LOG\necho 'the certificate authority is unreachable' >&2\nexit 2\n",
    );
    stub(
        root,
        "mariadb",
        r#"input=$(cat)
{ printf 'mariadb|'; printf '%s|' "$@"; printf '\n%s\n' "$input"; } >> LOG
if [ -e ROOT/fail-mariadb ]; then rm -f ROOT/fail-mariadb; printf "ERROR 1064 (42000) at line 2: You have an error near '%s'\n%s\n" "$input" "$input" >&2; exit 1; fi
case " $* " in *" --sandbox "*) sandboxed=1;; *) sandboxed=0;; esac
if printf '%s\n' "$input" | grep -q '^\\!'; then
  if [ $sandboxed = 1 ]; then echo 'ERROR at line 1: Not allowed in the sandbox mode' >&2; exit 1; fi
  printf '%s\n' "$input" | sed -n 's/^\\!//p' | sh
fi
"#,
    );
    stub(
        root,
        "mariadb-dump",
        "{ printf 'mariadb-dump|'; printf '%s|' \"$@\"; printf '\\n'; } >> LOG\nfor last; do :; done\nprintf -- '-- dump of %s\\n' \"$last\"\n",
    );
    stub(root, "digest-check", "echo digest-check >> LOG\n");
    stub(root, "other-check", "echo other-check >> LOG\n");
    stub(root, "systemctl", "echo \"systemctl $*\" >> LOG\n");
    stub(
        root,
        "apt-get",
        "printf '%s\\n' \"apt-get $*|DEBIAN_FRONTEND=$DEBIAN_FRONTEND\" >> LOG\nif [ -e ROOT/fail-apt-get ]; then rm -f ROOT/fail-apt-get; echo 'E: Unable to locate package nosuch' >&2; exit 100; fi\nverb=$1; shift\nfor arg; do\n  case $arg in -*|Dpkg::*|APT::*) continue;; esac\n  case $verb in\n    install) [ -n \"$(awk -F'\\t' -v k=\"$arg\" '$1 == k' ROOT/dpkg)\" ] || printf '%s\\tii \\t1.0-1\\n' \"$arg\" >> ROOT/dpkg;;\n    remove) awk -F'\\t' -v k=\"$arg\" '$1 != k' ROOT/dpkg > ROOT/dpkg.new && mv ROOT/dpkg.new ROOT/dpkg;;\n  esac\ndone\n",
    );
    stub(
        root,
        "dpkg-query",
        "printf '%s\\n' \"dpkg-query $*\" >> LOG\nmissing=0\nfor arg; do\n  case $arg in -*) continue;; esac\n  line=$(awk -F'\\t' -v k=\"$arg\" '$1 == k' ROOT/dpkg)\n  [ -n \"$line\" ] || { echo \"dpkg-query: no packages found matching $arg\" >&2; missing=1; continue; }\n  printf '%s\\n' \"$line\"\ndone\nexit $missing\n",
    );
    stub(
        root,
        "apt-cache",
        "printf '%s\\n' \"apt-cache $*\" >> LOG\n[ \"$1\" = pkgnames ] || exit 1\nawk -v p=\"$2\" 'index($0, p) == 1' ROOT/archive\n",
    );
    stub(
        root,
        "repquota",
        "echo \"repquota $*\" >> LOG\nprintf '*** Report for user quotas on device /dev/vda1\\nUser            used    soft    hard  grace    used  soft  hard  grace\\n----\\nroot      --  1843212       0       0      0   61234     0     0      0\\nkd_site   --     2100    1000    4000      0      12     0     0      0\\n'\n",
    );
}

fn refused(answer: &Value, key: &str) -> String {
    assert_eq!(answer["ok"], false, "{answer}");
    assert_eq!(answer["error"]["key"], key, "{answer}");
    answer["error"]["detail"]
        .as_str()
        .unwrap_or_else(|| panic!("a detail in {answer}"))
        .to_owned()
}

fn data(answer: &Value) -> &Value {
    assert_eq!(answer["ok"], true, "{answer}");
    &answer["data"]
}

#[test]
fn exec_runs_only_allowlisted_binaries_with_their_fixed_arguments() {
    let mut site = Site::start("exec");
    let answer = site.call("host.exec", "dns", json!({ "name": "certbot.renew" }));
    assert_eq!(
        data(&answer)["output"],
        "Congratulations, all renewals succeeded\n"
    );
    assert_eq!(
        site.calls(),
        ["certbot|renew|--quiet|--deploy-hook|systemctl reload nginx|"]
    );
    let line = site.audit_line(site.last_id());
    assert!(line.contains("op=host.exec"), "{line}");
    assert!(line.contains("outcome=ok"), "{line}");

    let answer = site.call("host.exec", "dns", json!({ "name": "uname" }));
    assert!(
        !data(&answer)["output"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .is_empty(),
        "{answer}"
    );

    let answer = site.call(
        "host.exec",
        "dns",
        json!({ "name": "certbot.renew", "args": ["--help"] }),
    );
    refused(&answer, "host.error.args");
    for name in ["sh", "/bin/sh", "certbot", "certbot.renew --help"] {
        let answer = site.call("host.exec", "dns", json!({ "name": name }));
        let detail = refused(&answer, "host.error.policy");
        assert!(detail.contains("exec_allow"), "{detail}");
        let line = site.audit_line(site.last_id());
        assert!(line.contains("outcome=host.error.policy"), "{line}");
    }
    assert_eq!(site.calls().len(), 1, "{:?}", site.calls());

    let answer = site.call("host.exec", "dns", json!({ "name": "broken" }));
    let detail = refused(&answer, "host.error.exec_failed");
    assert_eq!(detail, "the certificate authority is unreachable");
    assert_eq!(site.calls().last().map(String::as_str), Some("broken"));
}

#[test]
fn exec_with_a_dest_writes_the_output_into_a_root_as_a_file_of_the_kernel() {
    let mut site = Site::start("exec-dest");
    let answer = site.call(
        "host.exec",
        "dns",
        json!({ "name": "certbot.renew", "dest": { "root": "dumps", "name": "keel-dns-renew.log" } }),
    );
    let captured = site.root().join("dumps/keel-dns-renew.log");
    let text = fs::read_to_string(&captured).expect("the captured output");
    assert_eq!(text, "Congratulations, all renewals succeeded\n");
    assert_eq!(data(&answer), &json!({ "bytes": text.len() }));
    let meta = fs::metadata(&captured).expect("the captured metadata");
    assert_eq!(meta.mode() & 0o777, 0o600);
    assert_eq!(meta.uid(), own_uid());
    assert!(!site.root().join("dumps/.keel-dns-renew.log.tmp").exists());

    for name in ["../renew.log", "renew.log"] {
        let answer = site.call(
            "host.exec",
            "dns",
            json!({ "name": "certbot.renew", "dest": { "root": "dumps", "name": name } }),
        );
        refused(&answer, "host.error.policy");
    }
    let answer = site.call(
        "host.exec",
        "dns",
        json!({ "name": "broken", "dest": { "root": "dumps", "name": "keel-dns-broken.log" } }),
    );
    refused(&answer, "host.error.exec_failed");
    assert!(!site.root().join("dumps/keel-dns-broken.log").exists());
    assert!(!site.root().join("dumps/.keel-dns-broken.log.tmp").exists());
}

#[test]
fn mariadb_exec_runs_a_fixed_statement_with_quoted_names_and_dumps_into_a_root() {
    const SESSION: &str =
        "SET SESSION sql_mode = (SELECT REPLACE(@@SESSION.sql_mode, 'NO_BACKSLASH_ESCAPES', ''));";
    let mut site = Site::start("mariadb");
    for args in [
        json!({ "statement": "database.create", "database": "kd_site_wp" }),
        json!({ "statement": "user.create", "user": "kd_site_app", "password": "it's \\ ok" }),
        json!({ "statement": "grant", "user": "kd_site_app", "database": "kd_site_wp" }),
        json!({ "statement": "revoke", "user": "kd_site_app", "database": "kd_site_wp" }),
        json!({ "statement": "user.lock", "user": "kd_site_app" }),
        json!({ "statement": "user.drop", "user": "kd_site_app" }),
        json!({ "statement": "database.drop", "database": "kd_site_wp" }),
    ] {
        let answer = site.call("mariadb.exec", "demo", args);
        assert_eq!(data(&answer), &json!({}));
    }
    let line = site.audit_line(site.last_id());
    assert!(line.contains("op=mariadb.exec"), "{line}");
    assert!(line.contains("outcome=ok"), "{line}");
    let client = "mariadb|--batch|--sandbox|--default-character-set=utf8mb4|";
    assert_eq!(
        site.calls(),
        [
            client,
            SESSION,
            "CREATE DATABASE `kd_site_wp` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;",
            client,
            SESSION,
            "CREATE USER 'kd_site_app'@'localhost' IDENTIFIED BY 'it\\'s \\\\ ok';",
            client,
            SESSION,
            "GRANT ALL PRIVILEGES ON `kd\\_site\\_wp`.* TO 'kd_site_app'@'localhost';",
            client,
            SESSION,
            "GRANT SELECT ON `kd\\_site\\_wp`.* TO 'kd_site_app'@'localhost';",
            "REVOKE ALL PRIVILEGES ON `kd\\_site\\_wp`.* FROM 'kd_site_app'@'localhost';",
            client,
            SESSION,
            "ALTER USER 'kd_site_app'@'localhost' ACCOUNT LOCK;",
            client,
            SESSION,
            "DROP USER IF EXISTS 'kd_site_app'@'localhost';",
            client,
            SESSION,
            "DROP DATABASE IF EXISTS `kd_site_wp`;",
        ]
    );

    let before = site.calls().len();
    for (product, args, key) in [
        (
            "demo",
            json!({ "statement": "database.drop", "database": "mysql" }),
            "host.error.policy",
        ),
        (
            "demo",
            json!({ "statement": "grant", "user": "kd_x", "database": "kd_a`b" }),
            "host.error.policy",
        ),
        (
            "demo",
            json!({ "statement": "user.create", "user": "root", "password": "x" }),
            "host.error.policy",
        ),
        (
            "dns",
            json!({ "statement": "database.create", "database": "kd_x" }),
            "host.error.policy",
        ),
        (
            "demo",
            json!({ "statement": "sql", "text": "DROP DATABASE mysql" }),
            "host.error.args",
        ),
        (
            "demo",
            json!({ "statement": "database.create", "database": "kd_x", "sql": "x" }),
            "host.error.args",
        ),
        (
            "demo",
            json!({ "statement": "user.password", "user": "kd_x", "password": "a\nb" }),
            "host.error.args",
        ),
    ] {
        let answer = site.call("mariadb.exec", product, args.clone());
        refused(&answer, key);
        let line = site.audit_line(site.last_id());
        assert!(line.contains(&format!("outcome={key}")), "{args}: {line}");
    }
    assert_eq!(site.calls().len(), before, "{:?}", site.calls());

    site.fail_next("mariadb");
    let answer = site.call(
        "mariadb.exec",
        "demo",
        json!({ "statement": "user.password", "user": "kd_site_app", "password": "it's secret" }),
    );
    let detail = refused(&answer, "host.error.exec_failed");
    assert!(!detail.contains("secret"), "{detail}");
    assert!(
        detail.starts_with("ERROR 1064 (42000) at line 2: You have an error"),
        "{detail}"
    );
    let line = site.audit_line(site.last_id());
    assert!(!line.contains("secret"), "{line}");

    let answer = site.call(
        "mariadb.exec",
        "demo",
        json!({ "statement": "dump", "database": "kd_site_wp", "dest": { "root": "dumps", "name": "keel-demo-1.sql" } }),
    );
    let dumped = site.root().join("dumps/keel-demo-1.sql");
    let text = fs::read_to_string(&dumped).expect("the dump");
    assert_eq!(text, "-- dump of kd_site_wp\n");
    assert_eq!(data(&answer)["bytes"], json!(text.len()));
    let meta = fs::metadata(&dumped).expect("the dump metadata");
    assert_eq!(meta.mode() & 0o777, 0o600);
    assert_eq!(meta.uid(), own_uid());
    assert_eq!(
        site.calls().last().map(String::as_str),
        Some(
            "mariadb-dump|--single-transaction|--skip-comments|--default-character-set=utf8mb4|kd_site_wp|"
        )
    );
    let seal = site.root().join("dumps/.keel-demo-1.sql.sha256");
    assert_eq!(
        fs::read_to_string(&seal).expect("the digest of the dump"),
        sha256(text.as_bytes())
    );
    let meta = fs::metadata(&seal).expect("the digest metadata");
    assert_eq!(meta.mode() & 0o777, 0o600);
    assert_eq!(meta.uid(), own_uid());
    let answer = site.call(
        "host.fs.remove",
        "demo",
        json!({ "dest": { "root": "dumps", "name": "keel-demo-1.sql" } }),
    );
    assert_eq!(data(&answer), &json!({ "removed": true }));
    assert!(!dumped.exists());
    assert!(!seal.exists(), "the digest leaves with the dump");
    let answer = site.call(
        "mariadb.exec",
        "demo",
        json!({ "statement": "dump", "database": "kd_site_wp", "dest": { "root": "dumps", "name": "../x.sql" } }),
    );
    refused(&answer, "host.error.policy");
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn sealed_dump(site: &Site, name: &str, content: &str) -> PathBuf {
    let dump = site.root().join("dumps").join(name);
    fs::write(&dump, content).expect("a dump");
    fs::set_permissions(&dump, fs::Permissions::from_mode(0o600)).expect("the dump mode");
    let seal = site.root().join("dumps").join(format!(".{name}.sha256"));
    fs::write(&seal, sha256(content.as_bytes())).expect("the digest");
    fs::set_permissions(&seal, fs::Permissions::from_mode(0o600)).expect("the digest mode");
    dump
}

#[test]
fn mariadb_exec_imports_only_a_dump_the_agent_sealed_into_one_database() {
    let mut site = Site::start("import");
    let dump = sealed_dump(
        &site,
        "keel-demo-2.sql",
        "DROP TABLE IF EXISTS `t`;\nCREATE TABLE `t` (a INT);\n",
    );
    let answer = site.call(
        "mariadb.exec",
        "demo",
        json!({ "statement": "import", "database": "kd_site_wp", "source": { "root": "dumps", "name": "keel-demo-2.sql" } }),
    );
    assert_eq!(data(&answer), &json!({}));
    assert_eq!(
        site.calls(),
        [
            "mariadb|--batch|--sandbox|--default-character-set=utf8mb4|kd_site_wp|",
            "DROP TABLE IF EXISTS `t`;",
            "CREATE TABLE `t` (a INT);",
        ]
    );
    std::os::unix::fs::symlink(&dump, site.root().join("dumps/keel-demo-3.sql")).expect("a link");
    sealed_dump(&site, "keel-demo-4.sql", "CREATE TABLE `u` (a INT);\n");
    fs::write(
        site.root().join("dumps/keel-demo-4.sql"),
        "CREATE TABLE `u` (a INT);\r",
    )
    .expect("one byte changed");
    sealed_dump(&site, "keel-demo-5.sql", "CREATE TABLE `v` (a INT);\n");
    fs::set_permissions(
        site.root().join("dumps/keel-demo-5.sql"),
        fs::Permissions::from_mode(0o620),
    )
    .expect("a dump its group may write");
    sealed_dump(&site, "keel-demo-6.sql", "CREATE TABLE `w` (a INT);\n");
    fs::remove_file(site.root().join("dumps/.keel-demo-6.sql.sha256")).expect("no digest");
    sealed_dump(&site, "keel-demo-7.sql", "CREATE TABLE `x` (a INT);\n");
    fs::copy(
        site.root().join("dumps/keel-demo-7.sql"),
        site.root().join("notes/keel-demo-7.sql"),
    )
    .expect("a copy in another root");
    fs::copy(
        site.root().join("dumps/.keel-demo-7.sql.sha256"),
        site.root().join("notes/.keel-demo-7.sql.sha256"),
    )
    .expect("its digest in another root");
    for (args, key) in [
        (
            json!({ "statement": "import", "database": "mysql", "source": { "root": "dumps", "name": "keel-demo-2.sql" } }),
            "host.error.policy",
        ),
        (
            json!({ "statement": "import", "database": "kd_site_wp", "source": { "root": "dumps", "name": "../keel-demo-2.sql" } }),
            "host.error.policy",
        ),
        (
            json!({ "statement": "import", "database": "kd_site_wp", "source": { "root": "dumps", "name": "keel-demo-3.sql" } }),
            "host.error.policy",
        ),
        (
            json!({ "statement": "import", "database": "kd_site_wp", "source": { "root": "dumps", "name": "keel-demo-4.sql" } }),
            "host.error.policy",
        ),
        (
            json!({ "statement": "import", "database": "kd_site_wp", "source": { "root": "dumps", "name": "keel-demo-5.sql" } }),
            "host.error.policy",
        ),
        (
            json!({ "statement": "import", "database": "kd_site_wp", "source": { "root": "dumps", "name": "keel-demo-6.sql" } }),
            "host.error.policy",
        ),
        (
            json!({ "statement": "import", "database": "kd_site_wp", "source": { "root": "notes", "name": "keel-demo-7.sql" } }),
            "host.error.policy",
        ),
        (
            json!({ "statement": "import", "database": "kd_site_wp", "source": { "root": "dumps", "name": "keel-demo-absent.sql" } }),
            "host.error.io",
        ),
    ] {
        let answer = site.call("mariadb.exec", "demo", args.clone());
        refused(&answer, key);
        let line = site.audit_line(site.last_id());
        assert!(line.contains(&format!("outcome={key}")), "{args}: {line}");
    }
    assert_eq!(site.calls().len(), 3, "{:?}", site.calls());
}

#[test]
fn mariadb_exec_refuses_to_import_a_file_written_by_host_fs_write() {
    let mut site = Site::start("forged");
    let marker = site.root().join("forged-marker");
    let content = format!(
        "\\! touch {}\nGRANT ALL ON *.* TO 'kd_site'@'localhost' WITH GRANT OPTION;\n",
        marker.display()
    );
    let answer = site.call(
        "host.fs.write",
        "demo",
        json!({ "dest": { "root": "dumps", "name": "keel-demo-forged.sql" }, "content": content }),
    );
    assert_eq!(data(&answer), &json!({ "changed": true }));
    let answer = site.call(
        "mariadb.exec",
        "demo",
        json!({ "statement": "import", "database": "kd_site_wp", "source": { "root": "dumps", "name": "keel-demo-forged.sql" } }),
    );
    let detail = refused(&answer, "host.error.policy");
    assert!(detail.contains("no digest"), "{detail}");
    let line = site.audit_line(site.last_id());
    assert!(line.contains("outcome=host.error.policy"), "{line}");
    assert!(site.calls().is_empty(), "{:?}", site.calls());
    assert!(!marker.exists());
}

#[test]
fn mariadb_exec_runs_the_client_sandboxed_so_a_shell_escape_in_a_dump_runs_nothing() {
    let mut site = Site::start("sandbox");
    let marker = site.root().join("escape-marker");
    sealed_dump(
        &site,
        "keel-demo-escape.sql",
        &format!(
            "\\! touch {}\nCREATE TABLE `t` (a INT);\n",
            marker.display()
        ),
    );
    let answer = site.call(
        "mariadb.exec",
        "demo",
        json!({ "statement": "import", "database": "kd_site_wp", "source": { "root": "dumps", "name": "keel-demo-escape.sql" } }),
    );
    let detail = refused(&answer, "host.error.exec_failed");
    assert!(detail.contains("sandbox"), "{detail}");
    assert_eq!(
        site.calls().first().map(String::as_str),
        Some("mariadb|--batch|--sandbox|--default-character-set=utf8mb4|kd_site_wp|")
    );
    assert!(!marker.exists(), "the shell escape ran");
}

#[test]
fn pkg_install_calls_the_package_manager_and_reports_what_changed() {
    let mut site = Site::start("pkg");
    let answer = site.call(
        "host.pkg.install",
        "dns",
        json!({ "packages": ["bind9", "curl", "bind9"] }),
    );
    assert_eq!(
        data(&answer),
        &json!({
            "changed": true,
            "changes": [{ "name": "bind9", "from": null, "to": "1.0-1" }],
        })
    );
    let calls = site.calls();
    assert!(
        calls.contains(&"apt-get install --yes --quiet --no-install-recommends -o APT::Cmd::Pattern-Only=true -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold bind9 curl|DEBIAN_FRONTEND=noninteractive".to_owned()),
        "{calls:?}"
    );
    assert_eq!(
        calls
            .iter()
            .filter(|line| line.starts_with("dpkg-query --show --showformat=${Package}\\t${db:Status-Abbrev}\\t${Version}\\n bind9 curl"))
            .count(),
        2,
        "{calls:?}"
    );

    let answer = site.call("host.pkg.install", "dns", json!({ "packages": ["bind9"] }));
    assert_eq!(data(&answer), &json!({ "changed": false, "changes": [] }));

    let answer = site.call("host.pkg.get", "dns", json!({ "name": "bind9" }));
    assert_eq!(
        data(&answer),
        &json!({ "name": "bind9", "version": "1.0-1" })
    );
    let answer = site.call("host.pkg.get", "dns", json!({ "name": "nginx" }));
    assert_eq!(data(&answer), &json!({ "name": "nginx", "version": null }));

    let answer = site.call("host.pkg.remove", "dns", json!({ "packages": ["bind9"] }));
    assert_eq!(
        data(&answer),
        &json!({
            "changed": true,
            "changes": [{ "name": "bind9", "from": "1.0-1", "to": null }],
        })
    );
    assert!(
        site.calls().contains(
            &"apt-get remove --yes --quiet -o APT::Cmd::Pattern-Only=true bind9|DEBIAN_FRONTEND=noninteractive".to_owned()
        ),
        "{:?}",
        site.calls()
    );

    let answer = site.call("host.pkg.install", "dns", json!({ "packages": ["g++"] }));
    assert_eq!(
        data(&answer),
        &json!({
            "changed": true,
            "changes": [{ "name": "g++", "from": null, "to": "1.0-1" }],
        })
    );
    assert!(
        site.calls().contains(&"apt-cache pkgnames g++".to_owned()),
        "{:?}",
        site.calls()
    );
    let answer = site.call("host.pkg.remove", "dns", json!({ "packages": ["g++"] }));
    assert_eq!(data(&answer)["changed"], json!(true));

    let apt_get = |site: &Site| {
        site.calls()
            .iter()
            .filter(|line| line.starts_with("apt-get "))
            .count()
    };
    let before = apt_get(&site);
    for (op, packages) in [
        ("host.pkg.install", json!(["curl-"])),
        ("host.pkg.install", json!(["bind9", "openssh-server-"])),
        ("host.pkg.remove", json!(["curl-"])),
        ("host.pkg.remove", json!(["bind9+"])),
        ("host.pkg.remove", json!(["g++"])),
        ("host.pkg.install", json!(["bind9+"])),
    ] {
        let answer = site.call(op, "dns", json!({ "packages": packages }));
        refused(&answer, "host.error.policy");
    }
    assert_eq!(apt_get(&site), before, "{:?}", site.calls());

    let answer = site.call("host.pkg.refresh", "dns", json!({}));
    assert_eq!(data(&answer), &json!({ "refreshed": true }));
    assert!(
        site.calls()
            .contains(&"apt-get update --quiet|DEBIAN_FRONTEND=noninteractive".to_owned())
    );

    let before = site.calls().len();
    for packages in [json!(["-y"]), json!(["bind9; rm -rf /"]), json!(["Bind9"])] {
        let answer = site.call("host.pkg.install", "dns", json!({ "packages": packages }));
        refused(&answer, "host.error.policy");
    }
    let answer = site.call("host.pkg.install", "dns", json!({ "packages": [] }));
    refused(&answer, "host.error.args");
    assert_eq!(site.calls().len(), before, "{:?}", site.calls());

    site.fail_next("apt-get");
    let answer = site.call("host.pkg.install", "dns", json!({ "packages": ["nosuch"] }));
    let detail = refused(&answer, "host.error.exec_failed");
    assert!(
        detail.contains("Unable to locate package nosuch"),
        "{detail}"
    );
    let line = site.audit_line(site.last_id());
    assert!(line.contains("op=host.pkg.install"), "{line}");
    assert!(line.contains("outcome=host.error.exec_failed"), "{line}");
}

#[test]
fn cron_writes_a_drop_in_under_the_product_prefix_and_removes_it() {
    let mut site = Site::start("cron");
    let cron = site.root().join("cron.d");
    fs::write(cron.join("e2scrub_all"), "30 3 * * 0 root true\n").expect("a foreign drop-in");
    fs::write(cron.join("keel-dns-renew"), "@daily root true\n").expect("another product's");
    let write = |name: &str, jobs: Value| json!({ "name": name, "jobs": jobs });
    let jobs = json!([
        { "schedule": "*/15  * * * *", "user": "tester", "exec": "note.recompute" },
        { "schedule": "@daily", "user": "kd_site", "exec": "note.stamp" },
    ]);
    let answer = site.call("host.cron.write", "demo", write(CRON_FILE, jobs.clone()));
    assert_eq!(data(&answer), &json!({ "changed": true }));
    let target = cron.join(CRON_FILE);
    assert_eq!(
        fs::read_to_string(&target).expect("the drop-in"),
        "*/15 * * * * tester '/opt/keel/bin/current' 'ctl' 'note.recompute'\n@daily kd_site '/usr/bin/logger' '-t' 'note' 'it'\\''s 100\\%'\n"
    );
    assert_eq!(
        fs::metadata(&target).expect("metadata").mode() & 0o7777,
        0o644
    );
    let answer = site.call("host.cron.write", "demo", write(CRON_FILE, jobs));
    assert_eq!(data(&answer), &json!({ "changed": false }));

    let answer = site.call("host.cron.list", "demo", json!({}));
    assert_eq!(data(&answer), &json!({ "entries": [CRON_FILE] }));

    let one = |schedule: &str, user: &str, exec: &str| json!([{ "schedule": schedule, "user": user, "exec": exec }]);
    let refusals = [
        (
            "recompute",
            one("@daily", "tester", "note.recompute"),
            "host.error.policy",
        ),
        (
            "keel-demo-a.cron",
            one("@daily", "tester", "note.recompute"),
            "host.error.policy",
        ),
        (
            "keel-dns-renew",
            one("@daily", "tester", "note.recompute"),
            "host.error.policy",
        ),
        (
            CRON_FILE,
            one("@daily", "root", "note.recompute"),
            "host.error.policy",
        ),
        (
            CRON_FILE,
            one("@daily", "0", "note.recompute"),
            "host.error.policy",
        ),
        (
            CRON_FILE,
            one("@daily", "bind", "note.recompute"),
            "host.error.policy",
        ),
        (
            CRON_FILE,
            one("@daily", "kd_far", "note.recompute"),
            "host.error.policy",
        ),
        (
            CRON_FILE,
            one("@daily", "kd_ghost", "note.recompute"),
            "host.error.policy",
        ),
        (
            CRON_FILE,
            one("* * * *", "tester", "note.recompute"),
            "host.error.args",
        ),
        (
            CRON_FILE,
            one("@daily", "tester", "date +%F >> /home/kd_site/days"),
            "host.error.policy",
        ),
        (
            CRON_FILE,
            one("@daily", "kd_site", "/bin/sh"),
            "host.error.policy",
        ),
        (
            CRON_FILE,
            json!([{ "schedule": "@daily", "user": "tester", "command": "id" }]),
            "host.error.args",
        ),
        (CRON_FILE, json!([]), "host.error.args"),
    ];
    for (name, jobs, key) in refusals {
        let answer = site.call("host.cron.write", "demo", write(name, jobs));
        refused(&answer, key);
        let line = site.audit_line(site.last_id());
        assert!(line.contains(&format!("outcome={key}")), "{line}");
    }
    assert_eq!(
        fs::read_to_string(&target).expect("the drop-in"),
        "*/15 * * * * tester '/opt/keel/bin/current' 'ctl' 'note.recompute'\n@daily kd_site '/usr/bin/logger' '-t' 'note' 'it'\\''s 100\\%'\n"
    );
    std::os::unix::fs::symlink(site.root().join("passwd"), cron.join("keel-demo-link"))
        .expect("a planted link");
    let answer = site.call(
        "host.cron.write",
        "demo",
        write("keel-demo-link", one("@daily", "tester", "note.recompute")),
    );
    refused(&answer, "host.error.policy");
    assert!(
        fs::read_to_string(site.root().join("passwd"))
            .expect("passwd")
            .starts_with("root:x:0:0")
    );

    let answer = site.call("host.cron.remove", "demo", json!({ "name": CRON_FILE }));
    assert_eq!(data(&answer), &json!({ "removed": true }));
    assert!(!target.exists());
    let answer = site.call("host.cron.remove", "demo", json!({ "name": CRON_FILE }));
    assert_eq!(data(&answer), &json!({ "removed": false }));
    let answer = site.call("host.cron.remove", "demo", json!({ "name": "e2scrub_all" }));
    refused(&answer, "host.error.policy");
    let mut left: Vec<String> = fs::read_dir(&cron)
        .expect("the cron directory")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    left.sort();
    assert_eq!(left, ["e2scrub_all", "keel-demo-link", "keel-dns-renew"]);
}

#[test]
fn quota_reaches_setquota_and_repquota_and_answers_unavailable_without_them() {
    let mut site = Site::start("quota");
    let answer = site.call("host.quota.list", "demo", json!({ "root": "notes" }));
    let listed = data(&answer);
    assert_eq!(
        listed["quotas"],
        json!([{
            "user": "kd_site",
            "blocks": { "used": 2100, "soft": 1000, "hard": 4000, "grace": 0 },
            "inodes": { "used": 12, "soft": 0, "hard": 0, "grace": 0 },
        }])
    );
    let filesystem = listed["filesystem"]
        .as_str()
        .expect("a filesystem")
        .to_owned();
    let notes = site.root().join("notes").canonicalize().expect("canonical");
    assert!(notes.starts_with(&filesystem), "{filesystem}");
    assert_eq!(site.calls(), [format!("repquota -u -p {filesystem}")]);

    let limits = json!({
        "user": "kd_site",
        "root": "notes",
        "blocks": { "soft": 1000, "hard": 2000 },
        "inodes": { "soft": 0, "hard": 0 },
    });
    let answer = site.call("host.quota.set", "demo", limits.clone());
    let detail = refused(&answer, "host.error.unavailable");
    assert!(detail.contains("setquota"), "{detail}");
    stub(site.root(), "setquota", "echo \"setquota $*\" >> LOG\n");
    let answer = site.call("host.quota.set", "demo", limits);
    assert_eq!(
        data(&answer),
        &json!({ "user": "kd_site", "filesystem": filesystem })
    );
    assert_eq!(
        site.calls().last().cloned(),
        Some(format!("setquota -u kd_site 1000 2000 0 0 {filesystem}"))
    );
    for (user, root) in [("root", "notes"), ("kd_far", "notes"), ("kd_site", "etc")] {
        let answer = site.call(
            "host.quota.set",
            "demo",
            json!({ "user": user, "root": root, "blocks": { "soft": 1, "hard": 2 }, "inodes": { "soft": 0, "hard": 0 } }),
        );
        refused(&answer, "host.error.policy");
    }
}

#[test]
fn a_product_agent_op_is_executed_under_its_policy_class_and_audited() {
    let (ops, _) = demo_contributions();
    let op = ops
        .iter()
        .find(|op| op.name() == DIGEST_STAT)
        .expect("the demo contributes its agent operation");
    assert_eq!(op.class(), DIGEST_CLASS);
    let args = serde_json::to_value(op.args_schema()).expect("a schema");
    assert_eq!(args["required"], json!(["name"]));
    let result = serde_json::to_value(op.result_schema()).expect("a schema");
    assert!(result["properties"].get("lines").is_some(), "{result}");

    let mut site = Site::start("op");
    fs::write(
        site.root().join("notes").join(DIGEST_FILE),
        "Notes\n- a\n- b\n",
    )
    .expect("a digest");
    let answer = site.call(DIGEST_STAT, "demo", json!({ "name": DIGEST_FILE }));
    assert_eq!(
        data(&answer),
        &json!({ "product": "demo", "name": DIGEST_FILE, "lines": 3, "bytes": 14, "owner": own_uid() })
    );
    let line = site.audit_line(site.last_id());
    for field in [
        format!("op={DIGEST_STAT}"),
        "actor=alice".to_owned(),
        "level=user".to_owned(),
        "product=demo".to_owned(),
        format!("class={DIGEST_CLASS}"),
        "outcome=ok".to_owned(),
    ] {
        assert!(line.contains(&field), "{field} in {line}");
    }

    let answer = site.call(DIGEST_STAT, "dns", json!({ "name": DIGEST_FILE }));
    let detail = refused(&answer, "host.error.policy");
    assert!(detail.contains(DIGEST_CLASS), "{detail}");
    let line = site.audit_line(site.last_id());
    assert!(line.contains(&format!("class={DIGEST_CLASS}")), "{line}");
    assert!(line.contains("outcome=host.error.policy"), "{line}");

    for name in ["../passwd", "passwd", "keel-demo-../../passwd"] {
        let answer = site.call(DIGEST_STAT, "demo", json!({ "name": name }));
        refused(&answer, "host.error.policy");
    }
    let answer = site.call(
        DIGEST_STAT,
        "demo",
        json!({ "name": DIGEST_FILE, "path": "/etc" }),
    );
    refused(&answer, "host.error.args");
    let answer = site.call(
        DIGEST_STAT,
        "demo",
        json!({ "name": "keel-demo-absent.txt" }),
    );
    refused(&answer, "host.error.not_found");
    let answer = site.call("note.digest.drop", "demo", json!({}));
    refused(&answer, "host.error.unknown_op");
}

#[test]
fn the_agent_refuses_an_operation_named_like_the_catalogue_or_contributed_twice() {
    let noop = |name: &str| Operation::new(name, "note", |_, args| Ok(args));
    let refused = Agent::new(Policy::default(), 0, PathBuf::from("/nonexistent"))
        .extend(vec![noop("host.exec")], Vec::new())
        .err();
    assert!(matches!(refused, Some(AgentError::BuiltIn(name)) if name == "host.exec"));
    let refused = Agent::new(Policy::default(), 0, PathBuf::from("/nonexistent"))
        .extend(vec![noop("mariadb.exec")], Vec::new())
        .err();
    assert!(matches!(refused, Some(AgentError::BuiltIn(name)) if name == "mariadb.exec"));
    let refused = Agent::new(Policy::default(), 0, PathBuf::from("/nonexistent"))
        .extend(vec![noop("note.a"), noop("note.a")], Vec::new())
        .err();
    assert!(matches!(refused, Some(AgentError::OperationTwice(name)) if name == "note.a"));
    let template = || Template::new(DIGEST_TEMPLATE, "x", None);
    let refused = Agent::new(Policy::default(), 0, PathBuf::from("/nonexistent"))
        .extend(Vec::new(), vec![template(), template()])
        .err();
    assert!(matches!(refused, Some(AgentError::TemplateTwice(id)) if id == DIGEST_TEMPLATE));
}

#[test]
fn a_template_override_under_etc_wins_over_the_compiled_text() {
    let (_, templates) = demo_contributions();
    let compiled = templates
        .iter()
        .find(|template| template.id() == DIGEST_TEMPLATE)
        .expect("the demo contributes its template");
    assert_eq!(compiled.check(), Some(DIGEST_CHECK));

    let mut site = Site::start("override");
    let target = site.root().join("notes").join(DIGEST_FILE);
    let apply = |validate: Option<&str>| {
        let mut args = json!({
            "template": DIGEST_TEMPLATE,
            "vars": { "title": "Notes", "notes": ["a", "b"] },
            "dest": { "root": "notes", "name": DIGEST_FILE },
        });
        if let Some(check) = validate {
            args["validate"] = json!(check);
        }
        args
    };
    let answer = site.call("host.template.apply", "demo", apply(None));
    assert_eq!(data(&answer), &json!({ "changed": true }));
    assert_eq!(
        fs::read_to_string(&target).expect("the digest"),
        "Notes\n- a\n- b\n"
    );
    assert_eq!(site.calls(), ["digest-check"]);

    let override_path = site.root().join("templates").join(DIGEST_TEMPLATE);
    fs::write(&override_path, "OVERRIDE {{ title }}\n").expect("the override");
    let answer = site.call("host.template.apply", "demo", apply(None));
    assert_eq!(data(&answer), &json!({ "changed": true }));
    assert_eq!(
        fs::read_to_string(&target).expect("the digest"),
        "OVERRIDE Notes\n"
    );
    assert_eq!(site.calls(), ["digest-check", "digest-check"]);

    let answer = site.call("host.template.apply", "demo", apply(Some("other.check")));
    assert_eq!(data(&answer), &json!({ "changed": false }));
    fs::write(&override_path, "OTHER {{ title }}\n").expect("the override");
    let answer = site.call("host.template.apply", "demo", apply(Some("other.check")));
    assert_eq!(data(&answer), &json!({ "changed": true }));
    assert_eq!(
        site.calls(),
        ["digest-check", "digest-check", "other-check"]
    );

    fs::remove_file(&override_path).expect("the override is removed");
    let answer = site.call("host.template.apply", "demo", apply(None));
    assert_eq!(data(&answer), &json!({ "changed": true }));
    assert_eq!(
        fs::read_to_string(&target).expect("the digest"),
        "Notes\n- a\n- b\n"
    );

    let mut absent = apply(None);
    absent["template"] = json!("note/absent.j2");
    let answer = site.call("host.template.apply", "demo", absent);
    let detail = refused(&answer, "host.error.render_failed");
    assert!(detail.contains("note/absent.j2"), "{detail}");
}

fn two_init(r: &mut Registrar) -> keel_sdk::Result<()> {
    r.contribute(ConfigTemplate::new(DIGEST_TEMPLATE, "two {{ title }}\n").validate(DIGEST_CHECK));
    Ok(())
}

static TWO: ModuleDef = ModuleDef {
    id: "two",
    version: "0.0.1",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: two_init,
};

#[test]
fn two_templates_with_one_id_fail_the_freeze() {
    let alone = Composition::new()
        .module(&NOTE)
        .resolve()
        .expect("the composition resolves")
        .init()
        .expect("init registers")
        .freeze();
    assert!(alone.is_ok(), "one template freezes");
    let Err(error) = Composition::new()
        .module(&NOTE)
        .module(&TWO)
        .resolve()
        .expect("the composition resolves")
        .init()
        .expect("init registers")
        .freeze()
    else {
        panic!("a second template with the same id fails the freeze");
    };
    let listed: Vec<String> = error.problems().iter().map(ToString::to_string).collect();
    assert_eq!(
        listed,
        [format!(
            "the configuration template {DIGEST_TEMPLATE} is contributed by both note and two"
        )]
    );
}
