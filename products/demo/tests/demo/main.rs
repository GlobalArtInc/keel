mod audit;
mod day_boundary;
mod note;
mod problems;
mod runtime;
mod tasks;
mod tokens;

use keel_demo::MANIFEST;
use keel_testkit::{Builder, Harness};

fn demo() -> Builder {
    Harness::new()
        .product(&MANIFEST)
        .module(&keel_system::tasks::MODULE)
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
}
