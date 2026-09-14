fn main() -> keel::ExitCode {
    let runtime = keel::Runtime::new()
        .module(&keel_system::auth::MODULE)
        .module(&keel_system::settings::MODULE)
        .module(&keel_system::audit::MODULE)
        .frontend(keel::embed_frontend!("../../frontend/dist/shell"));
    #[cfg(feature = "demo")]
    let runtime = runtime.product(&keel_demo::MANIFEST);
    runtime.run()
}
