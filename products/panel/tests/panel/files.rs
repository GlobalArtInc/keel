use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};

use keel::prelude::*;
use keel::types::TaskState;
use keel_system::tasks::api::Drain;
use keel_testkit::{ErrorCode, Harness, Reply, UploadPart, Who, tokio};

use crate::support::{PATIENCE, Site, create, data, panel, state_becomes};

const UPLOAD_ROUTE: &str = "/api/v1/x/hosting/file.upload";
const DOWNLOAD_ROUTE: &str = "/api/v1/x/hosting/file.download";
const OCTET_STREAM: &str = "application/octet-stream";
const OUTSIDE: &str = "hosting.file.error.outside";

async fn housed(harness: &Harness, site: &Site, name: &str) -> (String, u64) {
    let id = create(harness, Who::A1, name).await;
    state_becomes(harness, &id, "active").await;
    let got = harness
        .call(Who::A1, "hosting.user.get", json!({ "id": id }))
        .await;
    let uid = data(&got)["uid"].as_u64().expect("a uid");
    assert!(site.home(name).is_dir());
    (id, uid)
}

async fn settled(harness: &Harness, started: &Value) -> (TaskState, Value) {
    let task: Id = started["task"]["id"]
        .as_str()
        .expect("a task reference")
        .parse()
        .expect("a task id");
    let drain = harness
        .registry()
        .services()
        .get::<Drain>()
        .expect("the tasks module provides the drain");
    let state = tokio::time::timeout(PATIENCE, drain.wait(task))
        .await
        .expect("the job ends in time")
        .expect("the state is read");
    let detail = harness
        .call(Who::A1, "task.get", json!({ "id": task }))
        .await;
    (state, data(&detail).clone())
}

async fn done(harness: &Harness, func: &str, params: Value) {
    let reply = harness.call(Who::A1, func, params).await;
    let (state, detail) = settled(harness, data(&reply)).await;
    assert_eq!(state, TaskState::Done, "{func}: {detail}");
}

fn refused_outside(reply: &Reply) {
    let error = reply.error().expect("the path is refused");
    assert_eq!(
        (error.code, error.key.as_str()),
        (ErrorCode::Forbidden, OUTSIDE),
        "{error:?}"
    );
}

fn worker_calls(site: &Site) -> Vec<String> {
    site.calls()
        .into_iter()
        .filter(|line| line.starts_with("keel "))
        .collect()
}

#[tokio::test]
async fn a_path_escaping_the_home_is_refused() {
    let site = Site::new();
    let harness = panel(&site).await;
    let (id, _) = housed(&harness, &site, "files").await;
    housed(&harness, &site, "other").await;
    fs::write(site.home("other").join("secret.txt"), "theirs").expect("a file of another user");
    symlink(site.home("other"), site.home("files").join("escape")).expect("a link out of the home");

    let parent = harness
        .call(
            Who::A1,
            "hosting.file.list",
            json!({ "user": id, "path": "../keel-panel-other" }),
        )
        .await;
    refused_outside(&parent);
    let linked = harness
        .call(
            Who::A1,
            "hosting.file.list",
            json!({ "user": id, "path": "escape" }),
        )
        .await;
    refused_outside(&linked);
    let absolute = harness
        .call(
            Who::A1,
            "hosting.file.list",
            json!({ "user": id, "path": site.home("other").display().to_string() }),
        )
        .await;
    refused_outside(&absolute);

    let http = harness.http().await;
    let fetched = http
        .post(
            Who::A1,
            DOWNLOAD_ROUTE,
            &json!({ "user": id, "path": "escape/secret.txt" }),
        )
        .await;
    assert_eq!(fetched.status, 403, "{}", fetched.body);
    assert!(fetched.body.contains(OUTSIDE), "{}", fetched.body);
    assert!(!fetched.body.contains("theirs"));

    let planted = harness
        .call(
            Who::A1,
            "hosting.file.mkdir",
            json!({ "user": id, "path": "escape/planted" }),
        )
        .await;
    let (state, detail) = settled(&harness, data(&planted)).await;
    assert_eq!(state, TaskState::Failed, "{detail}");
    assert!(detail["error"].to_string().contains(OUTSIDE), "{detail}");
    assert!(!site.home("other").join("planted").exists());

    let foreign = harness
        .call(
            Who::A2,
            "hosting.file.list",
            json!({ "user": id, "path": "" }),
        )
        .await;
    assert!(
        foreign.error().is_some(),
        "another account never lists the home"
    );
    assert!(
        worker_calls(&site).is_empty(),
        "a refused path never reaches the worker: {:?}",
        worker_calls(&site)
    );
    assert_eq!(
        fs::read_to_string(site.home("other").join("secret.txt")).expect("the file stays"),
        "theirs"
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn files_are_written_as_the_hosting_user() {
    let site = Site::new();
    let harness = panel(&site).await;
    let (id, uid) = housed(&harness, &site, "files").await;
    let home = site.home("files");

    done(
        &harness,
        "hosting.file.mkdir",
        json!({ "user": id, "path": "site" }),
    )
    .await;
    let http = harness.http().await;
    let uploaded = http
        .upload(
            Who::A1,
            UPLOAD_ROUTE,
            &json!({ "user": id, "path": "site/index.html" }),
            &[UploadPart::new(
                "content",
                "index.html",
                OCTET_STREAM,
                b"<p>hello</p>",
            )],
        )
        .await;
    assert_eq!(uploaded.status, 200, "{}", uploaded.body);
    let (state, detail) = settled(&harness, &uploaded.json()["data"]).await;
    assert_eq!(state, TaskState::Done, "{detail}");
    assert_eq!(
        fs::read_to_string(home.join("site/index.html")).expect("the uploaded file"),
        "<p>hello</p>"
    );
    let write = format!(
        " {bin}/keel agent-home --home {home} write {home}/site/index.html",
        bin = site.path("bin").display(),
        home = home.display(),
    );
    assert!(
        worker_calls(&site).iter().any(|line| line
            .starts_with(&format!("keel agent-exec --uid {uid} "))
            && line.ends_with(&write)),
        "the upload ran the worker as uid {uid}: {:?}",
        worker_calls(&site)
    );

    let listed = harness
        .call(
            Who::A1,
            "hosting.file.list",
            json!({ "user": id, "path": "site" }),
        )
        .await;
    let entries = &data(&listed)["entries"];
    assert_eq!(entries[0]["name"], "index.html", "{listed:?}");
    assert_eq!(entries[0]["kind"], "file");
    assert_eq!(entries[0]["size"], 12);

    done(
        &harness,
        "hosting.file.rename",
        json!({ "user": id, "from": "site/index.html", "to": "site/main.html" }),
    )
    .await;
    done(
        &harness,
        "hosting.file.chmod",
        json!({ "user": id, "path": "site/main.html", "mode": "600" }),
    )
    .await;
    let mode = fs::metadata(home.join("site/main.html"))
        .expect("the renamed file")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    let fetched = http
        .post(
            Who::A1,
            DOWNLOAD_ROUTE,
            &json!({ "user": id, "path": "site/main.html" }),
        )
        .await;
    assert_eq!(fetched.status, 200, "{}", fetched.body);
    assert_eq!(fetched.body, "<p>hello</p>");
    assert!(
        fetched
            .header("content-disposition")
            .is_some_and(|value| value.contains("main.html"))
    );
    let taken = harness
        .call(
            Who::A1,
            "hosting.file.rename",
            json!({ "user": id, "from": "site", "to": "site" }),
        )
        .await;
    let (state, detail) = settled(&harness, data(&taken)).await;
    assert_eq!(state, TaskState::Failed, "{detail}");
    assert!(
        detail["error"]
            .to_string()
            .contains("hosting.file.error.exists"),
        "{detail}"
    );
    done(
        &harness,
        "hosting.file.delete",
        json!({ "user": id, "path": "site" }),
    )
    .await;
    assert!(!home.join("site").exists());

    let calls = worker_calls(&site);
    assert!(calls.len() >= 7, "{calls:?}");
    assert!(
        calls
            .iter()
            .all(|line| line.starts_with(&format!("keel agent-exec --uid {uid} "))),
        "every file operation runs as uid {uid}: {calls:?}"
    );
    assert!(
        fs::read_dir(site.path("transfer"))
            .expect("the transfer directory")
            .next()
            .is_none(),
        "no staged upload or download is left behind"
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}
