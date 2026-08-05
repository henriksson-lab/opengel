//! Camera capture for the app: enumerate the available backends into one flat
//! list, and open whichever one the user picked.
//!
//! Scientific cameras come from [nu-manager](https://github.com/henriksson-lab/numanager)
//! as devices — it owns the hardware protocols and the USB autodiscovery, and
//! OpenGel just drives the typed device. Plain webcams still come from nokhwa
//! (useful for framing a gel with whatever is at hand), and the mock backend
//! keeps the UI fully usable with no hardware at all.

use std::cell::RefCell;

use image::DynamicImage;
use opengel::camera::mock;
use opengel::camera::{capture_bracket, Camera};
use opengel::core::model::CaptureMeta;

/// Default exposure bracket (seconds) for HDR capture.
pub const DEFAULT_BRACKET: [f64; 3] = [0.05, 0.2, 0.8];

#[derive(Debug, Clone, Copy)]
enum CameraChoice {
    #[cfg(all(numanager_backend, not(test)))]
    Numanager(usize),
    #[cfg(all(nokhwa_backend, not(test)))]
    Nokhwa(usize),
    Mock(usize),
}

/// Probe every compiled-in backend, in priority order.
///
/// Returns what was found and whether any backend *failed to answer*, which is
/// not the same as finding nothing: a probe that errored has told us nothing
/// about what is attached. See [`list_camera_names`], which is what acts on the
/// difference.
fn enumerate() -> (Vec<(String, CameraChoice)>, bool) {
    // Both are written only by the backend blocks below, and every one of those
    // is behind a `cfg` — a build with no real backend (including the test
    // harness, which compiles them out) leaves them untouched.
    #[allow(unused_mut)]
    let mut out = Vec::new();
    #[allow(unused_mut)]
    let mut probe_failed = false;
    #[cfg(all(numanager_backend, not(test)))]
    {
        use opengel::camera::numanager_backend;
        match numanager_backend::list_cameras() {
            Ok(cams) => out.extend(
                cams.into_iter()
                    .map(|c| (c.name, CameraChoice::Numanager(c.index))),
            ),
            // Worth saying out loud: discovery failing (rather than finding
            // nothing) usually means USB permissions — on Linux, the udev rule
            // in `packaging/` is missing. Silently showing no camera would send
            // people hunting in the wrong place.
            Err(e) => {
                probe_failed = true;
                eprintln!("nu-manager camera discovery failed: {e}");
            }
        }
    }
    #[cfg(all(nokhwa_backend, not(test)))]
    {
        use opengel::camera::nokhwa_backend;
        match nokhwa_backend::list_cameras() {
            Ok(cams) => out.extend(
                cams.into_iter()
                    .map(|c| (c.name, CameraChoice::Nokhwa(c.index))),
            ),
            Err(_) => probe_failed = true,
        }
    }
    (out, probe_failed)
}

/// Whether a listed choice is real hardware rather than the synthetic fallback.
fn is_real(choice: &CameraChoice) -> bool {
    !matches!(choice, CameraChoice::Mock(_))
}

thread_local! {
    /// The list [`list_camera_names`] last handed out, so [`open_camera_by_index`]
    /// resolves an index against exactly what the caller saw.
    static LISTED: RefCell<Vec<(String, CameraChoice)>> = const { RefCell::new(Vec::new()) };
}

/// Names of the available cameras. Order defines the index used by
/// [`open_camera_by_index`].
///
/// A re-probe never demotes a real camera to the mock. nu-manager's discovery
/// *opens* what it finds, so it cannot claim a USB interface we are already
/// holding open — which means the commonest reason a probe fails is that the
/// bench camera is working. Falling back to the synthetic gel there would swap
/// the user's camera out from under them mid-session, which is exactly what it
/// looks like: the demo gel appearing in the live preview for no reason.
pub fn list_camera_names() -> Vec<String> {
    let (mut choices, probe_failed) = enumerate();
    if !choices.iter().any(|(_, choice)| is_real(choice)) {
        let kept = LISTED.with(|listed| listed.borrow().clone());
        if probe_failed && kept.iter().any(|(_, choice)| is_real(choice)) {
            // Keep the previous list wholesale rather than merging: the index a
            // caller holds is a position in the list it was handed.
            choices = kept;
        }
    }
    if choices.is_empty() {
        choices.extend(
            mock::list_cameras()
                .into_iter()
                .map(|c| (c.name, CameraChoice::Mock(c.index))),
        );
    }
    let names = choices.iter().map(|(name, _)| name.clone()).collect();
    LISTED.with(|listed| *listed.borrow_mut() = choices);
    names
}

/// Resolve an index from the last [`list_camera_names`] without re-probing.
///
/// Re-probing here would be worse than wasteful: nu-manager's discovery *opens*
/// each device it finds, and a fresh probe could also renumber the list out
/// from under the selection the user made.
fn choice_at(index: usize) -> CameraChoice {
    let listed = LISTED.with(|listed| listed.borrow().get(index).map(|(_, choice)| *choice));
    listed.unwrap_or_else(|| {
        let (choices, _) = enumerate();
        let choice = choices.get(index).map(|(_, choice)| *choice);
        LISTED.with(|listed| *listed.borrow_mut() = choices);
        choice.unwrap_or(CameraChoice::Mock(0))
    })
}

/// Open the camera at `index` (position in [`list_camera_names`]). Falls back to
/// the mock backend when the real device can't be opened. Returns `(name, handle)`.
pub fn open_camera_by_index(index: usize) -> anyhow::Result<(String, Box<dyn Camera>)> {
    match choice_at(index) {
        #[cfg(all(numanager_backend, not(test)))]
        CameraChoice::Numanager(device_index) => {
            match opengel::camera::numanager_backend::open(device_index) {
                Ok(cam) => Ok((cam.info().name.clone(), Box::new(cam))),
                Err(_) => open_mock(),
            }
        }
        #[cfg(all(nokhwa_backend, not(test)))]
        CameraChoice::Nokhwa(device_index) => {
            match opengel::camera::nokhwa_backend::open(device_index) {
                Ok(cam) => Ok((cam.info().name.clone(), Box::new(cam))),
                Err(_) => open_mock(),
            }
        }
        CameraChoice::Mock(device_index) => {
            let cam = mock::open(device_index)?;
            Ok((cam.info().name.clone(), Box::new(cam)))
        }
    }
}

/// Fall back to the synthetic camera when a real device won't open, so the UI
/// stays usable. Only reachable when a real backend is compiled in.
#[cfg(all(camera_backend, not(test)))]
fn open_mock() -> anyhow::Result<(String, Box<dyn Camera>)> {
    let cam = mock::open(0)?;
    Ok((cam.info().name.clone(), Box::new(cam)))
}

/// Capture an exposure bracket from the first camera that opens, returning
/// `(source, frames)`.
pub fn capture_bracket_frames(
    bracket_group: u32,
) -> anyhow::Result<(String, Vec<(DynamicImage, CaptureMeta)>)> {
    for index in 0..list_camera_names().len() {
        let Ok((name, mut cam)) = open_camera_by_index(index) else {
            continue;
        };
        let frames = capture_bracket(cam.as_mut(), &DEFAULT_BRACKET, bracket_group)?;
        return Ok((format!("camera '{name}'"), frames));
    }

    let mut cam = mock::open(0)?;
    let frames = capture_bracket(&mut cam, &DEFAULT_BRACKET, bracket_group)?;
    Ok((cam.info().name.clone(), frames))
}
