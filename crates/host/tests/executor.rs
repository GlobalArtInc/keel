use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

use keel_host::api::{Attributes, CronJob, Dest, HostExecutor, Origin};
use keel_sdk::prelude::*;
use keel_sdk::types::{Envelope, ErrorCode};
use keel_testkit::{Harness, Who};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tracing_subscriber::fmt::MakeWriter;

const PRODUCT: &str = "probe";
const PAGE_TEMPLATE: &str = "probe/page.j2";
const PAGE_CHECK: &str = "page.check";

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
            .with_max_level(tracing::Level::INFO)
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

#[derive(Deserialize, Serialize, JsonSchema)]
struct Page {
    name: String,
    title: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Published {
    name: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Previewed {
    changed: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Nothing {}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Presence {
    executor: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct SizeArgs {
    name: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
struct Size {
    product: String,
    bytes: u64,
}

fn page_origin(principal: &Principal, request_id: &str) -> Origin {
    Origin::new(PRODUCT, principal, request_id)
}

async fn apply_page(host: &dyn HostExecutor, origin: Origin, page: &Page) -> Result<bool> {
    host.on(origin)
        .template(PAGE_TEMPLATE)
        .vars(&json!({ "title": page.title }))
        .dest("pages", &page.name)
        .mode("0640")
        .validate(PAGE_CHECK)
        .apply()
        .await
}

fn size(scope: &dyn AgentScope, args: SizeArgs) -> std::result::Result<Size, AgentOpError> {
    let root = scope
        .policy()
        .get("root")
        .and_then(Value::as_str)
        .ok_or_else(|| AgentOpError::new("host.error.policy", "the class probe names no root"))?;
    let path = scope.resolve(root, &args.name)?;
    let meta = fs::metadata(&path)
        .map_err(|error| AgentOpError::new("host.error.io", error.to_string()))?;
    Ok(Size {
        product: scope.product().to_owned(),
        bytes: meta.len(),
    })
}

static PROBE: ModuleDef = ModuleDef {
    id: "probe",
    version: "0.1.0",
    sdk: VERSION,
    requires: &["host"],
    migrations: None,
    init: probe,
};

fn probe(r: &mut Registrar) -> Result<()> {
    r.require::<dyn HostExecutor>()
        .contribute(
            ConfigTemplate::new(PAGE_TEMPLATE, "<h1>{{ title }}</h1>\n").validate(PAGE_CHECK),
        )
        .contribute(AgentOp::new::<SizeArgs, Size>("probe.size", size).policy("probe"));
    r.function(
        function::<Page, Published>("probe.publish")
            .kind(Kind::Action)
            .level(Level::User)
            .unowned()
            .handler(|call: Call<Page>| async move {
                let host = call.require::<dyn HostExecutor>()?;
                let page = Page {
                    name: call.params().name.clone(),
                    title: call.params().title.clone(),
                };
                let name = page.name.clone();
                call.after_commit(move |ctx| async move {
                    let origin = page_origin(ctx.principal(), ctx.request_id());
                    apply_page(host.as_ref(), origin, &page).await.map(drop)
                });
                Ok(Published { name })
            }),
    );
    r.function(
        function::<Page, Previewed>("probe.preview")
            .kind(Kind::Action)
            .mode(Mode::Read)
            .level(Level::User)
            .unowned()
            .handler(|call: Call<Page>| async move {
                let host = call.require::<dyn HostExecutor>()?;
                let origin = page_origin(call.principal(), call.request_id());
                let changed = apply_page(host.as_ref(), origin, call.params()).await?;
                Ok(Previewed { changed })
            }),
    );
    Ok(())
}

static STAND_IN: ModuleDef = ModuleDef {
    id: "host",
    version: "0.1.0",
    sdk: VERSION,
    requires: &[],
    migrations: None,
    init: |_| Ok(()),
};

static OBSERVER: ModuleDef = ModuleDef {
    id: "observer",
    version: "0.1.0",
    sdk: VERSION,
    requires: &[],
    migrations: None,
    init: observer,
};

fn observer(r: &mut Registrar) -> Result<()> {
    r.function(
        function::<Nothing, Presence>("observer.look")
            .kind(Kind::Get)
            .level(Level::User)
            .unowned()
            .handler(|call: Call<Nothing>| async move {
                Ok(Presence {
                    executor: call.require::<dyn HostExecutor>().is_ok(),
                })
            }),
    );
    Ok(())
}

struct Site {
    dir: tempfile::TempDir,
}

impl Site {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        for sub in ["pages", "bin", "cron.d"] {
            fs::create_dir(dir.path().join(sub)).unwrap();
        }
        let site = Self { dir };
        let pages = site.path("pages");
        site.stub(
            "page-check",
            &format!(
                "if grep -rl broken {} >/dev/null 2>&1; then echo \"page-check: a broken page in {}\"; exit 1; fi\nexit 0",
                pages.display(),
                pages.display()
            ),
        );
        let login = current_login();
        site.stub(
            "getent",
            &format!(
                "if [ \"$3\" = \"{login}\" ]; then echo \"{login}:x:$(id -u):$(id -g)::/tmp:/bin/sh\"; exit 0; fi\nexit 2"
            ),
        );
        let calls = site.path("systemctl.calls");
        site.stub(
            "systemctl",
            &format!(
                "echo \"$@\" >> {}\nif [ \"$1\" = is-active ]; then echo active; fi\nexit 0",
                calls.display()
            ),
        );
        site.stub("stamp", "echo \"stamped $1\"");
        site
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn stub(&self, name: &str, body: &str) {
        let path = self.path("bin").join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn policy(&self) -> String {
        let bin = self.path("bin");
        format!(
            "policy:
  file_roots: {{ pages: {pages} }}
  services: [probe-web]
  checks: {{ {PAGE_CHECK}: [\"{bin}/page-check\"] }}
  exec_allow: [{{ stamp: [\"{bin}/stamp\", \"--fixed\"] }}]
  products:
    {PRODUCT}: {{ user_prefix: pr_, drop_in_prefix: keel-probe-, classes: {{ probe: {{ root: pages }} }} }}
  tools: {{ getent: {bin}/getent, systemctl: {bin}/systemctl, cron_dir: {cron} }}
",
            pages = self.path("pages").display(),
            bin = bin.display(),
            cron = self.path("cron.d").display(),
        )
    }
}

fn current_login() -> String {
    let output = std::process::Command::new("id")
        .arg("-un")
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn request_id(envelope: &Envelope<Value>) -> String {
    match envelope {
        Envelope::Ok { meta, .. } => meta.request_id.clone(),
        Envelope::Err { error } => panic!("the call failed: {error:?}"),
    }
}

async fn eventually(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..100 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{what} did not happen within five seconds");
}

#[tokio::test]
async fn a_module_applies_a_template_through_the_executor_and_the_agent_audits_the_request_id() {
    capture_journal();
    let site = Site::new();
    let harness = Harness::new()
        .module(&keel_host::MODULE)
        .module(&PROBE)
        .agent(&site.policy())
        .boot()
        .await;
    let reply = harness
        .call(
            Who::A1,
            "probe.publish",
            json!({ "name": "keel-probe-home", "title": "Home" }),
        )
        .await;
    let request = request_id(&reply.envelope);
    let page = site.path("pages").join("keel-probe-home");
    eventually("the after-commit render", || page.exists()).await;
    assert_eq!(fs::read_to_string(&page).unwrap(), "<h1>Home</h1>\n");
    assert_eq!(
        fs::metadata(&page).unwrap().permissions().mode() & 0o777,
        0o640
    );
    let login = harness.principal(Who::A1).login.clone();
    let marker = format!("request_id={request}");
    eventually("the audit line of the apply", || {
        journal().lines().any(|line| line.contains(&marker))
    })
    .await;
    let text = journal();
    let line = text
        .lines()
        .find(|line| line.contains(&marker))
        .unwrap_or_else(|| panic!("no audit line with {marker} in:\n{text}"));
    assert!(line.contains("keel_host_agent::audit"), "{line}");
    assert!(line.contains("op=host.template.apply"), "{line}");
    assert!(line.contains(&format!("actor={login}")), "{line}");
    assert!(line.contains("level=user"), "{line}");
    assert!(line.contains(&format!("product={PRODUCT}")), "{line}");
    assert!(line.contains("outcome=ok"), "{line}");
    harness.finish().await;
}

#[tokio::test]
async fn an_agent_error_surfaces_in_the_envelope_as_its_key_with_the_detail() {
    capture_journal();
    let site = Site::new();
    let harness = Harness::new()
        .module(&keel_host::MODULE)
        .module(&PROBE)
        .agent(&site.policy())
        .boot()
        .await;
    let reply = harness
        .call(
            Who::A1,
            "probe.preview",
            json!({ "name": "keel-probe-draft", "title": "broken" }),
        )
        .await;
    let Envelope::Err { error } = reply.envelope else {
        panic!("a failing check must fail the call");
    };
    assert_eq!(error.code, ErrorCode::Validation);
    assert_eq!(error.key, "host.error.validate_failed");
    let detail = error.params["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("page-check: a broken page"), "{detail}");
    assert!(error.request_id.is_some());
    assert!(!site.path("pages").join("keel-probe-draft").exists());

    let reply = harness
        .call(
            Who::A1,
            "probe.preview",
            json!({ "name": "home", "title": "Home" }),
        )
        .await;
    let Envelope::Err { error } = reply.envelope else {
        panic!("a name outside the drop-in prefix must be refused");
    };
    assert_eq!(error.code, ErrorCode::Forbidden);
    assert_eq!(error.key, "host.error.policy");
    let detail = error.params["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("keel-probe-"), "{detail}");

    let reply = harness
        .call(
            Who::A1,
            "probe.preview",
            json!({ "name": "keel-probe-draft", "title": "Draft" }),
        )
        .await;
    let Envelope::Ok { data, .. } = reply.envelope else {
        panic!("a passing check applies the page");
    };
    assert_eq!(data, json!({ "changed": true }));
    harness.finish().await;
}

#[tokio::test]
async fn a_module_requiring_host_without_a_provider_fails_the_freeze() {
    let without = Harness::new().module(&PROBE).try_boot().await.unwrap_err();
    let text = without.to_string();
    assert!(text.contains("requires host"), "{text}");

    let stand_in = Harness::new()
        .module(&STAND_IN)
        .module(&PROBE)
        .try_boot()
        .await
        .unwrap_err();
    let text = stand_in.to_string();
    assert!(
        text.contains("module probe requires the service") && text.contains("HostExecutor"),
        "{text}"
    );
    assert!(text.contains("which no module provides"), "{text}");

    let with = Harness::new()
        .module(&keel_host::MODULE)
        .module(&PROBE)
        .try_boot()
        .await
        .unwrap();
    with.finish().await;
}

struct FakeAgent {
    socket: PathBuf,
    accepted: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl FakeAgent {
    fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let accepted = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&accepted);
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                counter.fetch_add(1, Ordering::SeqCst);
                let (read, mut write) = stream.into_split();
                let mut line = String::new();
                if BufReader::new(read).read_line(&mut line).await.is_err() {
                    continue;
                }
                let id = keel_sdk::serde_json::from_str::<Value>(&line)
                    .ok()
                    .and_then(|request| request["id"].as_u64())
                    .unwrap_or(0);
                let answer =
                    format!("{{\"id\":{id},\"ok\":true,\"data\":{{\"changed\":false}}}}\n");
                write.write_all(answer.as_bytes()).await.ok();
            }
        });
        Self {
            socket,
            accepted,
            task,
            _dir: dir,
        }
    }

    fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    fn section(&self) -> Value {
        json!({ "socket": self.socket })
    }
}

fn socket_of(section: &Value) -> &Path {
    Path::new(section["socket"].as_str().unwrap())
}

#[tokio::test]
async fn without_host_loaded_the_kernel_never_connects_to_the_socket() {
    let agent = FakeAgent::start();
    assert!(socket_of(&agent.section()).exists());

    let bare = Harness::new()
        .module(&OBSERVER)
        .section("host", agent.section())
        .boot()
        .await;
    let reply = bare.call(Who::A1, "observer.look", json!({})).await;
    let Envelope::Ok { data, .. } = reply.envelope else {
        panic!("observer.look answers");
    };
    assert_eq!(data, json!({ "executor": false }));
    for who in [Who::Owner, Who::Admin, Who::A1] {
        bare.call(who, "observer.look", json!({})).await;
    }
    bare.finish().await;
    assert_eq!(agent.accepted(), 0);

    let loaded = Harness::new()
        .module(&keel_host::MODULE)
        .module(&PROBE)
        .section("host", agent.section())
        .boot()
        .await;
    assert_eq!(
        agent.accepted(),
        0,
        "the host module does not connect at start"
    );
    let reply = loaded
        .call(
            Who::A1,
            "probe.preview",
            json!({ "name": "keel-probe-home", "title": "Home" }),
        )
        .await;
    let Envelope::Ok { data, .. } = reply.envelope else {
        panic!("probe.preview reaches the socket");
    };
    assert_eq!(data, json!({ "changed": false }));
    assert_eq!(agent.accepted(), 1);
    loaded.finish().await;
    agent.task.abort();
}

#[tokio::test]
async fn the_typed_client_speaks_the_catalogue_of_the_agent() {
    capture_journal();
    let site = Site::new();
    let harness = Harness::new()
        .module(&keel_host::MODULE)
        .module(&PROBE)
        .agent(&site.policy())
        .boot()
        .await;
    let host = harness
        .registry()
        .services()
        .get::<dyn HostExecutor>()
        .expect("the host module provides the executor");
    let principal = harness.principal(Who::Admin).clone();
    let client = host.on(page_origin(&principal, "req-typed"));

    let fs = client.fs();
    let notes = Dest::new("pages", "keel-probe-notes");
    assert!(
        fs.mkdir(&notes, &Attributes::default().mode("0750"))
            .await
            .unwrap()
    );
    let file = Dest::new("pages", "keel-probe-readme");
    assert!(
        fs.write(&file, "read me\n", &Attributes::default().mode("0600"))
            .await
            .unwrap()
    );
    assert!(
        !fs.write(&file, "read me\n", &Attributes::default().mode("0600"))
            .await
            .unwrap()
    );
    let content = fs.read(&file).await.unwrap();
    assert_eq!(content.content, "read me\n");
    assert_eq!(content.mode, "0600");
    let names: Vec<String> = fs
        .list("pages")
        .await
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    assert_eq!(names, ["keel-probe-notes", "keel-probe-readme"]);
    let sized: Size = client
        .op(
            "probe.size",
            &SizeArgs {
                name: "keel-probe-readme".to_owned(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        sized,
        Size {
            product: PRODUCT.to_owned(),
            bytes: 8
        }
    );
    assert!(fs.remove(&file, false).await.unwrap());
    assert!(!fs.remove(&file, false).await.unwrap());

    let services = client.services();
    services.reload("probe-web", None).await.unwrap();
    services.restart("probe-web", None).await.unwrap();
    let states = services.list().await.unwrap();
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].name, "probe-web");
    assert_eq!(states[0].state, "active");
    let refused = services.reload("sshd", None).await.unwrap_err();
    assert_eq!(refused.key(), "host.error.policy");
    let recorded = fs::read_to_string(site.path("systemctl.calls")).unwrap();
    assert!(recorded.contains("reload probe-web"), "{recorded}");
    assert!(recorded.contains("restart probe-web"), "{recorded}");

    let cron = client.cron();
    let jobs = [CronJob {
        schedule: "@daily".to_owned(),
        user: current_login(),
        exec: "stamp".to_owned(),
    }];
    assert!(cron.write("keel-probe-nightly", &jobs).await.unwrap());
    assert!(!cron.write("keel-probe-nightly", &jobs).await.unwrap());
    assert_eq!(cron.list().await.unwrap(), ["keel-probe-nightly"]);
    assert!(cron.remove("keel-probe-nightly").await.unwrap());

    assert_eq!(client.exec("stamp").await.unwrap(), "stamped --fixed\n");
    let unknown = client.exec("rm").await.unwrap_err();
    assert_eq!(unknown.key(), "host.error.policy");

    let users = client.users().list().await.unwrap_err();
    assert_eq!(users.key(), "host.error.policy");
    let detail = users.params()["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("uid_range"), "{detail}");

    let text = journal();
    assert!(
        text.lines()
            .filter(|line| line.contains("request_id=req-typed"))
            .count()
            >= 15,
        "{text}"
    );
    harness.finish().await;
}
