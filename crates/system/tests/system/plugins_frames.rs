use std::time::Duration;

use keel_sdk::Registrar;
use keel_sdk::prelude::*;
use keel_sdk::serde_json;
use keel_sdk::types::{ErrorCode, Scope};
use keel_system::plugins::{
    ERROR_ANSWER, ERROR_FAILING, ERROR_FRAME, ERROR_INVOKE, ERROR_TOO_LARGE, ERROR_UNAVAILABLE,
    PROBLEM_FAILING,
};
use keel_testkit::{Harness, Reply, Who, tokio};

use crate::plugins_process::{
    ACCOUNT, ID, PATIENCE, Site, data, enabled_with, eventually, failing, poll, raised,
};

const PLUGIN: &str = r#"echo "start $USER $*" >> MARKS/log
stray() {
  printf '%s\n' "$2"
  while IFS= read -r answer; do
    case "$answer" in
      "{\"id\":\"$1\","*) printf 'stray %s\n' "$answer" >> MARKS/log; return;;
    esac
    printf '{"id":"%s","ok":true}\n' "$(printf '%s\n' "$answer" | sed -n 's/^{"id":"\([^"]*\)".*/\1/p')"
  done
}
if [ -f MARKS/stray ]; then
  stray p-1 '{"id":"p-1","t":"shout"}'
  stray p-2 '{"id":"p-2","t":"invoke","func":"probe.who.get","params":{}}'
fi
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/^{"id":"\([^"]*\)".*/\1/p')
  t=$(printf '%s\n' "$line" | sed -n 's/^{"id":"[^"]*","t":"\([^"]*\)".*/\1/p')
  func=$(printf '%s\n' "$line" | sed -n 's/^{"id":"[^"]*","t":"[^"]*","func":"\([^"]*\)".*/\1/p')
  [ "$t" = ping ] || printf '%s\n' "$line" >> MARKS/frames
  case "$t:$func" in
    call:acme.sync.list)
      printf '{"id":"%s","ok":true,"data":{"data":[{"id":"one","name":"first"}],"total":1}}\n' "$id";;
    call:acme.sync.peek)
      target=$(printf '%s\n' "$line" | sed -n 's/.*"target":"\([^"]*\)".*/\1/p')
      printf '{"id":"%s","t":"invoke","func":"%s","params":{}}\n' "$id" "$target"
      IFS= read -r answer
      printf '{"id":"%s","ok":true,"data":%s}\n' "$id" "$answer";;
    call:acme.sync.slow) ;;
    call:acme.sync.broken)
      printf '{"id":"%s","ok":false,"error":{"code":"unauthenticated","key":"keel.error.unauthenticated"}}\n' "$id";;
    call:acme.sync.refuse)
      printf '{"id":"%s","ok":false,"error":{"code":"not_found","key":"acme.error.gone","params":{"name":"a.org"}}}\n' "$id";;
    call:acme.mirror.poll)
      printf '{"id":"%s","ok":true,"data":[{"key":"acme.mirror.lagging","subject":"mirror","severity":"warning","params":{"behind":3}}]}\n' "$id";;
    before:probe.item.delete)
      if printf '%s\n' "$line" | grep -q '"linked"'; then
        printf '{"id":"%s","ok":false,"error":{"key":"acme.error.linked","params":{"name":"linked"},"field":"ids"}}\n' "$id"
      else
        printf '{"id":"%s","ok":true,"params":{"checked":true}}\n' "$id"
      fi;;
    *) printf '{"id":"%s","ok":true}\n' "$id";;
  esac
done
"#;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Nothing {}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Items {
    ids: Vec<String>,
    #[serde(default)]
    checked: Option<bool>,
}

static PROBE: ModuleDef = ModuleDef {
    id: "probe",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: probe_init,
};

fn probe_init(r: &mut Registrar) -> Result<()> {
    r.function(
        function::<Nothing, Value>("probe.who.get")
            .kind(Kind::Get)
            .level(Level::User)
            .unowned()
            .handler(|call: Call<Nothing>| async move {
                let principal = call.principal();
                Ok(json!({
                    "login": principal.login,
                    "level": principal.level,
                    "account": principal.account_id,
                }))
            }),
    )
    .function(
        function::<Nothing, Value>("probe.secret.get")
            .kind(Kind::Get)
            .level(Level::Admin)
            .unowned()
            .handler(|_: Call<Nothing>| async { Ok(json!({ "secret": true })) }),
    )
    .function(
        function::<Items, Value>("probe.item.delete")
            .kind(Kind::Action)
            .level(Level::User)
            .unowned()
            .handler(|call: Call<Items>| async move {
                let params = call.params();
                Ok(json!({ "deleted": params.ids, "checked": params.checked }))
            }),
    );
    Ok(())
}

fn action(name: &str) -> Value {
    json!({
        "name": name,
        "kind": "action",
        "mode": "read",
        "access": { "level": "user" },
        "params": { "type": "object", "properties": { "target": { "type": "string" } } }
    })
}

fn manifest(mode: &str) -> Value {
    json!({
        "protocol": 1,
        "id": ID,
        "version": "1.0.0",
        "requires": { "keel": ">=0.4", "modules": { "probe": ">=0.1" } },
        "run": { "mode": mode, "cmd": ["./bin/acme"] },
        "level": "user",
        "rights": { "hook": ["probe.item.delete"] },
        "functions": [
            {
                "name": "acme.sync.list",
                "kind": "list",
                "access": { "level": "user" },
                "key": "id",
                "params": { "$ref": "keel://ListQuery" }
            },
            action("acme.sync.peek"),
            action("acme.sync.slow"),
            action("acme.sync.broken"),
            action("acme.sync.refuse"),
            action("acme.mirror.poll")
        ],
        "problems": [{ "source": "acme.mirror", "func": "acme.mirror.poll" }]
    })
}

fn steady() -> Value {
    json!({ "timeout_ms": 10_000, "failures": 5, "quiet_ms": 600_000, "liveness_ms": 600_000, "backoff_ms": 50 })
}

async fn serving(site: &Site, config: &Value) -> Harness {
    let harness = enabled_with(site, config, &[&PROBE]).await;
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        let reply = harness.call(Who::A1, "acme.sync.list", json!({})).await;
        if reply.data().is_some() {
            return harness;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the plugin never served its list: {:?}",
            reply.error()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn sent(site: &Site) -> Vec<Value> {
    std::fs::read_to_string(site.path("marks/frames"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).expect("every frame of the kernel is JSON"))
        .collect()
}

fn error(reply: &Reply) -> &keel_sdk::types::CallError {
    reply
        .error()
        .unwrap_or_else(|| panic!("an error was expected, got {:?}", reply.data()))
}

async fn peek(harness: &Harness, principal: &Principal, target: &str) -> Value {
    let reply = harness
        .call_as_principal(principal, "acme.sync.peek", json!({ "target": target }))
        .await;
    data(&reply).clone()
}

#[tokio::test]
async fn a_plugin_function_answers_through_a_call_frame() {
    let site = Site::with(PLUGIN, &manifest("daemon"));
    let harness = serving(&site, &steady()).await;
    let reply = harness
        .call(
            Who::A1,
            "acme.sync.list",
            json!({ "page": 2, "pageSize": 10 }),
        )
        .await;
    assert_eq!(
        data(&reply),
        &json!({ "data": [{ "id": "one", "name": "first" }], "total": 1 })
    );
    let frame = sent(&site)
        .pop()
        .expect("the call frame reached the plugin");
    let caller = harness.principal(Who::A1);
    assert_eq!(frame["t"], json!("call"));
    assert_eq!(frame["func"], json!("acme.sync.list"));
    assert!(
        frame["id"].as_str().is_some_and(|id| id.starts_with("k-")),
        "{frame}"
    );
    assert_eq!(frame["principal"]["login"], json!(caller.login));
    assert_eq!(frame["principal"]["id"], json!(caller.id));
    assert_eq!(frame["principal"]["accountId"], json!(caller.account_id));
    assert_eq!(frame["params"]["page"], json!(2));
    assert_eq!(frame["params"]["pageSize"], json!(10));
    harness.finish().await;
}

#[tokio::test]
async fn a_before_hook_vetoes_a_delete_with_a_field_error() {
    let site = Site::with(PLUGIN, &manifest("daemon"));
    let harness = serving(&site, &steady()).await;
    let vetoed = harness
        .call(Who::A1, "probe.item.delete", json!({ "ids": ["linked"] }))
        .await;
    let refused = error(&vetoed);
    assert_eq!(refused.code, ErrorCode::Validation);
    assert_eq!(vetoed.status(), 422);
    assert_eq!(refused.fields.len(), 1, "{refused:?}");
    assert_eq!(refused.fields[0].path, "ids");
    assert_eq!(refused.fields[0].key.as_deref(), Some("acme.error.linked"));
    assert_eq!(refused.fields[0].params["name"], json!("linked"));
    let frame = sent(&site)
        .pop()
        .expect("the before frame reached the plugin");
    assert_eq!(frame["t"], json!("before"));
    assert_eq!(frame["func"], json!("probe.item.delete"));
    assert_eq!(
        frame["principal"]["login"],
        json!(harness.principal(Who::A1).login)
    );
    assert_eq!(frame["params"], json!({ "ids": ["linked"] }));

    let passed = harness
        .call(Who::A1, "probe.item.delete", json!({ "ids": ["free"] }))
        .await;
    assert_eq!(data(&passed)["deleted"], json!(["free"]));
    assert_eq!(data(&passed)["checked"], json!(true));
    harness.finish().await;
}

#[tokio::test]
async fn invoke_runs_under_the_callers_principal_within_the_plugin_scope() {
    let site = Site::with(PLUGIN, &manifest("daemon"));
    let harness = serving(&site, &steady()).await;
    let alice = harness.principal(Who::A1).clone();
    let seen = peek(&harness, &alice, "probe.who.get").await;
    assert_eq!(seen["ok"], json!(true), "{seen}");
    assert_eq!(seen["data"]["login"], json!(alice.login));
    assert_eq!(seen["data"]["level"], json!("user"));
    assert_eq!(seen["data"]["account"], json!(alice.account_id));
    assert!(
        seen["id"].as_str().is_some_and(|id| id.starts_with("k-")),
        "{seen}"
    );

    let admin = harness.principal(Who::Admin).clone();
    let direct = harness
        .call_as_principal(&admin, "probe.secret.get", json!({}))
        .await;
    assert_eq!(data(&direct)["secret"], json!(true));
    let administered = peek(&harness, &admin, "probe.who.get").await;
    assert_eq!(administered["data"]["login"], json!(admin.login));
    for target in ["probe.secret.get", "auth.session.list", "probe.nowhere.get"] {
        let refused = peek(&harness, &admin, target).await;
        assert_eq!(refused["ok"], json!(false), "{refused}");
        assert_eq!(refused["error"]["code"], json!("forbidden"), "{refused}");
        assert_eq!(refused["error"]["key"], json!(ERROR_INVOKE), "{refused}");
        assert_eq!(refused["error"]["params"]["function"], json!(target));
    }

    let mut scoped = alice.clone();
    scoped.scope = Scope::Functions(vec!["acme.*".to_owned()]);
    let narrowed = peek(&harness, &scoped, "probe.who.get").await;
    assert_eq!(narrowed["error"]["key"], json!(ERROR_INVOKE), "{narrowed}");
    harness.finish().await;
}

#[tokio::test]
async fn a_call_frame_without_an_answer_times_out_and_counts_toward_the_breaker() {
    let site = Site::with(PLUGIN, &manifest("daemon"));
    let config = json!({ "timeout_ms": 1_000, "failures": 2, "quiet_ms": 600_000, "liveness_ms": 600_000, "backoff_ms": 50 });
    let harness = serving(&site, &config).await;
    let mut answers = Vec::new();
    while answers.last().map(String::as_str) != Some(ERROR_FAILING) {
        assert!(answers.len() < 2, "{answers:?}");
        let reply = harness.call(Who::A1, "acme.sync.slow", json!({})).await;
        assert_eq!(error(&reply).code, ErrorCode::Unavailable);
        answers.push(error(&reply).key.clone());
    }
    assert!(
        answers[..answers.len() - 1]
            .iter()
            .all(|key| key == ERROR_UNAVAILABLE),
        "{answers:?}"
    );
    assert_eq!(
        raised(&harness).await,
        format!("{ID} warning a frame got no answer within 1000 ms")
    );
    let list = harness.call(Who::A1, "acme.sync.list", json!({})).await;
    assert_eq!(error(&list).key, ERROR_FAILING);
    let problems = harness
        .database()
        .rows(&format!(
            "SELECT key FROM notify_problem WHERE key = '{PROBLEM_FAILING}' AND cleared_at IS NULL"
        ))
        .await;
    assert_eq!(problems, [PROBLEM_FAILING]);
    harness.finish().await;
}

#[tokio::test]
async fn an_answer_outside_the_plugin_prefix_is_refused_and_an_allowed_refusal_keeps_its_code() {
    let site = Site::with(PLUGIN, &manifest("daemon"));
    let harness = serving(&site, &steady()).await;
    let broken = harness.call(Who::A1, "acme.sync.broken", json!({})).await;
    assert_eq!(error(&broken).code, ErrorCode::Internal);
    assert_eq!(error(&broken).key, ERROR_ANSWER);
    assert_eq!(error(&broken).params["plugin"], json!(ID));
    let refused = harness.call(Who::A1, "acme.sync.refuse", json!({})).await;
    assert_eq!(error(&refused).code, ErrorCode::NotFound);
    assert_eq!(error(&refused).key, "acme.error.gone");
    assert_eq!(error(&refused).params["name"], json!("a.org"));
    assert!(failing(&harness).await.is_empty());
    harness.finish().await;
}

#[tokio::test]
async fn a_frame_the_kernel_does_not_take_is_answered_with_the_frame_error() {
    let site = Site::with(PLUGIN, &manifest("daemon"));
    site.mark("stray");
    let harness = serving(&site, &steady()).await;
    eventually("the plugin got both answers", || {
        site.plugin_log()
            .iter()
            .filter(|line| line.starts_with("stray "))
            .count()
            == 2
    })
    .await;
    let answers: Vec<Value> = site
        .plugin_log()
        .iter()
        .filter_map(|line| line.strip_prefix("stray "))
        .map(|answer| serde_json::from_str(answer).expect("the answer is JSON"))
        .collect();
    assert_eq!(answers.len(), 2, "{:?}", site.plugin_log());
    for (answer, id) in answers.iter().zip(["p-1", "p-2"]) {
        assert_eq!(answer["id"], json!(id), "{answer}");
        assert_eq!(answer["ok"], json!(false), "{answer}");
        assert_eq!(answer["error"]["key"], json!(ERROR_FRAME), "{answer}");
    }
    assert!(failing(&harness).await.is_empty());
    harness.finish().await;
}

#[tokio::test]
async fn a_call_frame_over_the_limit_never_reaches_the_plugin() {
    let site = Site::with(PLUGIN, &manifest("daemon"));
    let harness = serving(&site, &steady()).await;
    let before = sent(&site).len();
    let huge = "x".repeat(1024 * 1024);
    let reply = harness
        .call(Who::A1, "acme.sync.peek", json!({ "target": huge }))
        .await;
    assert_eq!(error(&reply).code, ErrorCode::Validation);
    assert_eq!(error(&reply).key, ERROR_TOO_LARGE);
    assert_eq!(sent(&site).len(), before);
    let list = harness.call(Who::A1, "acme.sync.list", json!({})).await;
    assert!(list.data().is_some(), "{:?}", list.error());
    harness.finish().await;
}

#[tokio::test]
async fn an_exec_plugin_answers_each_call_frame_in_a_process_of_its_own() {
    let site = Site::with(PLUGIN, &manifest("exec"));
    let harness = serving(&site, &steady()).await;
    eventually(
        "the liveness process and the first call each ran in a process",
        || site.starts() == 2,
    )
    .await;
    let alice = harness.principal(Who::A1).clone();
    let seen = peek(&harness, &alice, "probe.who.get").await;
    assert_eq!(seen["data"]["login"], json!(alice.login));
    let vetoed = harness
        .call(Who::A1, "probe.item.delete", json!({ "ids": ["linked"] }))
        .await;
    assert_eq!(
        error(&vetoed).fields[0].key.as_deref(),
        Some("acme.error.linked")
    );
    assert_eq!(site.starts(), 4, "{:?}", site.plugin_log());
    assert!(
        site.plugin_log()
            .iter()
            .filter(|line| line.starts_with("start "))
            .all(|line| *line == format!("start {ACCOUNT} ")),
        "{:?}",
        site.plugin_log()
    );
    harness.finish().await;
}

#[tokio::test]
async fn a_problem_source_of_the_plugin_raises_what_its_function_answers() {
    let site = Site::with(PLUGIN, &manifest("daemon"));
    let harness = serving(&site, &steady()).await;
    poll(&harness).await;
    eventually("the poll reaches the plugin", || {
        sent(&site)
            .iter()
            .any(|frame| frame["func"] == json!("acme.mirror.poll"))
    })
    .await;
    let frame = sent(&site)
        .into_iter()
        .find(|frame| frame["func"] == json!("acme.mirror.poll"))
        .expect("the poll frame");
    assert_eq!(frame["principal"]["login"], json!(format!("plugin-{ID}")));
    let problems = harness
        .database()
        .rows("SELECT key || ' ' || subject || ' ' || source FROM notify_problem WHERE cleared_at IS NULL ORDER BY key")
        .await;
    assert_eq!(problems, ["acme.mirror.lagging mirror acme.mirror"]);
    harness.finish().await;
}
