use keel_release::Version;
use serde::Deserialize;
use serde_yaml_ng::Value;

const CHANNEL_FILE: &str = include_str!("../channel.json");
const WORKFLOW: &str = include_str!("../../../.github/workflows/release.yml");

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
        "python3 hack/release.py --tag \"$GITHUB_REF_NAME\" --binary target/x86_64-unknown-linux-musl/release/keel"
    );
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
