//! Runs the app on the desktop backend (`cargo run`).

#[cfg(target_os = "macos")]
fn main() -> impl std::process::Termination {
    use nsh_platform_macos as platform;
    env_logger::init();
    platform::run_app(
        platform::AppSource::Rust(platform::Root::of::<grkit_app::App>()),
        platform::AppOptions {
            title: "GRKit".to_owned(),
            size: (390, 844),
            ..Default::default()
        },
    )
}

#[cfg(not(target_os = "macos"))]
compile_error!("Run this binary on macOS; use `nsh run --target ios` for iOS.");
