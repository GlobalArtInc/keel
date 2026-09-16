use std::path::Path;
use std::time::Duration;

use keel::prelude::*;
use keel::sdk::serde_json;
use keel::types::{TaskProgress, TaskState};
use keel_demo::note::{EXPORT_CLASS, EXPORT_CSV_HEADER, EXPORT_LOG_PREFIX, export_file_of};
use keel_system::tasks::api::Drain;
use keel_system::tasks::{DOWNLOAD_FUNCTION, FRAME_KIND, output_path};
use keel_testkit::{Answer, Harness, Reply, TestHttp, Who, tokio};

const WAIT: Duration = Duration::from_secs(30);

fn drain(harness: &Harness) -> Arc<Drain> {
    harness
        .registry()
        .services()
        .get::<Drain>()
        .expect("the tasks module provides the drain")
}

async fn wait_final(harness: &Harness, id: Id) -> TaskState {
    tokio::time::timeout(WAIT, drain(harness).wait(id))
        .await
        .expect("the job ends in time")
        .expect("the state is read")
}

async fn wait_requeued(harness: &Harness, id: Id) {
    let sql = format!("SELECT state FROM task_job WHERE id = '{id}'");
    for _ in 0..600 {
        let rows = harness.database().rows(&sql).await;
        if rows.first().is_some_and(|state| state != "interrupted") {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the job {id} was not requeued by the worker of the second kernel");
}

async fn wait_progress(harness: &Harness, id: Id, at_least: u64) {
    let sql = format!("SELECT progress_done::text FROM task_job WHERE id = '{id}'");
    for _ in 0..600 {
        let rows = harness.database().rows(&sql).await;
        if rows
            .first()
            .and_then(|done| done.parse::<u64>().ok())
            .is_some_and(|done| done >= at_least)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the job {id} did not reach {at_least} notes");
}

async fn create(harness: &Harness, who: Who, title: &str) -> Id {
    let reply = harness
        .call(who, "note.edit", json!({ "title": title }))
        .await;
    let data = reply
        .data()
        .unwrap_or_else(|| panic!("note.edit failed: {:?}", reply.error()));
    data["id"].as_str().expect("an id").parse().expect("a ulid")
}

fn task_ref(data: &Value) -> Id {
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

fn task_of_reply(reply: &Reply) -> Id {
    task_ref(
        reply
            .data()
            .unwrap_or_else(|| panic!("a task ref was expected: {:?}", reply.error())),
    )
}

fn task_of_answer(answer: &Answer) -> Id {
    assert_eq!(answer.status, 200, "{}", answer.body);
    let envelope = answer.json();
    assert_eq!(envelope["ok"], json!(true), "{envelope}");
    task_ref(&envelope["data"])
}

async fn export(http: &TestHttp, who: Who, params: Value) -> Id {
    task_of_answer(&http.post(who, "/api/v1/call/note.export", &params).await)
}

async fn detail(harness: &Harness, who: Who, id: Id) -> Value {
    let got = harness.call(who, "task.get", json!({ "id": id })).await;
    got.data()
        .unwrap_or_else(|| panic!("task.get failed: {:?}", got.error()))
        .clone()
}

fn unstamped(detail: &Value) -> Vec<String> {
    detail["logTail"]
        .as_array()
        .expect("the log tail")
        .iter()
        .map(|line| {
            line.as_str()
                .and_then(|line| line.split_once(' '))
                .map(|(_, rest)| rest.to_owned())
                .expect("a stamped line")
        })
        .collect()
}

fn exported_titles(detail: &Value) -> Vec<String> {
    let mut titles: Vec<String> = unstamped(detail)
        .iter()
        .map(|line| {
            line.strip_prefix(&format!("{EXPORT_LOG_PREFIX} "))
                .unwrap_or_else(|| panic!("a log line names an exported note: {line}"))
                .to_owned()
        })
        .collect();
    titles.sort();
    titles
}

fn exported_result(exported: u64, id: Id) -> Value {
    json!({
        "exported": exported,
        "download": { "function": DOWNLOAD_FUNCTION, "params": { "id": id.to_string() } }
    })
}

fn output_lines(data_dir: &Path, id: Id) -> Vec<String> {
    std::fs::read_to_string(output_path(data_dir, id))
        .expect("the export output exists")
        .lines()
        .map(str::to_owned)
        .collect()
}

fn titles_in(lines: &[String]) -> Vec<String> {
    let mut titles: Vec<String> = lines
        .iter()
        .skip(1)
        .map(|line| line.split(',').nth(1).expect("a title column").to_owned())
        .collect();
    titles.sort();
    titles
}

async fn steps(harness: &Harness, id: Id, with_checkpoint: bool) -> u64 {
    let condition = if with_checkpoint {
        "IS NOT NULL"
    } else {
        "IS NULL"
    };
    harness
        .database()
        .rows(&format!(
            "SELECT count(*)::text FROM task_step WHERE job_id = '{id}' AND checkpoint_json {condition}"
        ))
        .await[0]
        .parse()
        .expect("a count")
}

#[tokio::test]
async fn note_export_runs_as_a_task_with_progress_a_log_line_per_note_and_a_checkpoint_after_each()
{
    let harness = crate::demo().boot().await;
    assert_eq!(
        harness.modules(),
        ["keel", "cron", "notify", "task", "note", "testkit"]
    );
    let mut titles = vec!["alpha".to_owned(), "beta".to_owned(), "gamma".to_owned()];
    for title in &titles {
        create(&harness, Who::A1, title).await;
    }
    create(&harness, Who::A2, "theirs").await;
    titles.sort();

    let http = harness.http().await;
    let id = export(&http, Who::A1, json!({ "format": "csv" })).await;
    assert_eq!(wait_final(&harness, id).await, TaskState::Done);

    let detail = detail(&harness, Who::A1, id).await;
    assert_eq!(detail["function"], json!("note.export"));
    assert_eq!(detail["class"], json!(EXPORT_CLASS));
    assert_eq!(detail["state"], json!("done"));
    assert_eq!(detail["resumable"], json!(true));
    assert_eq!(detail["progress"], json!("3/3"));
    assert_eq!(detail["result"], exported_result(3, id));
    assert_eq!(detail["params"]["format"], json!("csv"));
    assert_eq!(detail["checkpoint"]["written"], json!(3));
    assert!(detail["checkpoint"]["lastId"].is_string(), "{detail}");
    assert_eq!(exported_titles(&detail), titles);
    assert_eq!(steps(&harness, id, false).await, 3);
    assert_eq!(steps(&harness, id, true).await, 3);

    let lines = output_lines(harness.data_dir(), id);
    assert_eq!(lines[0], EXPORT_CSV_HEADER);
    assert_eq!(lines.len(), 4);
    assert_eq!(titles_in(&lines), titles);

    let foreign = harness.call(Who::A2, "task.get", json!({ "id": id })).await;
    assert_eq!(foreign.status(), 404);
    let listed = harness.call(Who::A1, "task.list", json!({})).await;
    let page = listed.data().expect("the list answers");
    assert_eq!(page["total"], json!(1));
    assert_eq!(page["data"][0]["function"], json!("note.export"));
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn note_export_ends_with_exported_and_a_download_that_task_download_streams_as_the_callers_csv()
 {
    let harness = crate::demo().boot().await;
    create(&harness, Who::A1, "milk, eggs").await;
    create(&harness, Who::A1, "plain").await;
    create(&harness, Who::A2, "theirs").await;
    let http = harness.http().await;
    let id = export(&http, Who::A1, json!({ "format": "csv" })).await;
    assert_eq!(wait_final(&harness, id).await, TaskState::Done);
    let detail = detail(&harness, Who::A1, id).await;
    assert_eq!(detail["result"], exported_result(2, id));
    let download = &detail["result"]["download"];
    let route = format!(
        "/api/v1/x/task/{}?id={}",
        download["function"]
            .as_str()
            .and_then(|function| function.strip_prefix("task."))
            .expect("the download names a task function"),
        download["params"]["id"].as_str().expect("the job id")
    );
    let file = export_file_of(ExportFormat::Csv);

    let got = http.get(Who::A1, &route).await;
    assert_eq!(got.status, 200, "{}", got.body);
    assert_eq!(got.header("content-type"), Some(file.content_type.as_str()));
    assert_eq!(
        got.header("content-disposition"),
        Some(format!("attachment; filename=\"{}\"", file.filename).as_str())
    );
    assert_eq!(got.header("cache-control"), Some("no-store"));
    let lines: Vec<&str> = got.body.lines().collect();
    assert_eq!(lines[0], EXPORT_CSV_HEADER);
    assert_eq!(lines.len(), 3);
    let mut titles: Vec<String> = lines[1..]
        .iter()
        .map(|line| {
            if line.contains("\"milk, eggs\"") {
                "milk, eggs".to_owned()
            } else {
                line.split(',').nth(1).expect("a title column").to_owned()
            }
        })
        .collect();
    titles.sort();
    assert_eq!(titles, ["milk, eggs", "plain"]);
    assert_eq!(
        got.body,
        std::fs::read_to_string(output_path(harness.data_dir(), id)).expect("the file")
    );

    let foreign = http.get(Who::A2, &route).await;
    assert_eq!(foreign.status, 404, "{}", foreign.body);

    let as_json = export(&http, Who::A1, json!({ "format": "json" })).await;
    assert_eq!(wait_final(&harness, as_json).await, TaskState::Done);
    let got = http
        .get(Who::A1, &format!("/api/v1/x/task/download?id={as_json}"))
        .await;
    assert_eq!(got.status, 200, "{}", got.body);
    assert_eq!(got.header("content-type"), Some("application/json"));
    assert_eq!(
        got.header("content-disposition"),
        Some("attachment; filename=\"notes.json\"")
    );
    assert_eq!(got.body.lines().count(), 2);
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn note_export_is_cancelled_between_two_notes_when_paced() {
    let harness = crate::demo().boot().await;
    for title in ["one", "two", "three"] {
        create(&harness, Who::A1, title).await;
    }
    let mut listener = harness.live_events();
    let http = harness.http().await;
    let id = export(&http, Who::A1, json!({ "format": "json", "pace": 500 })).await;
    loop {
        let frame = tokio::time::timeout(WAIT, listener.recv())
            .await
            .expect("a frame arrives in time")
            .expect("the bus is open");
        assert_eq!(frame.kind, FRAME_KIND);
        let progress: TaskProgress = serde_json::from_value(frame.data).expect("a progress frame");
        if progress.id == id && progress.done >= 1 {
            assert_eq!(progress.state, TaskState::Running);
            break;
        }
    }
    let cancelled = harness
        .call(Who::A1, "task.cancel", json!({ "id": id }))
        .await;
    assert!(cancelled.data().is_some(), "{:?}", cancelled.error());
    assert_eq!(wait_final(&harness, id).await, TaskState::Cancelled);

    let detail = detail(&harness, Who::A1, id).await;
    assert_eq!(detail["state"], json!("cancelled"));
    assert_eq!(detail["result"], Value::Null);
    let done = detail["done"].as_u64().expect("the count");
    assert!((1..3).contains(&done), "{detail}");
    assert_eq!(detail["total"], json!(3));
    assert_eq!(exported_titles(&detail).len() as u64, done);
    let lines = output_lines(harness.data_dir(), id);
    assert_eq!(lines.len() as u64, done);
    for line in &lines {
        let record: Value = serde_json::from_str(line).expect("a json record");
        assert!(record["title"].is_string(), "{record}");
    }
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn note_export_interrupted_by_a_restart_resumes_from_its_checkpoint_and_writes_no_note_twice()
{
    let dir = tempfile::tempdir().expect("a data directory");
    let first = crate::demo().data_dir(dir.path()).boot().await;
    let mut titles: Vec<String> = ["one", "two", "three", "four"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    for title in &titles {
        create(&first, Who::A1, title).await;
    }
    titles.sort();
    let http = first.http().await;
    let id = export(&http, Who::A1, json!({ "format": "csv", "pace": 300 })).await;
    wait_progress(&first, id, 1).await;
    http.stop().await.expect("the server stops");
    let database = first.stop().await;

    let left = database
        .rows(&format!(
            "SELECT state || ' ' || progress_done || ' ' || coalesce(checkpoint_json->>'written', '-') FROM task_job WHERE id = '{id}'"
        ))
        .await;
    let mut parts = left[0].split(' ');
    assert_eq!(parts.next(), Some("interrupted"), "{left:?}");
    let written_before: u64 = parts
        .next()
        .and_then(|done| done.parse().ok())
        .expect("a count");
    assert!((1..4).contains(&written_before), "{left:?}");
    assert_eq!(
        parts.next(),
        Some(written_before.to_string().as_str()),
        "{left:?}"
    );
    assert_eq!(
        output_lines(dir.path(), id).len() as u64,
        written_before + 1
    );

    let second = crate::demo()
        .data_dir(dir.path())
        .database(database)
        .boot()
        .await;
    wait_requeued(&second, id).await;
    assert_eq!(wait_final(&second, id).await, TaskState::Done);
    let detail = detail(&second, Who::A1, id).await;
    assert_eq!(detail["state"], json!("done"));
    assert_eq!(detail["progress"], json!("4/4"));
    assert_eq!(detail["result"], exported_result(4, id));
    assert_eq!(detail["error"], Value::Null);
    assert_eq!(exported_titles(&detail), titles);
    let lines = output_lines(dir.path(), id);
    assert_eq!(lines[0], EXPORT_CSV_HEADER);
    assert_eq!(lines.len(), 5);
    assert_eq!(titles_in(&lines), titles);
    second.finish().await;
}

#[tokio::test]
async fn note_delete_above_ten_ids_becomes_a_task_that_deletes_in_chunks_of_ten() {
    let harness = crate::demo().boot().await;
    let mut ids = Vec::new();
    for index in 0..12 {
        ids.push(create(&harness, Who::A1, &format!("note {index}")).await);
    }
    let reply = harness
        .call(Who::A1, "note.delete", json!({ "ids": ids }))
        .await;
    let id = task_of_reply(&reply);
    assert_eq!(wait_final(&harness, id).await, TaskState::Done);

    let detail = detail(&harness, Who::A1, id).await;
    assert_eq!(detail["function"], json!("note.delete"));
    assert_eq!(detail["class"], json!("default"));
    assert_eq!(detail["resumable"], json!(false));
    assert_eq!(detail["state"], json!("done"));
    assert_eq!(detail["progress"], json!("12/12"));
    assert_eq!(
        detail["result"],
        json!([{ "deleted": 10 }, { "deleted": 2 }])
    );
    assert_eq!(
        unstamped(&detail),
        ["chunk 1/2: 10 ids", "chunk 2/2: 2 ids"]
    );
    assert_eq!(detail["checkpoint"]["done"], json!(2));
    assert_eq!(steps(&harness, id, false).await, 2);
    assert_eq!(steps(&harness, id, true).await, 2);
    let left = harness.call(Who::A1, "note.list", json!({})).await;
    assert_eq!(left.data().expect("a page")["total"], json!(0));
    let events = harness
        .database()
        .rows("SELECT count(*)::text FROM keel_outbox WHERE type = 'note.deleted'")
        .await;
    assert_eq!(events, ["12"]);

    let mut ten = Vec::new();
    for index in 0..10 {
        ten.push(create(&harness, Who::A1, &format!("again {index}")).await);
    }
    let inline = harness
        .call(Who::A1, "note.delete", json!({ "ids": ten }))
        .await;
    assert_eq!(inline.data(), Some(&json!({ "deleted": 10 })));
    let jobs = harness.call(Who::A1, "task.list", json!({})).await;
    assert_eq!(
        jobs.data().expect("the list answers")["total"],
        json!(1),
        "ten ids ran inline and spawned no job"
    );
    harness.finish().await;
}
