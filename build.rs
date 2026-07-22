fn main() {
    slint_build::compile("src/gui/ui/app.slint").expect("compile app.slint");

    // The real camera backend (nokhwa) is compiled when either the `camera`
    // feature is set, or the target OS ships the native capture APIs with no
    // extra system deps (macOS AVFoundation, Windows MediaFoundation). Linux
    // stays opt-in via `--features camera` because it needs libv4l-dev at build
    // time. Both paths are unified behind the `camera_backend` cfg.
    println!("cargo:rustc-check-cfg=cfg(camera_backend)");
    let feature = std::env::var_os("CARGO_FEATURE_CAMERA").is_some();
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if feature || matches!(target_os.as_str(), "macos" | "windows") {
        println!("cargo:rustc-cfg=camera_backend");
    }
}
