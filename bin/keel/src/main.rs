fn main() -> keel::ExitCode {
    let runtime = keel::Runtime::new()
        .module(&keel_system::auth::MODULE)
        .module(&keel_system::settings::MODULE)
        .module(&keel_system::audit::MODULE)
        .module(&keel_system::tasks::MODULE)
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .module(&keel_system::webhooks::MODULE)
        .module(&keel_system::update::MODULE)
        .module(&keel_system::backup::MODULE)
        .frontend(keel::embed_frontend!("../../frontend/dist/shell"));
    #[cfg(feature = "demo")]
    let runtime = runtime.product(&keel_demo::MANIFEST);
    #[cfg(feature = "dns")]
    let runtime = runtime.product(&keel_dns::MANIFEST);
    runtime.run()
}
