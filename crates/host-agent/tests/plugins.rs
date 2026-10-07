use std::fs;
use std::io::{self, BufRead, BufReader, IoSliceMut, Read, Write};
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use keel_host_agent::{Agent, Policy};
use nix::sys::signal::kill;
use nix::sys::socket::{ControlMessageOwned, MsgFlags, recvmsg};
use nix::unistd::Pid;
use serde_json::{Value, json};
use tokio::sync::oneshot;

const PATIENCE: Duration = Duration::from_secs(20);
const UID: u32 = 100_000;
const GETENT: &str = "db=$1; shift\n[ \"${1:-}\" = -- ] && shift\ncase $db in passwd) file=ROOT/passwd;; group) file=ROOT/group;; *) exit 1;; esac\nif [ $# -eq 0 ]; then cat \"$file\"; exit 0; fi\nline=$(awk -F: -v k=\"$1\" '$1 == k || $3 == k' \"$file\")\n[ -n \"$line\" ] || exit 2\nprintf '%s\\n' \"$line\"\n";
const USERADD: &str = "echo \"useradd $*\" >> ROOT/log\nuid=; gid=\nwhile [ $# -gt 1 ]; do\n  case $1 in\n    --uid) uid=$2; shift 2;;\n    --gid) gid=$2; shift 2;;\n    --shell|--home-dir) shift 2;;\n    --no-create-home) shift;;\n    *) exit 3;;\n  esac\ndone\nprintf '%s:x:%s:%s::/nonexistent:/usr/sbin/nologin\\n' \"$1\" \"$uid\" \"$gid\" >> ROOT/passwd\n";
const GROUPADD: &str = "printf '%s:x:%s:\\n' \"$3\" \"$2\" >> ROOT/group\n";
const KEEL: &str = "echo \"keel $*\" >> ROOT/log\nif [ -f ROOT/refuse ]; then echo 'keel agent-exec: setresuid: Operation not permitted' >&2; exit 1; fi\n[ \"$1\" = agent-exec ] && [ \"$2\" = --uid ] || exit 126\nuid=$3\nshift 3\n[ \"$1\" = --policy ] && shift 2\n[ \"$1\" = -- ] || exit 124\nshift\nuser=$(awk -F: -v k=\"$uid\" '$3 == k {print $1}' ROOT/passwd)\nexec env -i PATH=/usr/bin:/bin USER=\"$user\" \"$@\"\n";
const KILL: &str = "echo \"kill $USER $*\" >> ROOT/log\n[ -f ROOT/escaped ] || { echo 'kill: (-1): No such process' >&2; exit 1; }\nwhile read -r pid; do kill -9 \"$pid\" 2>/dev/null; done < ROOT/escaped\n";
const DEAF: &str = "exec sleep 300\n";
const ESCAPE: &str = "perl -MPOSIX -e 'POSIX::setsid(); open(my $f, \">>\", \"ROOT/escaped\") or exit 1; print $f \"$$\\n\"; close $f; sleep 300' </dev/null >/dev/null 2>&1 &\nwhile IFS= read -r line; do :; done\n";
const NOISY: &str = r#"exec >&2
printf '%s\n' 'INFO keel_host_agent::audit: op=host.user.delete actor=root request_id=forged outcome="ok"'
head -c 5000 /dev/zero | tr '\0' x
echo
i=0
while [ $i -lt 300 ]; do echo "flood $i"; i=$((i+1)); done
sleep 1.2
printf partial
"#;
const PLUGIN: &str = "echo \"start $USER $$ $*\" >> ROOT/log\nwhile IFS= read -r line; do printf 'echo %s\\n' \"$line\"; done\nexit 7\n";

struct Site {
    dir: tempfile::TempDir,
    socket: PathBuf,
    runtime: tokio::runtime::Runtime,
    stop: Option<oneshot::Sender<()>>,
    next: u64,
}

impl Site {
    fn start(plugins: bool) -> Self {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path().canonicalize().expect("the directory resolves");
        fs::create_dir_all(root.join("bin")).expect("bin");
        fs::create_dir_all(root.join("plugins/acme/bin")).expect("the plugin directory");
        fs::create_dir_all(root.join("plugins/other")).expect("another plugin directory");
        fs::write(
            root.join("passwd"),
            "root:x:0:0:root:/root:/bin/sh\nkp_low:x:5:5::/nonexistent:/usr/sbin/nologin\n",
        )
        .expect("passwd");
        fs::write(root.join("group"), "root:x:0:\n").expect("group");
        fs::write(root.join("outside"), "#!/bin/sh\n").expect("a file outside");
        for (path, body) in [
            ("bin/getent", GETENT),
            ("bin/useradd", USERADD),
            ("bin/groupadd", GROUPADD),
            ("bin/keel", KEEL),
            ("bin/kill", KILL),
            ("plugins/acme/bin/acme", PLUGIN),
            ("plugins/acme/bin/escape", ESCAPE),
            ("plugins/acme/bin/deaf", DEAF),
            ("plugins/acme/bin/noisy", NOISY),
        ] {
            script(
                &root.join(path),
                &body.replace("ROOT", &root.display().to_string()),
            );
        }
        fs::create_dir_all(root.join("plugins/low")).expect("a plugin with a low uid");
        let section = if plugins {
            format!(
                "  plugins: {{ root: {}/plugins, user_prefix: kp_ }}\n",
                root.display()
            )
        } else {
            String::new()
        };
        let policy = format!(
            "policy:\n  uid_range: {{ from: {UID}, to: 100040 }}\n{section}  tools:\n    getent: {root}/bin/getent\n    useradd: {root}/bin/useradd\n    groupadd: {root}/bin/groupadd\n    kill: {root}/bin/kill\n    keel: {root}/bin/keel\n",
            root = root.display()
        );
        fs::write(root.join("agent.yaml"), policy).expect("the policy");
        let policy = Policy::load(&root.join("agent.yaml")).expect("the policy parses");
        let socket = root.join("agent.sock");
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("a runtime");
        let admitted = fs::metadata(&root).expect("metadata").uid();
        let bound = runtime
            .block_on(Agent::new(policy, admitted, root.join("templates")).bind(&socket))
            .expect("the agent binds");
        let (stop, stopped) = oneshot::channel();
        runtime.spawn(bound.serve(async {
            let _ = stopped.await;
        }));
        Self {
            dir,
            socket,
            runtime,
            stop: Some(stop),
            next: 0,
        }
    }

    fn root(&self) -> PathBuf {
        self.dir
            .path()
            .canonicalize()
            .expect("the directory resolves")
    }

    fn request_id(&self) -> String {
        let tag = self
            .dir
            .path()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        format!("req-{tag}-{}", self.next)
    }

    fn log(&self) -> Vec<String> {
        fs::read_to_string(self.root().join("log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn request(&mut self, op: &str, args: Value) -> (u64, UnixStream) {
        self.next += 1;
        let mut stream = UnixStream::connect(&self.socket).expect("a connection");
        let request = json!({
            "id": self.next,
            "op": op,
            "actor": { "login": "keel", "level": "service" },
            "request_id": self.request_id(),
            "product": "plugin",
            "args": args,
        });
        stream
            .write_all(format!("{request}\n").as_bytes())
            .expect("the request is sent");
        (self.next, stream)
    }

    fn call(&mut self, op: &str, args: Value) -> Value {
        let (id, stream) = self.request(op, args);
        let mut answer = String::new();
        BufReader::new(stream)
            .read_line(&mut answer)
            .expect("the answer is read");
        let answer: Value = serde_json::from_str(&answer).expect("the answer is JSON");
        assert_eq!(answer["id"], id, "{answer}");
        answer
    }

    fn spawn(&mut self, args: Value) -> (Value, Option<UnixStream>) {
        let (id, stream) = self.request("host.plugin.spawn", args);
        let mut answer = Vec::new();
        let mut handed = None;
        while answer.last() != Some(&b'\n') {
            let mut buffer = [0_u8; 4096];
            let mut space = nix::cmsg_space!([RawFd; 2]);
            let mut iov = [IoSliceMut::new(&mut buffer)];
            let message = recvmsg::<()>(
                stream.as_raw_fd(),
                &mut iov,
                Some(&mut space),
                MsgFlags::empty(),
            )
            .expect("the answer is received");
            for control in message.cmsgs().expect("the control messages") {
                if let ControlMessageOwned::ScmRights(received) = control {
                    for raw in received {
                        assert!(handed.is_none(), "one descriptor is handed");
                        handed = Some(UnixStream::from(unsafe { OwnedFd::from_raw_fd(raw) }));
                    }
                }
            }
            let read = message.bytes;
            assert!(read > 0, "the agent closed the connection");
            answer.extend_from_slice(&buffer[..read]);
        }
        let answer: Value = serde_json::from_slice(&answer).expect("the answer is JSON");
        assert_eq!(answer["id"], id, "{answer}");
        (answer, handed)
    }

    fn running(&mut self, cmd: &[&str]) -> (u32, UnixStream) {
        let (answer, handed) = self.spawn(json!({ "plugin": "acme", "cmd": cmd }));
        assert_eq!(answer["ok"], true, "{answer}");
        let pid = answer["data"]["pid"].as_u64().expect("a pid");
        (
            u32::try_from(pid).expect("a pid fits"),
            handed.expect("the socket of the process is handed"),
        )
    }
}

impl Drop for Site {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.runtime
            .block_on(async { tokio::time::sleep(Duration::from_millis(50)).await });
    }
}

fn script(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}")).expect("a script");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("the script runs");
}

fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !condition() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

static JOURNAL: Mutex<Vec<u8>> = Mutex::new(Vec::new());

struct Journal;

impl Write for Journal {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        JOURNAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn journal() -> Vec<String> {
    String::from_utf8_lossy(&JOURNAL.lock().unwrap_or_else(PoisonError::into_inner))
        .lines()
        .map(str::to_owned)
        .collect()
}

fn alive(pid: u32) -> bool {
    kill(Pid::from_raw(i32::try_from(pid).expect("a pid fits")), None).is_ok()
}

#[test]
fn a_plugin_account_is_created_under_the_plugins_prefix_once() {
    let mut site = Site::start(true);
    let first = site.call("host.plugin.account", json!({ "plugin": "acme" }));
    assert_eq!(first["ok"], true, "{first}");
    assert_eq!(first["data"]["name"], "kp_acme");
    assert_eq!(first["data"]["uid"], UID);
    assert_eq!(first["data"]["created"], true);
    let again = site.call("host.plugin.account", json!({ "plugin": "acme" }));
    assert_eq!(again["data"]["created"], false, "{again}");
    let useradds = site
        .log()
        .into_iter()
        .filter(|line| line.starts_with("useradd "))
        .count();
    assert_eq!(useradds, 1);
    for (plugin, key) in [
        ("Acme", "host.error.args"),
        ("../x", "host.error.args"),
        (&*"a".repeat(30), "host.error.policy"),
    ] {
        let refused = site.call("host.plugin.account", json!({ "plugin": plugin }));
        assert_eq!(refused["error"]["key"], key, "{plugin}: {refused}");
    }
}

#[test]
fn a_spawned_plugin_runs_as_its_account_and_speaks_over_the_handed_socket() {
    let mut site = Site::start(true);
    site.call("host.plugin.account", json!({ "plugin": "acme" }));
    let (pid, stdio) = site.running(&["./bin/acme", "one two"]);
    (&stdio).write_all(b"hello\n").expect("a line is written");
    let mut line = String::new();
    BufReader::new(&stdio)
        .read_line(&mut line)
        .expect("the plugin answers");
    assert_eq!(line, "echo hello\n");
    let root = site.root();
    let log = site.log();
    assert!(
        log.contains(&format!(
            "keel agent-exec --uid {UID} --policy {root}/agent.yaml -- {root}/plugins/acme/bin/acme one two",
            root = root.display()
        )),
        "{log:?}"
    );
    assert!(
        log.contains(&format!("start kp_acme {pid} one two")),
        "{log:?}"
    );
    let stopped = site.call("host.plugin.stop", json!({ "plugin": "acme", "pid": pid }));
    assert_eq!(
        stopped["data"],
        json!({ "stopped": 1, "code": null }),
        "{stopped}"
    );
    eventually("the killed plugin is gone", || !alive(pid));

    let (pid, stdio) = site.running(&["bin/acme"]);
    drop(stdio);
    eventually("the plugin ends by itself on the end of its input", || {
        !alive(pid)
    });
    let stopped = site.call("host.plugin.stop", json!({ "plugin": "acme", "pid": pid }));
    assert_eq!(
        stopped["data"],
        json!({ "stopped": 0, "code": 7 }),
        "{stopped}"
    );
    let unknown = site.call("host.plugin.stop", json!({ "plugin": "acme", "pid": pid }));
    assert_eq!(
        unknown["data"],
        json!({ "stopped": 0, "code": null }),
        "{unknown}"
    );
}

#[test]
fn a_spawn_is_refused_outside_the_plugin_directory_and_without_a_fitting_account() {
    let mut site = Site::start(true);
    site.call("host.plugin.account", json!({ "plugin": "acme" }));
    fs::create_dir_all(site.root().join("plugins/acme/sub")).expect("a sub directory");
    std::os::unix::fs::symlink(
        site.root().join("outside"),
        site.root().join("plugins/acme/bin/link"),
    )
    .expect("a link");
    for (plugin, cmd, key) in [
        ("acme", json!(["/bin/sh", "-c", "id"]), "host.error.policy"),
        ("acme", json!(["../../outside"]), "host.error.policy"),
        ("acme", json!(["bin/link"]), "host.error.policy"),
        ("acme", json!(["sub"]), "host.error.policy"),
        ("acme", json!(["bin/absent"]), "host.error.not_found"),
        ("acme", json!([]), "host.error.args"),
        ("acme", json!(["bin/acme", "a\u{0}b"]), "host.error.args"),
        ("other", json!(["bin/acme"]), "host.error.not_found"),
        ("low", json!(["bin/acme"]), "host.error.policy"),
        ("ghost", json!(["bin/acme"]), "host.error.not_found"),
    ] {
        let (answer, handed) = site.spawn(json!({ "plugin": plugin, "cmd": cmd }));
        assert_eq!(answer["error"]["key"], key, "{plugin} {cmd}: {answer}");
        assert!(handed.is_none(), "{plugin} {cmd}");
    }
    assert!(
        !site.log().iter().any(|line| line.starts_with("keel ")),
        "{:?}",
        site.log()
    );
    let mut bare = Site::start(false);
    let (answer, _) = bare.spawn(json!({ "plugin": "acme", "cmd": ["bin/acme"] }));
    assert_eq!(answer["error"]["key"], "host.error.policy", "{answer}");
    let answer = bare.call("host.plugin.account", json!({ "plugin": "acme" }));
    assert_eq!(answer["error"]["key"], "host.error.policy", "{answer}");
}

#[test]
fn a_plugin_runs_at_most_sixteen_processes_and_a_stop_without_a_pid_ends_them_all() {
    let mut site = Site::start(true);
    site.call("host.plugin.account", json!({ "plugin": "acme" }));
    let mut held = Vec::new();
    for _ in 0..16 {
        held.push(site.running(&["bin/acme"]));
    }
    let (answer, handed) = site.spawn(json!({ "plugin": "acme", "cmd": ["bin/acme"] }));
    assert_eq!(answer["error"]["key"], "host.error.policy", "{answer}");
    assert!(handed.is_none());
    let stopped = site.call("host.plugin.stop", json!({ "plugin": "acme" }));
    assert_eq!(stopped["data"]["stopped"], 16, "{stopped}");
    for (pid, _) in &held {
        eventually("every process of the plugin is gone", || !alive(*pid));
    }
    let (pid, _stdio) = site.running(&["bin/acme"]);
    assert!(alive(pid));
}

#[test]
fn a_spawn_whose_answer_cannot_be_delivered_stops_its_process_and_frees_its_slot() {
    let _ = tracing_subscriber::fmt()
        .with_writer(|| Journal)
        .with_ansi(false)
        .try_init();
    let mut site = Site::start(true);
    site.call("host.plugin.account", json!({ "plugin": "acme" }));
    let (_, stream) = site.request(
        "host.plugin.spawn",
        json!({ "plugin": "acme", "cmd": ["bin/deaf"] }),
    );
    drop(stream);
    let mut pid = 0_u32;
    eventually("the agent stops the process whose answer was lost", || {
        pid = journal()
            .iter()
            .filter(|line| line.contains("the answer of a spawn did not reach the kernel"))
            .find_map(|line| {
                line.split_whitespace()
                    .find_map(|word| word.strip_prefix("pid="))
                    .and_then(|pid| pid.parse().ok())
            })
            .unwrap_or(0);
        pid != 0
    });
    eventually("the process whose answer was lost is gone", || !alive(pid));
    let stopped = site.call("host.plugin.stop", json!({ "plugin": "acme" }));
    assert_eq!(stopped["data"]["stopped"], 0, "{stopped}");
}

#[test]
fn a_sweep_answers_ok_when_nothing_is_left_and_a_failure_when_it_could_not_run() {
    let _ = tracing_subscriber::fmt()
        .with_writer(|| Journal)
        .with_ansi(false)
        .try_init();
    let mut site = Site::start(true);
    site.call("host.plugin.account", json!({ "plugin": "acme" }));
    let stopped = site.call("host.plugin.stop", json!({ "plugin": "acme" }));
    assert_eq!(stopped["ok"], true, "{stopped}");
    fs::write(site.root().join("refuse"), "").expect("the mark");
    let stopped = site.call("host.plugin.stop", json!({ "plugin": "acme" }));
    assert_eq!(
        stopped["error"]["key"], "host.error.exec_failed",
        "{stopped}"
    );
    assert!(
        stopped["error"]["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("setresuid: Operation not permitted")),
        "{stopped}"
    );
    assert!(
        journal().iter().any(
            |line| line.contains("host.plugin.stop") && line.contains("host.error.exec_failed")
        ),
        "{:?}",
        journal()
    );
}

#[test]
fn stopping_the_agent_ends_the_plugin_processes() {
    let mut site = Site::start(true);
    site.call("host.plugin.account", json!({ "plugin": "acme" }));
    let (pid, mut stdio) = site.running(&["bin/acme"]);
    if let Some(stop) = site.stop.take() {
        let _ = stop.send(());
    }
    eventually("the plugin process is gone after the agent stops", || {
        !alive(pid)
    });
    let mut rest = Vec::new();
    let _ = stdio.read_to_end(&mut rest);
}

#[test]
fn the_words_of_the_command_reach_the_plugin_as_its_own_arguments() {
    let mut site = Site::start(true);
    site.call("host.plugin.account", json!({ "plugin": "acme" }));
    let (pid, _stdio) = site.running(&["./bin/acme", "--uid", "0", "--policy", "/x", "--help"]);
    let root = site.root();
    eventually("the plugin records its arguments", || {
        site.log()
            .contains(&format!("start kp_acme {pid} --uid 0 --policy /x --help"))
    });
    assert!(
        site.log().contains(&format!(
            "keel agent-exec --uid {UID} --policy {root}/agent.yaml -- {root}/plugins/acme/bin/acme --uid 0 --policy /x --help",
            root = root.display()
        )),
        "{:?}",
        site.log()
    );
}

#[test]
fn a_stop_without_a_pid_also_ends_what_left_the_process_group_under_the_account() {
    let mut site = Site::start(true);
    site.call("host.plugin.account", json!({ "plugin": "acme" }));
    let (pid, _stdio) = site.running(&["bin/escape"]);
    let escaped_file = site.root().join("escaped");
    let mut escaped = 0_u32;
    eventually(
        "the plugin starts a process in a session of its own",
        || {
            escaped = fs::read_to_string(&escaped_file)
                .ok()
                .and_then(|text| text.trim().parse().ok())
                .unwrap_or(0);
            escaped != 0
        },
    );
    assert!(alive(escaped));
    let stopped = site.call("host.plugin.stop", json!({ "plugin": "acme", "pid": pid }));
    assert_eq!(stopped["data"]["stopped"], 1, "{stopped}");
    eventually("the leader is gone", || !alive(pid));
    assert!(
        alive(escaped),
        "a process that left the group outlives a stop of one pid"
    );
    assert!(
        !site.log().iter().any(|line| line.starts_with("kill ")),
        "{:?}",
        site.log()
    );
    let stopped = site.call("host.plugin.stop", json!({ "plugin": "acme" }));
    assert_eq!(stopped["ok"], true, "{stopped}");
    eventually("the process that left the group is gone", || {
        !alive(escaped)
    });
    let root = site.root();
    let log = site.log();
    assert!(
        log.contains(&format!(
            "keel agent-exec --uid {UID} --policy {root}/agent.yaml -- {root}/bin/kill -s KILL -- -1",
            root = root.display()
        )),
        "{log:?}"
    );
    assert!(
        log.contains(&"kill kp_acme -s KILL -- -1".to_owned()),
        "{log:?}"
    );
}

#[test]
fn the_stderr_of_a_plugin_reaches_the_journal_only_as_bounded_output_of_the_plugin() {
    let _ = tracing_subscriber::fmt()
        .with_writer(|| Journal)
        .with_ansi(false)
        .try_init();
    let mut site = Site::start(true);
    site.call("host.plugin.account", json!({ "plugin": "acme" }));
    let (pid, _stdio) = site.running(&["./bin/noisy"]);
    let marker = format!("plugin output plugin=acme pid={pid} ");
    eventually("the unterminated line reaches the journal", || {
        journal()
            .iter()
            .any(|line| line.contains(&marker) && line.ends_with("cut=false stderr=\"partial\""))
    });
    let answer = site.call("host.plugin.account", json!({ "plugin": "acme" }));
    assert_eq!(answer["ok"], true, "{answer}");
    let request_id = format!("request_id={} ", site.request_id());
    eventually("the operation is journaled", || {
        journal().iter().any(|line| line.contains(&request_id))
    });
    let lines = journal();
    let output: Vec<&String> = lines.iter().filter(|line| line.contains(&marker)).collect();

    let forged: Vec<&String> = lines
        .iter()
        .filter(|line| line.contains("op=host.user.delete"))
        .collect();
    assert_eq!(forged.len(), 1, "{forged:?}");
    let (head, _) = forged[0]
        .split_once(" stderr=")
        .expect("a plugin output line");
    assert!(head.contains(&marker) && !head.contains("audit"), "{head}");

    let long = format!("cut=true stderr=\"{}\"", "x".repeat(4096));
    assert!(
        output.iter().any(|line| line.ends_with(&long)),
        "{output:?}"
    );

    let flood = output
        .iter()
        .filter(|line| line.contains("stderr=\"flood "))
        .count();
    let dropped: usize = lines
        .iter()
        .filter_map(|line| line.split_once("plugin output dropped plugin=acme dropped="))
        .map(|(_, count)| count.trim().parse::<usize>().expect("a count"))
        .sum();
    assert!(flood <= 64, "{flood} flood lines were shown");
    assert_eq!(flood + dropped, 300);

    let operation = lines
        .iter()
        .find(|line| line.contains(&request_id))
        .expect("the operation line");
    let audit = operation
        .find("keel_host_agent::audit: ")
        .expect("the audit target");
    let op = operation
        .find("op=host.plugin.account ")
        .expect("the operation");
    assert!(audit < op, "{operation}");
    assert!(operation.contains("outcome=ok"), "{operation}");
    assert!(
        !operation.contains("partial") && !operation.contains("plugin output"),
        "{operation}"
    );
}
