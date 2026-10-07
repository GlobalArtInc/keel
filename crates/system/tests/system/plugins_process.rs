use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use keel_sdk::prelude::*;
use keel_sdk::types::ErrorCode;
use keel_system::notify::POLL_JOB;
use keel_system::plugins::{
    DIRECTORY, ENABLE, ERROR_FAILING, ERROR_STOPPED, ERROR_UNAVAILABLE, FILE, LIST, MODULE,
    PROBLEM_FAILING,
};
use keel_testkit::{Harness, Reply, Who, tokio};
use tempfile::TempDir;

const PATIENCE: Duration = Duration::from_secs(30);
const ID: &str = "acme";
const FUNCTION: &str = "acme.sync.list";
const ACCOUNT: &str = "kp_acme";
const UID: u32 = 100_000;

const GETENT: &str = "db=$1; shift\n[ \"${1:-}\" = -- ] && shift\ncase $db in passwd) file=HOST/passwd;; group) file=HOST/group;; *) exit 1;; esac\nif [ $# -eq 0 ]; then cat \"$file\"; exit 0; fi\nline=$(awk -F: -v k=\"$1\" '$1 == k || $3 == k' \"$file\")\n[ -n \"$line\" ] || exit 2\nprintf '%s\\n' \"$line\"\n";
const USERADD: &str = "echo \"useradd $*\" >> HOST/log\nuid=; gid=; shell=; home=\nwhile [ $# -gt 1 ]; do\n  case $1 in\n    --uid) uid=$2; shift 2;;\n    --gid) gid=$2; shift 2;;\n    --shell) shell=$2; shift 2;;\n    --home-dir) home=$2; shift 2;;\n    --no-create-home) shift;;\n    *) echo \"useradd: unexpected $1\" >&2; exit 3;;\n  esac\ndone\nprintf '%s:x:%s:%s::%s:%s\\n' \"$1\" \"$uid\" \"$gid\" \"$home\" \"$shell\" >> HOST/passwd\n";
const GROUPADD: &str =
    "echo \"groupadd $*\" >> HOST/log\nprintf '%s:x:%s:\\n' \"$3\" \"$2\" >> HOST/group\n";
const GROUPDEL: &str = "echo \"groupdel $*\" >> HOST/log\n";
const KEEL: &str = "echo \"keel $*\" >> HOST/log\n[ \"$1\" = agent-exec ] && [ \"$2\" = --uid ] || exit 126\nuid=$3\nshift 3\n[ \"$1\" = --policy ] && shift 2\n[ \"$1\" = -- ] || exit 124\nshift\n[ \"$1\" = kill ] && exit 123\nuser=$(awk -F: -v k=\"$uid\" '$3 == k {print $1}' HOST/passwd)\n[ -n \"$user\" ] || exit 125\nexec env -i PATH=/usr/bin:/bin HOME=/nonexistent USER=\"$user\" LOGNAME=\"$user\" \"$@\"\n";
const KILL: &str = "echo \"kill $USER $*\" >> HOST/log\n";
const PLUGIN: &str = "echo \"start $USER $*\" >> MARKS/log\nif [ -f MARKS/crash ]; then rm -f MARKS/crash; echo crash >> MARKS/log; exit 3; fi\nif [ -f MARKS/huge ]; then echo huge >> MARKS/log; head -c 1100000 /dev/zero | tr '\\0' a; fi\nwhile IFS= read -r line; do\n  id=$(printf '%s\\n' \"$line\" | sed -n 's/.*\"id\":\"\\([^\"]*\\)\".*/\\1/p')\n  if [ -f MARKS/silent ]; then echo \"silent $id\" >> MARKS/log; continue; fi\n  if [ -f MARKS/flood ]; then i=0; while [ $i -lt 300 ]; do echo 'not a frame'; i=$((i+1)); done; printf '{\"id\":\"x-%s\",\"ok\":true}\\n' \"$id\"; fi\n  echo \"frame $id\" >> MARKS/log\n  printf '{\"id\":\"%s\",\"ok\":true}\\n' \"$id\"\n  if [ -f MARKS/once ]; then echo once >> MARKS/log; exit 3; fi\ndone\necho eof >> MARKS/log\n";

struct Site {
    dir: TempDir,
}

impl Site {
    fn new(run: Value) -> Self {
        let dir = TempDir::new().expect("a temporary directory");
        let site = Self { dir };
        for sub in ["host", "marks", "data"] {
            fs::create_dir_all(site.path(sub)).expect("a directory of the site");
        }
        fs::write(site.path("host/passwd"), "root:x:0:0:root:/root:/bin/sh\n").expect("passwd");
        fs::write(site.path("host/group"), "root:x:0:\n").expect("group");
        for (name, body) in [
            ("getent", GETENT),
            ("useradd", USERADD),
            ("groupadd", GROUPADD),
            ("groupdel", GROUPDEL),
            ("keel", KEEL),
            ("kill", KILL),
        ] {
            site.script(&format!("host/{name}"), body);
        }
        let home = site.plugin_dir();
        fs::create_dir_all(home.join("bin")).expect("the plugin directory");
        site.script(
            &format!("data/{DIRECTORY}/{ID}/bin/acme"),
            &PLUGIN.replace("MARKS", &site.path("marks").display().to_string()),
        );
        fs::write(home.join(FILE), manifest(run).to_string()).expect("the manifest");
        site
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.dir.path().join(relative)
    }

    fn data(&self) -> PathBuf {
        self.path("data")
    }

    fn plugin_dir(&self) -> PathBuf {
        self.data().join(DIRECTORY).join(ID)
    }

    fn script(&self, relative: &str, body: &str) {
        let path = self.path(relative);
        let body = body.replace("HOST", &self.path("host").display().to_string());
        fs::write(&path, format!("#!/bin/sh\n{body}")).expect("a script");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("the script runs");
    }

    fn mark(&self, name: &str) {
        fs::write(self.path(&format!("marks/{name}")), b"").expect("a mark");
    }

    fn unmark(&self, name: &str) {
        fs::remove_file(self.path(&format!("marks/{name}"))).expect("the mark is removed");
    }

    fn policy(&self) -> String {
        let host = self.path("host");
        format!(
            "policy:\n  uid_range: {{ from: {UID}, to: 100009 }}\n  plugins: {{ root: {plugins}, user_prefix: kp_ }}\n  tools:\n    getent: {host}/getent\n    useradd: {host}/useradd\n    groupadd: {host}/groupadd\n    groupdel: {host}/groupdel\n    kill: {host}/kill\n    keel: {host}/keel\n",
            plugins = self.data().join(DIRECTORY).display(),
            host = host.display(),
        )
    }

    fn host_log(&self) -> Vec<String> {
        lines(&self.path("host/log"))
    }

    fn plugin_log(&self) -> Vec<String> {
        lines(&self.path("marks/log"))
    }

    fn passwd(&self) -> String {
        fs::read_to_string(self.path("host/passwd")).unwrap_or_default()
    }

    fn starts(&self) -> usize {
        self.plugin_log()
            .iter()
            .filter(|line| line.starts_with("start "))
            .count()
    }
}

fn lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn manifest(run: Value) -> Value {
    json!({
        "protocol": 1,
        "id": ID,
        "version": "1.0.0",
        "requires": { "keel": ">=0.4" },
        "run": run,
        "level": "user",
        "functions": [
            {
                "name": FUNCTION,
                "kind": "list",
                "access": { "level": "user" },
                "key": "id",
                "params": { "$ref": "keel://ListQuery" }
            }
        ]
    })
}

fn daemon() -> Value {
    json!({ "mode": "daemon", "cmd": ["./bin/acme", "--serve"], "restart": "always" })
}

fn steady() -> Value {
    json!({ "timeout_ms": 10_000, "failures": 5, "quiet_ms": 1_000, "liveness_ms": 200, "backoff_ms": 50 })
}

async fn boot(site: &Site, config: &Value) -> Harness {
    Harness::new()
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .module(&keel_host::MODULE)
        .module(&MODULE)
        .data_dir(site.data())
        .agent(&site.policy())
        .section("plugin", config.clone())
        .boot()
        .await
}

async fn enabled(site: &Site, config: &Value) -> Harness {
    let harness = boot(site, config).await;
    let reply = harness.call(Who::Admin, ENABLE, json!({ "id": ID })).await;
    assert_eq!(data(&reply)["changed"], json!(true), "{reply:?}");
    eventually("the account of the plugin is created on enable", || {
        site.passwd().contains(&format!("{ACCOUNT}:x:{UID}:{UID}:"))
    })
    .await;
    let database = harness.stop().await;
    Harness::new()
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .module(&keel_host::MODULE)
        .module(&MODULE)
        .data_dir(site.data())
        .agent(&site.policy())
        .section("plugin", config.clone())
        .database(database)
        .boot()
        .await
}

fn data(reply: &Reply) -> &Value {
    reply
        .data()
        .unwrap_or_else(|| panic!("a result was expected, got {:?}", reply.error()))
}

async fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while !condition() {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn answer_key(harness: &Harness) -> (ErrorCode, String) {
    let reply = harness.call(Who::Admin, FUNCTION, json!({})).await;
    let error = reply
        .error()
        .unwrap_or_else(|| panic!("{FUNCTION} answers an error before the frames exist"));
    (error.code, error.key.clone())
}

async fn answers(harness: &Harness, key: &str) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        let (code, found) = answer_key(harness).await;
        if found == key {
            assert_eq!(code, ErrorCode::Unavailable);
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{FUNCTION} answers {found}, not {key}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn failing(harness: &Harness) -> Vec<String> {
    harness
        .database()
        .rows(&format!(
            "SELECT subject || ' ' || severity || ' ' || (params_json->>'reason') FROM notify_problem WHERE key = '{PROBLEM_FAILING}' AND cleared_at IS NULL ORDER BY subject"
        ))
        .await
}

async fn raised(harness: &Harness) -> String {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        if let Some(problem) = failing(harness).await.into_iter().next() {
            return problem;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{PROBLEM_FAILING} was not raised"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn poll(harness: &Harness) {
    let scheduler = harness
        .registry()
        .services()
        .get::<keel_system::cron::api::Scheduler>()
        .expect("the cron module provides the scheduler");
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while !scheduler.is_leader() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the scheduler did not lead"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let started = loop {
        match scheduler.run_now(POLL_JOB).await {
            Ok(started) => break started,
            Err(error) => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "the poll did not start: {error:?}"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    };
    tokio::time::timeout(PATIENCE, scheduler.wait(started.run))
        .await
        .expect("the poll ends in time")
        .expect("the outcome is read");
}

async fn enabled_without_account(site: &Site, config: &Value) -> Harness {
    let harness = Harness::new()
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .module(&MODULE)
        .data_dir(site.data())
        .boot()
        .await;
    let reply = harness.call(Who::Admin, ENABLE, json!({ "id": ID })).await;
    assert_eq!(data(&reply)["changed"], json!(true), "{reply:?}");
    let database = harness.stop().await;
    assert!(!site.passwd().contains(ACCOUNT), "{}", site.passwd());
    Harness::new()
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .module(&keel_host::MODULE)
        .module(&MODULE)
        .data_dir(site.data())
        .agent(&site.policy())
        .section("plugin", config.clone())
        .database(database)
        .boot()
        .await
}

fn frames(site: &Site) -> usize {
    site.plugin_log()
        .iter()
        .filter(|line| line.starts_with("frame k-"))
        .count()
}

#[tokio::test]
async fn a_daemon_plugin_runs_under_its_own_account() {
    let site = Site::new(daemon());
    let harness = enabled(&site, &steady()).await;
    assert!(
        site.host_log().iter().any(|line| line
            == &format!(
                "useradd --uid {UID} --gid {UID} --shell /usr/sbin/nologin --home-dir /nonexistent --no-create-home {ACCOUNT}"
            )),
        "{:?}",
        site.host_log()
    );
    eventually("the plugin answers two liveness frames", || {
        frames(&site) >= 2
    })
    .await;
    let log = site.plugin_log();
    assert_eq!(log[0], format!("start {ACCOUNT} --serve"), "{log:?}");
    assert_eq!(site.starts(), 1, "{log:?}");
    let program = site
        .plugin_dir()
        .canonicalize()
        .expect("the plugin directory resolves")
        .join("bin/acme");
    let spawned: Vec<String> = site
        .host_log()
        .into_iter()
        .filter(|line| line.starts_with("keel ") && !line.contains("/kill "))
        .collect();
    assert_eq!(spawned.len(), 1, "{spawned:?}");
    assert!(
        spawned[0].starts_with(&format!("keel agent-exec --uid {UID} --policy "))
            && spawned[0].ends_with(&format!(" -- {} --serve", program.display())),
        "{spawned:?}"
    );
    answers(&harness, ERROR_UNAVAILABLE).await;
    assert_eq!(failing(&harness).await, Vec::<String>::new());
    harness.finish().await;
}

#[tokio::test]
async fn a_crash_of_a_daemon_plugin_does_not_take_the_kernel_down_and_it_is_restarted() {
    let site = Site::new(daemon());
    site.mark("crash");
    let harness = enabled(&site, &steady()).await;
    eventually("the restarted plugin answers a liveness frame", || {
        let log = site.plugin_log();
        log.iter()
            .position(|line| line == "crash")
            .is_some_and(|crashed| {
                log[crashed..]
                    .iter()
                    .any(|line| line.starts_with("frame k-"))
            })
    })
    .await;
    let log = site.plugin_log();
    assert_eq!(
        &log[..3],
        [
            format!("start {ACCOUNT} --serve"),
            "crash".to_owned(),
            format!("start {ACCOUNT} --serve"),
        ],
        "{log:?}"
    );
    let listed = harness.call(Who::Admin, LIST, json!({})).await;
    assert!(
        data(&listed)["data"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["id"] == json!(ID))),
        "{listed:?}"
    );
    answers(&harness, ERROR_UNAVAILABLE).await;
    assert_eq!(failing(&harness).await, Vec::<String>::new());
    harness.finish().await;
}

#[tokio::test]
async fn repeated_timeouts_open_the_breaker_and_raise_a_problem() {
    let site = Site::new(daemon());
    site.mark("silent");
    let config = json!({ "timeout_ms": 300, "failures": 3, "quiet_ms": 600_000, "liveness_ms": 100, "backoff_ms": 50 });
    let harness = enabled(&site, &config).await;
    let problem = raised(&harness).await;
    assert_eq!(
        problem,
        format!("{ID} warning a frame got no answer within 300 ms")
    );
    answers(&harness, ERROR_FAILING).await;
    assert!(
        site.plugin_log()
            .iter()
            .filter(|line| line.starts_with("silent k-"))
            .count()
            >= 2,
        "{:?}",
        site.plugin_log()
    );
    let starts = site.starts();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(site.starts(), starts, "an open breaker starts no process");
    poll(&harness).await;
    assert_eq!(failing(&harness).await, [problem]);
    harness.finish().await;
}

#[tokio::test]
async fn the_breaker_closes_after_a_quiet_period_once_the_plugin_answers() {
    let site = Site::new(daemon());
    site.mark("silent");
    let config = json!({ "timeout_ms": 1_000, "failures": 2, "quiet_ms": 1_500, "liveness_ms": 100, "backoff_ms": 50 });
    let harness = enabled(&site, &config).await;
    raised(&harness).await;
    answers(&harness, ERROR_FAILING).await;
    site.unmark("silent");
    answers(&harness, ERROR_UNAVAILABLE).await;
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while !failing(&harness).await.is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the problem is not cleared"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(site.starts() >= 2, "{:?}", site.plugin_log());
    harness.finish().await;
}

#[tokio::test]
async fn a_daemon_that_is_never_restarted_stays_stopped_after_a_crash() {
    let site = Site::new(
        json!({ "mode": "daemon", "cmd": ["./bin/acme", "--serve"], "restart": "never" }),
    );
    site.mark("crash");
    let harness = enabled(&site, &steady()).await;
    answers(&harness, ERROR_STOPPED).await;
    assert_eq!(
        raised(&harness).await,
        format!("{ID} warning its process ended with the code 3")
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(site.starts(), 1, "{:?}", site.plugin_log());
    harness.finish().await;
}

#[tokio::test]
async fn an_exec_plugin_runs_a_process_for_its_liveness_frame() {
    let site = Site::new(json!({ "mode": "exec", "cmd": ["./bin/acme"] }));
    let harness = enabled(&site, &steady()).await;
    eventually("the exec process answers its liveness frame", || {
        site.plugin_log()
            .starts_with(&[format!("start {ACCOUNT} "), "frame k-1".to_owned()])
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(site.starts(), 1, "{:?}", site.plugin_log());
    answers(&harness, ERROR_UNAVAILABLE).await;
    harness.finish().await;
}

#[tokio::test]
async fn an_exec_plugin_without_its_account_gets_one_from_the_supervisor() {
    let site = Site::new(json!({ "mode": "exec", "cmd": ["./bin/acme"] }));
    let harness = enabled_without_account(&site, &steady()).await;
    eventually("the exec process answers its liveness frame", || {
        site.plugin_log()
            .starts_with(&[format!("start {ACCOUNT} "), "frame k-1".to_owned()])
    })
    .await;
    assert!(
        site.passwd().contains(&format!("{ACCOUNT}:x:{UID}:{UID}:")),
        "{}",
        site.passwd()
    );
    answers(&harness, ERROR_UNAVAILABLE).await;
    assert_eq!(failing(&harness).await, Vec::<String>::new());
    harness.finish().await;
}

#[tokio::test]
async fn a_daemon_plugin_without_its_account_gets_one_from_the_supervisor() {
    let site = Site::new(daemon());
    let harness = enabled_without_account(&site, &steady()).await;
    eventually("the daemon answers its liveness frame", || {
        frames(&site) >= 1
    })
    .await;
    assert!(
        site.passwd().contains(&format!("{ACCOUNT}:x:{UID}:{UID}:")),
        "{}",
        site.passwd()
    );
    answers(&harness, ERROR_UNAVAILABLE).await;
    assert_eq!(failing(&harness).await, Vec::<String>::new());
    harness.finish().await;
}

#[tokio::test]
async fn without_a_host_agent_the_plugin_is_stopped_with_a_problem() {
    let site = Site::new(daemon());
    let harness = Harness::new()
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .module(&MODULE)
        .data_dir(site.data())
        .boot()
        .await;
    let reply = harness.call(Who::Admin, ENABLE, json!({ "id": ID })).await;
    assert_eq!(data(&reply)["changed"], json!(true), "{reply:?}");
    let database = harness.stop().await;
    let harness = Harness::new()
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .module(&MODULE)
        .data_dir(site.data())
        .database(database)
        .boot()
        .await;
    answers(&harness, ERROR_STOPPED).await;
    poll(&harness).await;
    assert_eq!(
        failing(&harness).await,
        [format!(
            "{ID} warning no host agent is composed into this installation, so its process cannot start"
        )]
    );
    assert!(!site.passwd().contains(ACCOUNT));
    assert_eq!(site.starts(), 0);
    harness.finish().await;
}

#[tokio::test]
async fn repeated_crashes_after_an_answer_open_the_breaker_and_raise_a_problem() {
    let site = Site::new(daemon());
    site.mark("once");
    let config = json!({ "timeout_ms": 5_000, "failures": 3, "quiet_ms": 600_000, "liveness_ms": 100, "backoff_ms": 50 });
    let harness = enabled(&site, &config).await;
    let problem = raised(&harness).await;
    assert!(
        problem.starts_with(&format!("{ID} warning its process ended")),
        "{problem}"
    );
    answers(&harness, ERROR_FAILING).await;
    let log = site.plugin_log();
    assert_eq!(site.starts(), 3, "{log:?}");
    assert_eq!(frames(&site), 3, "{log:?}");
    assert_eq!(
        log.iter().filter(|line| *line == "once").count(),
        3,
        "{log:?}"
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(site.starts(), 3, "an open breaker starts no process");
    let host = site.host_log();
    let last = host
        .iter()
        .rposition(|line| line.starts_with("keel ") && line.ends_with("/bin/acme --serve"))
        .expect("the host log holds the starts of the plugin");
    let sweep = format!("-- {} -s KILL -- -1", site.path("host/kill").display());
    assert!(
        host[last..].iter().any(|line| {
            line.starts_with(&format!("keel agent-exec --uid {UID} --policy "))
                && line.ends_with(&sweep)
        }) && host[last..].contains(&format!("kill {ACCOUNT} -s KILL -- -1")),
        "the breaker opened by a crash ends every process of the account: {host:?}"
    );
    harness.finish().await;
}

#[tokio::test]
async fn a_stop_while_a_frame_waits_for_its_answer_ends_promptly_and_counts_no_failure() {
    let site = Site::new(daemon());
    site.mark("silent");
    let config = json!({ "timeout_ms": 5_000, "failures": 1, "quiet_ms": 600_000, "liveness_ms": 100, "backoff_ms": 50 });
    let harness = enabled(&site, &config).await;
    eventually("a liveness frame waits for its answer", || {
        site.plugin_log()
            .iter()
            .any(|line| line.starts_with("silent k-"))
    })
    .await;
    let since = tokio::time::Instant::now();
    let database = harness.stop().await;
    let took = since.elapsed();
    assert!(took < Duration::from_secs(3), "the stop took {took:?}");
    assert_eq!(
        database
            .rows(&format!(
                "SELECT key FROM notify_problem WHERE key = '{PROBLEM_FAILING}'"
            ))
            .await,
        Vec::<String>::new()
    );
    database.finish().await;
}

#[tokio::test]
async fn the_words_of_the_command_reach_the_plugin_as_its_own_arguments() {
    let site = Site::new(json!({
        "mode": "daemon",
        "cmd": ["./bin/acme", "--uid", "0", "--policy", "/x", "--help"],
        "restart": "always"
    }));
    let harness = enabled(&site, &steady()).await;
    eventually("the plugin answers a liveness frame", || frames(&site) >= 1).await;
    let log = site.plugin_log();
    assert_eq!(
        log[0],
        format!("start {ACCOUNT} --uid 0 --policy /x --help"),
        "{log:?}"
    );
    answers(&harness, ERROR_UNAVAILABLE).await;
    harness.finish().await;
}

#[tokio::test]
async fn a_frame_line_over_the_limit_closes_the_output_and_counts_as_a_failure() {
    let site = Site::new(daemon());
    site.mark("huge");
    let config = json!({ "timeout_ms": 10_000, "failures": 2, "quiet_ms": 600_000, "liveness_ms": 100, "backoff_ms": 50 });
    let harness = enabled(&site, &config).await;
    let problem = raised(&harness).await;
    assert!(
        problem.starts_with(&format!("{ID} warning its process ended")),
        "{problem}"
    );
    answers(&harness, ERROR_FAILING).await;
    let log = site.plugin_log();
    assert_eq!(site.starts(), 2, "{log:?}");
    assert_eq!(
        log.iter().filter(|line| *line == "huge").count(),
        2,
        "{log:?}"
    );
    assert_eq!(frames(&site), 0, "{log:?}");
    harness.finish().await;
}

#[tokio::test]
async fn lines_that_are_no_frame_or_answer_nothing_waiting_are_dropped() {
    let site = Site::new(daemon());
    site.mark("flood");
    let harness = enabled(&site, &steady()).await;
    eventually(
        "the plugin answers three liveness frames among the noise",
        || frames(&site) >= 3,
    )
    .await;
    assert_eq!(site.starts(), 1, "{:?}", site.plugin_log());
    answers(&harness, ERROR_UNAVAILABLE).await;
    assert_eq!(failing(&harness).await, Vec::<String>::new());
    harness.finish().await;
}
