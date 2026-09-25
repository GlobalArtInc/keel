use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use keel_kernel::export::{COLUMNS_KEY, CSV_TYPE, EXPORT_TASK_ROWS, FUNCTION, JSON_TYPE};
use keel_sdk::Registrar;
use keel_sdk::prelude::*;
use keel_sdk::runtime::TASKS_UNAVAILABLE_KEY;
use keel_sdk::system::{Caller, CallerOptions, TaskRunner, TaskSink, TaskSpec};
use keel_sdk::ui::{Band, Report};
use keel_sdk::{TaskHandle, types::TaskState};
use keel_testkit::{Harness, Who};
use keel_types::{ErrorCode, Scope};

const ROUTE: &str = "/api/v1/x/keel/export";
const BIG_TOTAL: u64 = 10_001;
const EN: &str = r#"{ "stock": {
    "item": { "list": { "title": "Items" }, "column": { "name": "Name", "qty": "Quantity", "accountId": "Account" } },
    "secret": { "list": { "title": "Secrets" }, "column": { "name": "Name" } },
    "big": { "list": { "title": "Big" }, "column": { "name": "Name" } },
    "count": { "title": "Count" },
    "report": { "levels": { "title": "Levels" }, "band": { "items": "Items" }, "min": "At least",
                "column": { "name": "Name", "qty": "Quantity", "accountId": "Account" } }
} }"#;
const RU: &str = r#"{ "stock": { "item": { "column": { "qty": "Количество" } } } }"#;

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Item {
    name: String,
    qty: u64,
    #[schemars(extend("x-keel" = json!({ "level": "reseller" })))]
    account_id: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Nothing {}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Counted {
    counted: u64,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Floor {
    #[serde(default)]
    min: u64,
}

#[derive(Serialize, JsonSchema)]
struct Levels {
    bands: LevelBands,
}

#[derive(Serialize, JsonSchema)]
struct LevelBands {
    items: Vec<Item>,
}

type Stocked = BTreeMap<Id, Vec<(String, u64)>>;

static ROWS: LazyLock<Mutex<Stocked>> = LazyLock::new(Mutex::default);
static QUERIES: LazyLock<Mutex<Vec<(Id, ListQuery)>>> = LazyLock::new(Mutex::default);

fn stock(account: Id, rows: &[(&str, u64)]) {
    ROWS.lock().unwrap_or_else(PoisonError::into_inner).insert(
        account,
        rows.iter()
            .map(|(name, qty)| ((*name).to_owned(), *qty))
            .collect(),
    );
}

fn queries_of(principal: &Principal) -> Vec<ListQuery> {
    QUERIES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter(|(who, _)| *who == principal.id)
        .map(|(_, query)| query.clone())
        .collect()
}

struct Stock;

#[async_trait]
impl ListSource for Stock {
    type Row = Item;

    async fn fetch(&self, call: &Call<ListQuery>, _: &ListSpec) -> Result<Page<Item>> {
        let query = call.params();
        QUERIES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((call.principal().id, query.clone()));
        let mut rows = visible(call).await?;
        if !query.search.is_empty() {
            rows.retain(|row| row.name.contains(&query.search));
        }
        Ok(slice(rows, query))
    }
}

async fn visible<P>(call: &Call<P>) -> Result<Vec<Item>> {
    let stocked: Vec<(Id, Vec<(String, u64)>)> = ROWS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .map(|(account, rows)| (*account, rows.clone()))
        .collect();
    let mut rows = Vec::new();
    for (account, stocked) in stocked {
        if !call.account_scope().includes(call.db(), account).await? {
            continue;
        }
        rows.extend(stocked.into_iter().map(|(name, qty)| Item {
            name,
            qty,
            account_id: account.to_string(),
        }));
    }
    rows.sort_by(|a, b| a.account_id.cmp(&b.account_id).then(a.name.cmp(&b.name)));
    Ok(rows)
}

fn levels() -> FunctionDef {
    function::<Floor, Levels>("stock.report.levels")
        .kind(Kind::Report)
        .level(Level::User)
        .unowned()
        .ui(Report::new("stock.report.levels.title").band(
            Band::new("items")
                .column(Column::new("name", ColumnType::Text))
                .column(Column::new("qty", ColumnType::Number).total(Total::Sum))
                .column(Column::new("accountId", ColumnType::Text).level(Level::Reseller))
                .formats([ExportFormat::Csv, ExportFormat::Json]),
        ))
        .handler(|call: Call<Floor>| async move {
            let min = call.params().min;
            let mut items = visible(&call).await?;
            items.retain(|item| item.qty >= min);
            Ok(Levels {
                bands: LevelBands { items },
            })
        })
}

struct Big;

#[async_trait]
impl ListSource for Big {
    type Row = Item;

    async fn fetch(&self, call: &Call<ListQuery>, _: &ListSpec) -> Result<Page<Item>> {
        let query = call.params();
        QUERIES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((call.principal().id, query.clone()));
        let total: u64 = query.search.parse().unwrap_or(BIG_TOTAL);
        let start = u64::from(query.page - 1) * u64::from(query.page_size);
        let end = (start + u64::from(query.page_size)).min(total);
        let account = call.principal().account_id.to_string();
        let data = (start..end)
            .map(|index| Item {
                name: format!("row-{}", index + 1),
                qty: index,
                account_id: account.clone(),
            })
            .collect();
        Ok(Page::new(data, total))
    }
}

fn slice(rows: Vec<Item>, query: &ListQuery) -> Page<Item> {
    let total = rows.len() as u64;
    let skip = (query.page as usize - 1) * query.page_size as usize;
    let data = rows
        .into_iter()
        .skip(skip)
        .take(query.page_size as usize)
        .collect();
    Page::new(data, total)
}

#[derive(Default)]
struct RunnerDouble {
    spawned: Mutex<Vec<(Id, TaskSpec)>>,
}

impl RunnerDouble {
    fn spawned_for(&self, principal: &Principal) -> Vec<(Id, TaskSpec)> {
        self.spawned
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(_, spec)| spec.principal.id == principal.id)
            .cloned()
            .collect()
    }
}

#[async_trait]
impl TaskRunner for RunnerDouble {
    async fn spawn(&self, spec: TaskSpec) -> Result<Id> {
        let id = Id::generate();
        self.spawned
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((id, spec));
        Ok(id)
    }

    async fn progress(&self, _: Id, _: u64, _: Option<u64>, _: Option<&str>) -> Result<()> {
        Ok(())
    }

    async fn log(&self, _: Id, _: &str) -> Result<()> {
        Ok(())
    }

    async fn checkpoint(&self, _: Id, _: Value) -> Result<()> {
        Ok(())
    }

    async fn cancel(&self, _: Id) -> Result<()> {
        Ok(())
    }

    async fn state(&self, _: Id) -> Result<TaskState> {
        Ok(TaskState::Queued)
    }

    async fn quiesce(&self, _: Id) -> Result<Vec<Id>> {
        Ok(Vec::new())
    }

    fn resume(&self) -> Result<()> {
        Ok(())
    }
}

struct SinkDouble {
    id: Id,
    dir: PathBuf,
    progress: Mutex<Vec<(u64, Option<u64>)>>,
    file: Mutex<Option<DownloadFile>>,
}

#[async_trait]
impl TaskSink for SinkDouble {
    fn id(&self) -> Id {
        self.id
    }

    async fn progress(&self, done: u64, total: Option<u64>, _: Option<&str>) -> Result<()> {
        self.progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((done, total));
        Ok(())
    }

    async fn log(&self, _: &str) -> Result<()> {
        Ok(())
    }

    async fn checkpoint(&self, _: Value) -> Result<()> {
        Ok(())
    }

    fn resume_from(&self) -> Option<Value> {
        None
    }

    fn is_cancelled(&self) -> bool {
        false
    }

    fn output_path(&self) -> PathBuf {
        self.dir.join(format!("{}.out", self.id))
    }

    fn stored(&self, file: DownloadFile) {
        *self.file.lock().unwrap_or_else(PoisonError::into_inner) = Some(file);
    }
}

static RUNNER: LazyLock<Arc<RunnerDouble>> = LazyLock::new(Arc::default);

static STOCK: ModuleDef = ModuleDef {
    id: "stock",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init,
};

static STOCK_WITH_RUNNER: ModuleDef = ModuleDef {
    id: "stock",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: init_with_runner,
};

fn items() -> FunctionDef {
    function::<ListQuery, Page<Item>>("stock.item.list")
        .kind(Kind::List)
        .access(Level::User, "stock.item.read")
        .unowned()
        .ui(List::new("stock.item.list.title")
            .search(["name"])
            .column(Column::new("name", ColumnType::Text).sortable())
            .column(Column::new("qty", ColumnType::Number))
            .column(Column::new("accountId", ColumnType::Text).level(Level::Reseller)))
        .max_page_size(2)
        .source(Stock)
}

fn init(r: &mut Registrar) -> Result<()> {
    r.function(items())
        .function(levels())
        .function(
            function::<ListQuery, Page<Item>>("stock.secret.list")
                .kind(Kind::List)
                .level(Level::Admin)
                .unowned()
                .ui(List::new("stock.secret.list.title")
                    .column(Column::new("name", ColumnType::Text)))
                .source(Stock),
        )
        .function(
            function::<ListQuery, Page<Item>>("stock.big.list")
                .kind(Kind::List)
                .level(Level::User)
                .unowned()
                .ui(List::new("stock.big.list.title").column(Column::new("name", ColumnType::Text)))
                .max_page_size(4000)
                .source(Big),
        )
        .function(
            function::<Nothing, Counted>("stock.count")
                .kind(Kind::Action)
                .mode(Mode::Read)
                .level(Level::User)
                .unowned()
                .handler(|_: Call<Nothing>| async { Ok(Counted { counted: 0 }) }),
        )
        .permission("stock.item.read", &[Level::User, Level::Reseller])
        .i18n("en", EN)
        .i18n("ru", RU);
    Ok(())
}

fn init_with_runner(r: &mut Registrar) -> Result<()> {
    init(r)?;
    r.provide::<dyn TaskRunner>(RUNNER.clone());
    Ok(())
}

fn csv_lines(body: &str) -> Vec<&str> {
    assert!(body.ends_with("\r\n"), "{body:?}");
    body.split("\r\n").filter(|line| !line.is_empty()).collect()
}

fn today(harness: &Harness) -> String {
    harness.clock().now().date_naive().to_string()
}

#[keel_testkit::tokio::test]
async fn an_export_streams_every_page_under_the_screens_scope_and_strip_with_translated_headers() {
    let harness = Harness::new().module(&STOCK).boot().await;
    let accounts = harness.fixtures().accounts.clone();
    stock(
        accounts.a1,
        &[("apple, red", 3), ("berry", 5), ("cherry", 7)],
    );
    stock(accounts.a2, &[("date", 1), ("elder \"berry\"", 2)]);
    stock(accounts.unrelated, &[("fig", 9)]);
    let http = harness.http().await;

    let csv = http
        .get(Who::A1, &format!("{ROUTE}?func=stock.item.list&format=csv"))
        .await;
    assert_eq!(csv.status, 200, "{}", csv.body);
    assert_eq!(csv.header("content-type"), Some(CSV_TYPE));
    assert_eq!(
        csv.header("content-disposition"),
        Some(
            format!(
                "attachment; filename=\"stock.item.list-{}.csv\"",
                today(&harness)
            )
            .as_str()
        )
    );
    assert_eq!(csv.header("cache-control"), Some("no-store"));
    assert_eq!(
        csv_lines(&csv.body),
        ["Name,Quantity", "\"apple, red\",3", "berry,5", "cherry,7"]
    );
    let pages = queries_of(harness.principal(Who::A1));
    assert_eq!(pages.len(), 2, "{pages:?}");
    assert!(pages.iter().all(|query| query.page_size == 2), "{pages:?}");
    assert_eq!(pages[0].page, 1);
    assert_eq!(pages[1].page, 2);

    let reseller = http
        .get(
            Who::Reseller,
            &format!(
                "{ROUTE}?func=stock.item.list&format=csv&query={}",
                "%7B%22sort%22%3A%7B%22key%22%3A%22name%22%2C%22dir%22%3A%22asc%22%7D%7D"
            ),
        )
        .await;
    assert_eq!(reseller.status, 200, "{}", reseller.body);
    let lines = csv_lines(&reseller.body);
    assert_eq!(lines[0], "Name,Quantity,Account");
    assert_eq!(lines.len(), 6, "{lines:?}");
    assert!(
        lines
            .iter()
            .any(|line| *line == format!("berry,5,{}", accounts.a1))
    );
    assert!(
        lines
            .iter()
            .any(|line| *line == format!("\"elder \"\"berry\"\"\",2,{}", accounts.a2))
    );
    assert!(
        !lines.iter().any(|line| line.starts_with("fig")),
        "{lines:?}"
    );
    let pages = queries_of(harness.principal(Who::Reseller));
    assert_eq!(pages.len(), 3, "{pages:?}");
    assert!(
        pages
            .iter()
            .all(|query| query.sort.as_ref().is_some_and(|sort| sort.key == "name"))
    );

    let forbidden = http
        .post(
            Who::A1,
            ROUTE,
            &json!({ "func": "stock.item.list", "columns": ["accountId"] }),
        )
        .await;
    assert_eq!(forbidden.status, 422, "{}", forbidden.body);
    let error = &forbidden.json()["error"];
    assert_eq!(error["code"], json!("validation"));
    assert_eq!(error["fields"][0]["path"], json!("columns"));
    assert_eq!(error["fields"][0]["key"], json!(COLUMNS_KEY));
    assert_eq!(error["fields"][0]["params"]["column"], json!("accountId"));

    let picked = http
        .post(
            Who::Reseller,
            ROUTE,
            &json!({ "func": "stock.item.list", "columns": ["accountId", "name"], "query": { "search": "berry" } }),
        )
        .await;
    assert_eq!(picked.status, 200, "{}", picked.body);
    assert_eq!(
        csv_lines(&picked.body),
        [
            "Account,Name".to_owned(),
            format!("{},berry", accounts.a1),
            format!("{},\"elder \"\"berry\"\"\"", accounts.a2)
        ]
    );

    let json = http
        .post(
            Who::A1,
            ROUTE,
            &json!({ "func": "stock.item.list", "format": "json" }),
        )
        .await;
    assert_eq!(json.status, 200, "{}", json.body);
    assert_eq!(json.header("content-type"), Some(JSON_TYPE));
    assert_eq!(
        json.header("content-disposition"),
        Some(
            format!(
                "attachment; filename=\"stock.item.list-{}.json\"",
                today(&harness)
            )
            .as_str()
        )
    );
    let rows: Value = serde_json::from_str(&json.body).expect("one JSON array");
    assert_eq!(
        rows,
        json!([
            { "name": "apple, red", "qty": 3 },
            { "name": "berry", "qty": 5 },
            { "name": "cherry", "qty": 7 }
        ])
    );

    let mut russian = harness.principal(Who::A1).clone();
    russian.lang = "ru".to_owned();
    let reply = harness
        .call_as_principal(&russian, FUNCTION, json!({ "func": "stock.item.list" }))
        .await;
    let download = reply.download.expect("the body");
    let body = String::from_utf8(download.collect().await.expect("bytes")).expect("utf-8");
    assert_eq!(csv_lines(&body)[0], "Name,Количество");
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[keel_testkit::tokio::test]
async fn an_export_above_the_row_threshold_answers_a_task_ref_and_writes_the_file_inside_the_job() {
    let harness = Harness::new().module(&STOCK_WITH_RUNNER).boot().await;
    let caller = harness.principal(Who::A1).clone();
    let http = harness.http().await;

    let at_threshold = http
        .get(
            Who::A1,
            &format!(
                "{ROUTE}?func=stock.big.list&query=%7B%22search%22%3A%22{EXPORT_TASK_ROWS}%22%7D"
            ),
        )
        .await;
    assert_eq!(at_threshold.status, 200, "{}", at_threshold.body);
    assert_eq!(
        csv_lines(&at_threshold.body).len(),
        EXPORT_TASK_ROWS as usize + 1
    );
    assert!(RUNNER.spawned_for(&caller).is_empty());

    let above = http
        .post(
            Who::A1,
            ROUTE,
            &json!({ "func": "stock.big.list", "format": "csv" }),
        )
        .await;
    assert_eq!(above.status, 200, "{}", above.body);
    assert!(
        above
            .header("content-type")
            .is_some_and(|value| value.starts_with("application/json")),
        "{:?}",
        above.headers
    );
    assert_eq!(above.header("content-disposition"), None);
    let envelope = above.json();
    assert_eq!(envelope["ok"], json!(true), "{envelope}");
    let id: Id = envelope["data"]["task"]["id"]
        .as_str()
        .expect("a task ref")
        .parse()
        .expect("a ulid");
    assert_eq!(
        envelope["data"].as_object().map(|object| object.len()),
        Some(1)
    );
    let spawned = RUNNER.spawned_for(&caller);
    assert_eq!(spawned.len(), 1);
    let (spawned_id, spec) = &spawned[0];
    assert_eq!(*spawned_id, id);
    assert_eq!(spec.function, FUNCTION);
    assert_eq!(spec.params["func"], json!("stock.big.list"));
    assert_eq!(spec.params["format"], json!("csv"));
    assert_eq!(spec.class, "export");
    assert!(!spec.resumable);
    assert_eq!(spec.principal.id, caller.id);
    assert_eq!(
        spec.request_id.as_deref(),
        above.header("x-keel-request-id")
    );

    let dir = tempfile::tempdir().expect("a directory");
    let sink = Arc::new(SinkDouble {
        id,
        dir: dir.path().to_path_buf(),
        progress: Mutex::new(Vec::new()),
        file: Mutex::new(None),
    });
    let service = harness
        .registry()
        .services()
        .get::<dyn Caller>()
        .expect("the kernel provides the caller");
    let answer = service
        .call(
            spec.principal.clone(),
            &spec.function,
            spec.params.clone(),
            CallerOptions {
                request_id: Some(format!("task-{id}")),
                task: Some(TaskHandle::new(sink.clone())),
            },
        )
        .await
        .expect("the job writes the export");
    let filename = format!("stock.big.list-{}.csv", today(&harness));
    assert_eq!(
        answer,
        json!({ "filename": filename, "contentType": CSV_TYPE })
    );
    assert_eq!(
        *sink.file.lock().unwrap_or_else(PoisonError::into_inner),
        Some(DownloadFile::new(filename, CSV_TYPE))
    );
    let written = std::fs::read_to_string(sink.output_path()).expect("the output file");
    let lines = csv_lines(&written);
    assert_eq!(lines.len(), BIG_TOTAL as usize + 1);
    assert_eq!(lines[0], "Name");
    assert_eq!(lines[1], "row-1");
    assert_eq!(lines[BIG_TOTAL as usize], format!("row-{BIG_TOTAL}"));
    let progress = sink
        .progress
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert_eq!(
        progress,
        [
            (4000, Some(BIG_TOTAL)),
            (8000, Some(BIG_TOTAL)),
            (BIG_TOTAL, Some(BIG_TOTAL))
        ]
    );
    assert_eq!(
        RUNNER.spawned_for(&caller).len(),
        1,
        "the job spawned nothing"
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;

    let bare = Harness::new().module(&STOCK).boot().await;
    let reply = bare
        .call(Who::A1, FUNCTION, json!({ "func": "stock.big.list" }))
        .await;
    let error = reply.error().expect("no runner is composed");
    assert_eq!(error.code, ErrorCode::Unavailable);
    assert_eq!(error.key, TASKS_UNAVAILABLE_KEY);
    assert_eq!(error.params["rows"], json!(BIG_TOTAL));
    assert_eq!(error.params["threshold"], json!(EXPORT_TASK_ROWS));
    bare.finish().await;
}

#[keel_testkit::tokio::test]
async fn a_band_of_a_report_exports_with_the_reports_parameters_and_its_projected_columns() {
    let harness = Harness::new().module(&STOCK).boot().await;
    let accounts = harness.fixtures().accounts.clone();
    stock(accounts.a1, &[("apple", 3), ("berry", 5), ("cherry", 7)]);
    stock(accounts.a2, &[("date", 9)]);
    stock(accounts.unrelated, &[("fig", 11)]);
    let http = harness.http().await;

    let csv = http
        .post(
            Who::A1,
            ROUTE,
            &json!({ "func": "stock.report.levels", "band": "items", "params": { "min": 4 } }),
        )
        .await;
    assert_eq!(csv.status, 200, "{}", csv.body);
    assert_eq!(csv.header("content-type"), Some(CSV_TYPE));
    assert_eq!(
        csv.header("content-disposition"),
        Some(
            format!(
                "attachment; filename=\"stock.report.levels-items-{}.csv\"",
                today(&harness)
            )
            .as_str()
        )
    );
    assert_eq!(
        csv_lines(&csv.body),
        ["Name,Quantity", "berry,5", "cherry,7"]
    );

    let json = http
        .post(
            Who::Reseller,
            ROUTE,
            &json!({ "func": "stock.report.levels", "band": "items", "format": "json", "columns": ["accountId", "qty"] }),
        )
        .await;
    assert_eq!(json.status, 200, "{}", json.body);
    let rows: Value = serde_json::from_str(&json.body).expect("one JSON array");
    let rows = rows.as_array().expect("an array");
    assert_eq!(rows.len(), 4, "{rows:?}");
    assert!(rows.contains(&json!({ "accountId": accounts.a2.to_string(), "qty": 9 })));
    assert!(
        rows.iter()
            .all(|row| row.as_object().map(|row| row.len()) == Some(2))
    );

    let unknown = http
        .post(
            Who::A1,
            ROUTE,
            &json!({ "func": "stock.report.levels", "band": "shelves" }),
        )
        .await;
    assert_eq!(unknown.status, 422, "{}", unknown.body);
    let error = &unknown.json()["error"];
    assert_eq!(error["fields"][0]["path"], json!("band"));
    assert_eq!(error["fields"][0]["key"], json!("keel.error.export_band"));
    assert_eq!(error["fields"][0]["params"]["band"], json!("shelves"));

    let above = http
        .post(
            Who::A1,
            ROUTE,
            &json!({ "func": "stock.report.levels", "band": "items", "columns": ["accountId"] }),
        )
        .await;
    assert_eq!(above.status, 422, "{}", above.body);
    assert_eq!(
        above.json()["error"]["fields"][0]["key"],
        json!(COLUMNS_KEY)
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[keel_testkit::tokio::test]
async fn keel_export_of_a_function_the_caller_may_not_call_is_not_found() {
    let harness = Harness::new().module(&STOCK).boot().await;
    let refused = |reply: keel_testkit::Reply| {
        let error = reply.error().cloned().expect("an error");
        assert_eq!(error.code, ErrorCode::NotFound, "{error:?}");
        assert_eq!(error.key, "keel.error.function_not_found");
        error
    };
    let admin_only = harness
        .call(Who::A1, FUNCTION, json!({ "func": "stock.secret.list" }))
        .await;
    assert_eq!(
        refused(admin_only).params["function"],
        json!("stock.secret.list")
    );
    let served = harness
        .call(Who::Admin, FUNCTION, json!({ "func": "stock.secret.list" }))
        .await;
    assert!(served.download.is_some(), "{:?}", served.error());

    let unknown = harness
        .call(Who::A1, FUNCTION, json!({ "func": "stock.nothing.list" }))
        .await;
    refused(unknown);
    let not_a_list = harness
        .call(Who::A1, FUNCTION, json!({ "func": "stock.count" }))
        .await;
    refused(not_a_list);
    let itself = harness
        .call(Who::A1, FUNCTION, json!({ "func": FUNCTION }))
        .await;
    refused(itself);

    let mut scoped = harness.principal(Who::A1).clone();
    scoped.scope = Scope::Functions(vec![FUNCTION.to_owned()]);
    let out_of_scope = harness
        .call_as_principal(&scoped, FUNCTION, json!({ "func": "stock.item.list" }))
        .await;
    refused(out_of_scope);
    scoped.scope = Scope::Functions(vec![FUNCTION.to_owned(), "stock.item.list".to_owned()]);
    let in_scope = harness
        .call_as_principal(&scoped, FUNCTION, json!({ "func": "stock.item.list" }))
        .await;
    assert!(in_scope.download.is_some(), "{:?}", in_scope.error());
    harness.finish().await;
}
