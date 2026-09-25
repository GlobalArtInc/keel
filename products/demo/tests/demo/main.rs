mod audit;
mod day_boundary;
mod lifecycle;
mod note;
mod problems;
mod report;
mod runtime;
mod tasks;
mod tokens;

use keel_demo::MANIFEST;
use keel_testkit::{Builder, Harness};

fn loaded(ids: &[&'static str]) -> Vec<&'static str> {
    let mut ids = ids.to_vec();
    if cfg!(feature = "host") {
        let at = ids
            .iter()
            .position(|id| *id == "notify")
            .unwrap_or(ids.len());
        ids.insert(at, "host");
    }
    ids
}

fn demo() -> Builder {
    Harness::new()
        .product(&MANIFEST)
        .module(&keel_system::tasks::MODULE)
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
}
