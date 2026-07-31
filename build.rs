fn main() {
    slint_build::compile("src/gui/ui/app.slint").expect("compile app.slint");

    // Real camera backends are compiled when their feature is set, or when the
    // target OS ships the native nokhwa capture APIs with no extra system deps
    // (macOS AVFoundation, Windows MediaFoundation). Backend-specific cfgs keep
    // each implementation gated by the dependencies it actually needs.
    println!("cargo:rustc-check-cfg=cfg(camera_backend)");
    println!("cargo:rustc-check-cfg=cfg(nokhwa_backend)");
    println!("cargo:rustc-check-cfg=cfg(numanager_backend)");
    let nokhwa_feature = std::env::var_os("CARGO_FEATURE_NOKHWA_CAMERA").is_some();
    let numanager_feature = std::env::var_os("CARGO_FEATURE_NUMANAGER_CAMERA").is_some();
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let native_nokhwa = matches!(target_os.as_str(), "macos" | "windows");
    if nokhwa_feature || native_nokhwa {
        println!("cargo:rustc-cfg=nokhwa_backend");
    }
    if numanager_feature {
        println!("cargo:rustc-cfg=numanager_backend");
    }
    if nokhwa_feature || native_nokhwa || numanager_feature {
        println!("cargo:rustc-cfg=camera_backend");
    }

    // When the ML band detector is compiled in, make sure its weights were
    // actually fetched from Git LFS. A fresh clone without `git lfs pull` leaves
    // a ~130-byte pointer file in place of the real (tens-of-MB) `.bpk`, which
    // would otherwise fail cryptically at model-load time.
    if std::env::var_os("CARGO_FEATURE_GELGENIE_ML").is_some() {
        check_model_weights_present();
    }
}

/// Fail the build with a clear message if any bundled `assets/models/*.bpk` is
/// suspiciously small — i.e. an unfetched Git LFS pointer rather than the real
/// weights. A real model is > 10 MB; a pointer is a few hundred bytes.
fn check_model_weights_present() {
    // Absent files are fine (e.g. a crates.io build without the bundled assets);
    // only an *existing* too-small file signals a missing LFS object.
    const MIN_BYTES: u64 = 1_000_000; // 1 MB — far above any pointer, below any model
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let dir = std::path::Path::new(&manifest)
        .join("assets")
        .join("models");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("bpk") {
            continue;
        }
        // Re-run the check when the file changes (e.g. after `git lfs pull`).
        println!("cargo:rerun-if-changed={}", path.display());
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if size < MIN_BYTES {
            panic!(
                "\n\nGit LFS object missing: {} is only {size} bytes — this looks \
                 like an unfetched Git LFS pointer, not the real model weights.\n\n\
                 Fetch the weights:\n    git lfs install && git lfs pull\n\n\
                 Or build without the ML band detector:\n    \
                 cargo build --no-default-features --features camera\n",
                path.display()
            );
        }
    }
}
