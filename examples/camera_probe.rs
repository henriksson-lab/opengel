//! Drive the first nu-manager camera end to end and report what comes back:
//! `cargo run --release --example camera_probe`.
//!
//! Bring-up diagnostic for a bench camera that opens but returns nothing usable.
//! Unlike the GUI, every error is printed rather than swallowed, and each frame
//! is summarized (geometry, pixel format, min/mean/max) so "the preview is
//! black" can be told apart from "the capture failed".

#[cfg(numanager_backend)]
fn main() {
    use opengel::camera::{Camera, Exposure};

    let cams = match opengel::camera::numanager_backend::list_cameras() {
        Ok(cams) => cams,
        Err(e) => {
            println!("discovery failed: {e}");
            return;
        }
    };
    if cams.is_empty() {
        println!("no nu-manager cameras found");
        return;
    }
    for cam in &cams {
        println!("[{}] {}", cam.index, cam.name);
    }

    let mut cam = match opengel::camera::numanager_backend::open(0) {
        Ok(cam) => cam,
        Err(e) => {
            println!("open failed: {e}");
            return;
        }
    };
    let caps = cam.capabilities();
    println!(
        "capabilities: manual_exposure={} gain={} range={:?}",
        caps.manual_exposure, caps.gain, caps.exposure_range_s
    );
    println!("exposure now: {:?} s", cam.current_exposure_s());

    for seconds in [0.01, 0.1, 0.5, 2.0] {
        match cam.set_exposure(Exposure::Manual(seconds)) {
            Ok(applied) => println!(
                "\nset_exposure({seconds}) -> applied={applied}, device reports {:?} s",
                cam.current_exposure_s()
            ),
            Err(e) => {
                println!("\nset_exposure({seconds}) failed: {e}");
                continue;
            }
        }
        match cam.capture() {
            Ok(frame) => {
                // 16-bit stats, not 8: whether the samples reach full scale is
                // exactly the question, and to_luma8 throws away the evidence.
                let deep = frame.to_luma16();
                let (dmin, dmax) = deep.pixels().fold((u16::MAX, 0u16), |(lo, hi), p| {
                    (lo.min(p.0[0]), hi.max(p.0[0]))
                });
                let clipped = deep.pixels().filter(|p| p.0[0] == u16::MAX).count();
                println!(
                    "  16-bit: min={dmin} max={dmax} clipped={clipped} ({:.2}% of full scale)",
                    100.0 * dmax as f64 / u16::MAX as f64
                );
                let gray = frame.to_luma8();
                let (min, max, sum) = gray.pixels().fold((255u8, 0u8, 0u64), |(lo, hi, s), p| {
                    (lo.min(p.0[0]), hi.max(p.0[0]), s + p.0[0] as u64)
                });
                let n = gray.pixels().len().max(1) as f64;
                let nonzero = gray.pixels().filter(|p| p.0[0] > 0).count();
                println!(
                    "  frame {}x{}  min={min} max={max} mean={:.2}  nonzero={:.1}%",
                    gray.width(),
                    gray.height(),
                    sum as f64 / n,
                    100.0 * nonzero as f64 / n
                );
            }
            Err(e) => println!("  capture failed: {e}"),
        }
    }

    // Repeated grabs at one exposure: this is what the live preview does, and a
    // camera that drops the occasional frame behaves very differently from one
    // that dies on every grab.
    for seconds in [0.1, 0.5] {
        let _ = cam.set_exposure(Exposure::Manual(seconds));
        // The sequence matters, not just the count: every-other-frame failure
        // points at the stream being torn down and restarted per grab, while a
        // random scatter points at a timeout.
        let mut pattern = String::new();
        let mut ok = 0usize;
        let mut errors: Vec<String> = Vec::new();
        let mut elapsed = Vec::new();
        for _ in 0..12 {
            let start = std::time::Instant::now();
            let result = cam.capture();
            elapsed.push(start.elapsed().as_millis());
            match result {
                Ok(_) => {
                    ok += 1;
                    pattern.push('.');
                }
                Err(e) => {
                    pattern.push('X');
                    errors.push(e.to_string());
                }
            }
        }
        println!("\nburst at {seconds} s: {ok}/12 succeeded   [{pattern}]  (. = frame, X = error)");
        println!("  grab times ms: {elapsed:?}");
        let mut seen: Vec<&String> = Vec::new();
        for e in &errors {
            if !seen.contains(&e) {
                seen.push(e);
                println!("  {} x {e}", errors.iter().filter(|o| *o == e).count());
            }
        }
    }

    // Does an immediate second attempt succeed? If a retry always works, the
    // failure is a startup artefact of the grab rather than a dead device — and
    // that is the difference between a one-line workaround here and a driver fix.
    let _ = cam.set_exposure(Exposure::Manual(0.1));
    let (mut first_failed, mut retry_ok) = (0usize, 0usize);
    for _ in 0..12 {
        if cam.capture().is_err() {
            first_failed += 1;
            if cam.capture().is_ok() {
                retry_ok += 1;
            }
        }
    }
    println!("\nretry after a failure: {retry_ok}/{first_failed} recovered on the next grab");
}

#[cfg(not(numanager_backend))]
fn main() {
    println!("built without the nu-manager backend");
}
