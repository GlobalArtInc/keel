use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use keel_kernel::Composition;
use keel_kernel::keel::FUNCTIONS;
use serde_json::Value;

const UPDATE: &str = "KEEL_UPDATE_SNAPSHOTS";

fn snapshots_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/fixtures/descriptors")
}

fn render(descriptor: &Value) -> String {
    let mut text = serde_json::to_string_pretty(descriptor).expect("the descriptor serialises");
    text.push('\n');
    text
}

#[test]
fn the_descriptors_of_the_kernel_module_match_the_committed_snapshots() {
    let frozen = Composition::new()
        .resolve()
        .expect("the bare composition resolves")
        .init()
        .expect("the kernel module registers")
        .freeze()
        .expect("the registries freeze");
    let registry = frozen.registry();
    let update = std::env::var_os(UPDATE).is_some();
    let mut names = BTreeSet::new();
    let mut stale = Vec::new();
    for function in registry.functions() {
        assert_eq!(function.module(), "keel");
        names.insert(function.name().to_owned());
        let descriptor = serde_json::to_value(function.descriptor()).expect("serialises");
        let path = snapshots_dir().join(format!("{}.json", function.name()));
        let rendered = render(&descriptor);
        if update {
            fs::write(&path, &rendered).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            continue;
        }
        let committed = fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "{}: {e}; run the test with {UPDATE}=1 to write the snapshot",
                path.display()
            )
        });
        if committed != rendered {
            stale.push(function.name().to_owned());
        }
    }
    assert!(
        stale.is_empty(),
        "descriptors differ from their snapshots: {stale:?}; review the change and run the test with {UPDATE}=1"
    );
    let expected: BTreeSet<String> = FUNCTIONS.iter().map(|name| (*name).to_owned()).collect();
    assert_eq!(names, expected);
    let files: BTreeSet<String> = fs::read_dir(snapshots_dir())
        .expect("the snapshot directory exists")
        .map(|entry| {
            entry
                .expect("a readable entry")
                .file_name()
                .to_string_lossy()
                .trim_end_matches(".json")
                .to_owned()
        })
        .collect();
    assert_eq!(
        files, names,
        "every snapshot file belongs to a registered function"
    );
}
