use keel_release::Version;
use serde::Deserialize;
use serde_yaml_ng::Value;

const CHANNEL_FILE: &str = include_str!("../channel.json");
const WORKFLOW: &str = include_str!("../../../.github/workflows/release.yml");
const REFRESH: &str = include_str!("../../../.github/workflows/refresh-channel.yml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChannelFile {
    contract: u32,
    min_upgrade_from: Version,
}

#[test]
fn the_channel_file_names_a_contract_and_a_floor_the_workspace_version_admits() {
    let file: ChannelFile = serde_json::from_str(CHANNEL_FILE).unwrap();
    let workspace = Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
    assert!(file.contract >= 1, "the contract starts at 1");
    assert!(
        file.min_upgrade_from <= workspace,
        "min_upgrade_from {} is above the workspace version {workspace}; the next release would admit nothing",
        file.min_upgrade_from
    );
}

#[test]
fn the_release_workflow_triggers_on_release_tags_alone() {
    let workflow: Value = serde_yaml_ng::from_str(WORKFLOW).unwrap();
    let on = workflow["on"]
        .as_mapping()
        .expect("the workflow has an `on` mapping");
    assert_eq!(
        on.keys().collect::<Vec<_>>(),
        [&Value::from("push")],
        "push is the one trigger"
    );
    let push = on[&Value::from("push")]
        .as_mapping()
        .expect("push has a mapping");
    assert_eq!(
        push.keys().collect::<Vec<_>>(),
        [&Value::from("tags")],
        "push filters on tags alone"
    );
    assert_eq!(
        push[&Value::from("tags")],
        Value::Sequence(vec![Value::from("keel-v*")])
    );
    let publish = workflow["jobs"]["release"]["steps"]
        .as_sequence()
        .expect("the release job has steps")
        .iter()
        .find(|step| {
            step["run"]
                .as_str()
                .is_some_and(|run| run.contains("hack/release.py"))
        })
        .expect("a step runs hack/release.py");
    assert_eq!(
        publish["run"].as_str().unwrap().trim(),
        "python3 hack/release.py --tag \"$GITHUB_REF_NAME\" --binary target/x86_64-unknown-linux-musl/release/keel --keel-release target/debug/keel-release"
    );
    let steps = workflow["jobs"]["release"]["steps"].as_sequence().unwrap();
    let build = steps
        .iter()
        .position(|step| step["name"].as_str() == Some("Build keel-release"))
        .expect("a step builds keel-release");
    let publish_at = steps
        .iter()
        .position(|step| {
            step["run"]
                .as_str()
                .is_some_and(|run| run.contains("hack/release.py"))
        })
        .unwrap();
    assert!(
        build < publish_at,
        "keel-release is built before the secrets are handed out"
    );
    assert_eq!(
        steps[build]["run"].as_str(),
        Some("cargo build -p keel-release")
    );
    assert!(steps[build]["env"].is_null() && steps[build]["with"].is_null());
    let env = publish["env"]
        .as_mapping()
        .expect("the publish step has env");
    assert_eq!(
        env[&Value::from("KEEL_RELEASE_KEY")],
        Value::from("${{ secrets.KEEL_RELEASE_KEY }}")
    );
    assert_eq!(
        env[&Value::from("KEEL_RELEASE_ASTRUM_PASSWORD")],
        Value::from("${{ secrets.KEEL_RELEASE_ASTRUM_PASSWORD }}")
    );
    assert!(
        !WORKFLOW.contains("workflow_dispatch") && !WORKFLOW.contains("pull_request"),
        "no other event starts a release"
    );
}

#[test]
fn the_refresh_workflow_runs_weekly_and_signs_both_heads_again_with_the_two_secrets() {
    let workflow: Value = serde_yaml_ng::from_str(REFRESH).unwrap();
    let on = workflow["on"]
        .as_mapping()
        .expect("the workflow has an `on` mapping");
    assert_eq!(
        on.keys().collect::<Vec<_>>(),
        [&Value::from("schedule"), &Value::from("workflow_dispatch")]
    );
    let schedule = on[&Value::from("schedule")]
        .as_sequence()
        .expect("schedule is a list");
    assert_eq!(schedule.len(), 1);
    let cron = schedule[0]["cron"].as_str().expect("a cron line");
    let fields: Vec<&str> = cron.split(' ').collect();
    assert_eq!(fields.len(), 5);
    assert!(fields[0].parse::<u8>().is_ok(), "a fixed minute: {cron}");
    assert!(fields[1].parse::<u8>().is_ok(), "a fixed hour: {cron}");
    assert_eq!(&fields[2..4], ["*", "*"]);
    assert!(fields[4].parse::<u8>().is_ok(), "a fixed weekday: {cron}");
    assert_eq!(
        workflow["permissions"],
        serde_yaml_ng::from_str::<Value>("contents: read").unwrap()
    );
    let job = &workflow["jobs"]["refresh"];
    assert_eq!(
        job["if"].as_str(),
        Some("github.repository == 'GlobalArtInc/keel'")
    );
    assert_eq!(job["runs-on"].as_str(), Some("ubuntu-24.04"));
    assert_eq!(job["timeout-minutes"].as_u64(), Some(20));
    let steps = job["steps"].as_sequence().expect("the job has steps");
    let position = |name: &str| {
        steps
            .iter()
            .position(|step| step["name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("a step named {name}"))
    };
    let build = &steps[position("Build keel-release")];
    assert_eq!(build["run"].as_str(), Some("cargo build -p keel-release"));
    assert!(
        build["env"].is_null() && build["with"].is_null(),
        "the build sees no secret"
    );
    let stable = &steps[position("Refresh the stable head")];
    let beta = &steps[position("Refresh the beta head")];
    assert!(position("Build keel-release") < position("Refresh the stable head"));
    assert!(position("Refresh the stable head") < position("Refresh the beta head"));
    assert_eq!(
        stable["run"].as_str(),
        Some("python3 hack/release.py --refresh stable --keel-release target/debug/keel-release")
    );
    assert_eq!(
        beta["run"].as_str(),
        Some("python3 hack/release.py --refresh beta --keel-release target/debug/keel-release")
    );
    assert!(stable["if"].is_null());
    assert_eq!(beta["if"].as_str(), Some("${{ !cancelled() }}"));
    for step in [stable, beta] {
        let env = step["env"].as_mapping().expect("the refresh step has env");
        assert_eq!(
            env[&Value::from("KEEL_RELEASE_KEY")],
            Value::from("${{ secrets.KEEL_RELEASE_KEY }}")
        );
        assert_eq!(
            env[&Value::from("KEEL_RELEASE_ASTRUM_PASSWORD")],
            Value::from("${{ secrets.KEEL_RELEASE_ASTRUM_PASSWORD }}")
        );
    }
    assert_eq!(REFRESH.matches("secrets.").count(), 4);
    let cache = steps
        .iter()
        .find(|step| step["uses"].as_str() == Some("Swatinem/rust-cache@v2"))
        .expect("the build is cached");
    assert_eq!(cache["with"]["shared-key"].as_str(), Some("refresh"));
    assert!(!REFRESH.contains("pull_request") && !REFRESH.contains("push:"));
}

#[test]
fn a_release_and_a_refresh_share_one_concurrency_group_and_never_run_at_once() {
    for text in [WORKFLOW, REFRESH] {
        let workflow: Value = serde_yaml_ng::from_str(text).unwrap();
        assert_eq!(
            workflow["concurrency"]["group"].as_str(),
            Some("release-channel")
        );
        assert_eq!(
            workflow["concurrency"]["cancel-in-progress"].as_bool(),
            Some(false)
        );
    }
}
