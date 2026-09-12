fn main() -> keel::ExitCode {
    let runtime = keel::Runtime::new();
    #[cfg(feature = "demo")]
    let runtime = runtime.product(&keel_demo::PRODUCT);
    runtime.run()
}
