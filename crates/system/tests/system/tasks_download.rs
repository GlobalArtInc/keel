use std::io::Write;
use std::time::Duration;

use keel_sdk::Registrar;
use keel_sdk::prelude::*;
use keel_sdk::types::TaskState;
use keel_system::tasks::api::Drain;
use keel_system::tasks::{DOWNLOAD_FUNCTION, ERROR_NO_FILE, MODULE};
use keel_testkit::{Answer, Harness, TestHttp, Who, tokio};

const ROUTE: &str = "/api/v1/x/task/download";
const EXPORT_ROUTE: &str = "/api/v1/x/keel/export";
const BIG_TOTAL: u64 = 20_000;
const EN: &str = r#"{ "dumper": {
    "dump": { "title": "Dump lines" }, "lines": "Lines", "count": { "title": "Count" },
    "big": { "list": { "title": "Big" }, "column": { "name": "Name", "index": "Index" } }
} }"#;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Dump {
    lines: u64,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Dumped {
    lines: u64,
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct BigRow {
    name: String,
    index: u64,
}

struct Big;

#[async_trait]
impl ListSource for Big {
    type Row = BigRow;

    async fn fetch(&self, call: &Call<ListQuery>, _: &ListSpec) -> Result<Page<BigRow>> {
        let query = call.params();
        let start = u64::from(query.page - 1) * u64::from(query.page_size);
        let end = (start + u64::from(query.page_size)).min(BIG_TOTAL);
        let data = (start..end)
            .map(|index| BigRow {
                name: format!("row {}", index + 1),
                index,
            })
            .collect();
        Ok(Page::new(data, BIG_TOTAL))
    }
}

static DUMPER: ModuleDef = ModuleDef {
    id: "dumper",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &["task"],
    migrations: None,
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.function(
        function::<Dump, Dumped>("dumper.dump")
            .kind(Kind::Action)
            .mode(Mode::Read)
            .level(Level::User)
            .unowned()
            .ui(ActionScreen::new("dumper.dump.title").result(ScreenResult::Task))
            .handler(|call: Call<Dump>| async move {
                let task = call
                    .task()
                    .ok_or_else(|| KeelError::internal("dumper.error.inline"))?;
                let lines = call.params().lines;
                let mut output = task.output()?;
                for line in 1..=lines {
                    writeln!(output, "line {line} of {}", call.principal().login).map_err(
                        |error| KeelError::internal("dumper.error.output").source(error),
                    )?;
                    task.progress(line, Some(lines), None).await?;
                }
                output
                    .flush()
                    .map_err(|error| KeelError::internal("dumper.error.output").source(error))?;
                task.stored(DownloadFile::new("dump.txt", "text/plain; charset=utf-8"));
                Ok(Dumped { lines })
            }),
    )
    .function(
        function::<Dump, Dumped>("dumper.count")
            .kind(Kind::Action)
            .mode(Mode::Read)
            .level(Level::User)
            .unowned()
            .ui(ActionScreen::new("dumper.count.title").result(ScreenResult::Task))
            .handler(|call: Call<Dump>| async move {
                let task = call
                    .task()
                    .ok_or_else(|| KeelError::internal("dumper.error.inline"))?;
                task.progress(call.params().lines, Some(call.params().lines), None)
                    .await?;
                Ok(Dumped {
                    lines: call.params().lines,
                })
            }),
    )
    .function(
        function::<ListQuery, Page<BigRow>>("dumper.big.list")
            .kind(Kind::List)
            .level(Level::User)
            .unowned()
            .ui(List::new("dumper.big.list.title")
                .column(Column::new("name", ColumnType::Text))
                .column(Column::new("index", ColumnType::Number)))
            .max_page_size(2500)
            .source(Big),
    )
    .i18n("en", EN);
    Ok(())
}

pub async fn boot() -> Harness {
    Harness::new().module(&MODULE).module(&DUMPER).boot().await
}

pub fn task_of(answer: &Answer) -> Id {
    assert_eq!(answer.status, 200, "{}", answer.body);
    let envelope = answer.json();
    assert_eq!(envelope["ok"], json!(true), "{envelope}");
    let data = &envelope["data"];
    assert_eq!(
        data.as_object().map(|object| object.len()),
        Some(1),
        "a task ref carries the job alone: {data}"
    );
    data["task"]["id"]
        .as_str()
        .expect("the task id")
        .parse()
        .expect("a ulid")
}

pub async fn wait_final(harness: &Harness, id: Id) -> TaskState {
    let drain = harness
        .registry()
        .services()
        .get::<Drain>()
        .expect("the tasks module provides the drain");
    tokio::time::timeout(Duration::from_secs(30), drain.wait(id))
        .await
        .expect("the job ends in time")
        .expect("the state is read")
}

pub async fn detail(harness: &Harness, who: Who, id: Id) -> Value {
    let got = harness.call(who, "task.get", json!({ "id": id })).await;
    got.data()
        .unwrap_or_else(|| panic!("task.get failed: {:?}", got.error()))
        .clone()
}

pub fn assert_download_ref(result: &Value, id: Id) {
    assert_eq!(
        result["download"],
        json!({ "function": DOWNLOAD_FUNCTION, "params": { "id": id.to_string() } }),
        "{result}"
    );
}

pub async fn fetch(http: &TestHttp, who: Who, id: Id) -> Answer {
    http.get(who, &format!("{ROUTE}?id={id}")).await
}

#[tokio::test]
async fn a_jobs_stored_file_ends_in_a_download_ref_that_task_download_serves() {
    let harness = boot().await;
    let http = harness.http().await;
    let id = task_of(
        &http
            .post(Who::A1, "/api/v1/call/dumper.dump", &json!({ "lines": 3 }))
            .await,
    );
    assert_eq!(wait_final(&harness, id).await, TaskState::Done);

    let detail = detail(&harness, Who::A1, id).await;
    assert_eq!(detail["state"], json!("done"));
    assert_eq!(detail["progress"], json!("3/3"));
    let result = &detail["result"];
    assert_eq!(result["lines"], json!(3));
    assert_download_ref(result, id);
    assert_eq!(result.as_object().map(|object| object.len()), Some(2));
    let stored = harness
        .database()
        .rows(&format!(
            "SELECT file_name || ' ' || file_type FROM task_job WHERE id = '{id}'"
        ))
        .await;
    assert_eq!(stored, ["dump.txt text/plain; charset=utf-8"]);

    let got = fetch(&http, Who::A1, id).await;
    assert_eq!(got.status, 200, "{}", got.body);
    assert_eq!(
        got.header("content-type"),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(
        got.header("content-disposition"),
        Some("attachment; filename=\"dump.txt\"")
    );
    assert_eq!(got.header("cache-control"), Some("no-store"));
    assert!(got.header("x-keel-request-id").is_some());
    assert_eq!(got.body, "line 1 of a1\nline 2 of a1\nline 3 of a1\n");

    let posted = http.post(Who::A1, ROUTE, &json!({ "id": id })).await;
    assert_eq!(posted.status, 200, "{}", posted.body);
    assert_eq!(posted.body, got.body);

    let admin = fetch(&http, Who::Admin, id).await;
    assert_eq!(admin.status, 200, "{}", admin.body);
    assert_eq!(admin.body, got.body);

    let on_call_route = http
        .post(Who::A1, "/api/v1/call/task.download", &json!({ "id": id }))
        .await;
    assert_eq!(on_call_route.status, 422, "{}", on_call_route.body);
    assert_eq!(
        on_call_route.json()["error"]["params"]["route"],
        json!(ROUTE)
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn task_download_of_another_accounts_job_or_of_a_job_without_a_file_is_not_found() {
    let harness = boot().await;
    let http = harness.http().await;
    let with_file = task_of(
        &http
            .post(Who::A1, "/api/v1/call/dumper.dump", &json!({ "lines": 1 }))
            .await,
    );
    let without_file = task_of(
        &http
            .post(Who::A1, "/api/v1/call/dumper.count", &json!({ "lines": 2 }))
            .await,
    );
    assert_eq!(wait_final(&harness, with_file).await, TaskState::Done);
    assert_eq!(wait_final(&harness, without_file).await, TaskState::Done);

    let foreign = fetch(&http, Who::A2, with_file).await;
    assert_eq!(foreign.status, 404, "{}", foreign.body);
    assert_eq!(foreign.json()["error"]["code"], json!("not_found"));

    let unrelated = fetch(&http, Who::Unrelated, with_file).await;
    assert_eq!(unrelated.status, 404, "{}", unrelated.body);

    let reseller = fetch(&http, Who::Reseller, with_file).await;
    assert_eq!(reseller.status, 200, "{}", reseller.body);
    assert_eq!(reseller.body, "line 1 of a1\n");

    let empty = fetch(&http, Who::A1, without_file).await;
    assert_eq!(empty.status, 404, "{}", empty.body);
    assert_eq!(empty.json()["error"]["key"], json!(ERROR_NO_FILE));
    let detail = detail(&harness, Who::A1, without_file).await;
    assert_eq!(detail["result"], json!({ "lines": 2 }));

    let unknown = fetch(&http, Who::A1, Id::generate()).await;
    assert_eq!(unknown.status, 404, "{}", unknown.body);
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn an_export_job_above_the_threshold_ends_in_a_download_ref_that_task_download_serves() {
    let harness = boot().await;
    let http = harness.http().await;
    let answer = http
        .post(
            Who::A1,
            EXPORT_ROUTE,
            &json!({ "func": "dumper.big.list", "format": "csv" }),
        )
        .await;
    assert!(
        answer
            .header("content-type")
            .is_some_and(|value| value.starts_with("application/json")),
        "{:?}",
        answer.headers
    );
    let id = task_of(&answer);
    assert_eq!(wait_final(&harness, id).await, TaskState::Done);

    let detail = detail(&harness, Who::A1, id).await;
    assert_eq!(detail["function"], json!("keel.export"));
    assert_eq!(detail["class"], json!("export"));
    assert_eq!(
        detail["progress"],
        json!(format!("{BIG_TOTAL}/{BIG_TOTAL}"))
    );
    assert_eq!(detail["params"]["func"], json!("dumper.big.list"));
    let result = &detail["result"];
    let filename = format!("dumper.big.list-{}.csv", harness.clock().now().date_naive());
    assert_eq!(result["filename"], json!(filename));
    assert_eq!(result["contentType"], json!("text/csv; charset=utf-8"));
    assert_download_ref(result, id);

    let got = fetch(&http, Who::A1, id).await;
    assert_eq!(got.status, 200, "{}", got.body);
    assert_eq!(got.header("content-type"), Some("text/csv; charset=utf-8"));
    assert_eq!(
        got.header("content-disposition"),
        Some(format!("attachment; filename=\"{filename}\"").as_str())
    );
    let lines: Vec<&str> = got
        .body
        .split("\r\n")
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(lines.len(), BIG_TOTAL as usize + 1);
    assert_eq!(lines[0], "Name,Index");
    assert_eq!(lines[1], "row 1,0");
    assert_eq!(
        lines[BIG_TOTAL as usize],
        format!("row {BIG_TOTAL},{}", BIG_TOTAL - 1)
    );
    let steps = harness
        .database()
        .rows(&format!(
            "SELECT count(*)::text FROM task_step WHERE job_id = '{id}'"
        ))
        .await;
    assert_eq!(steps, ["8"]);

    let foreign = fetch(&http, Who::A2, id).await;
    assert_eq!(foreign.status, 404, "{}", foreign.body);
    http.stop().await.expect("the server stops");
    harness.finish().await;
}
