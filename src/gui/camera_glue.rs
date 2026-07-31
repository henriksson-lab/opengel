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
fn enumerate() -> Vec<(String, CameraChoice)> {
    let mut out = Vec::new();
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
            Err(e) => eprintln!("nu-manager camera discovery failed: {e}"),
        }
    }
    #[cfg(all(nokhwa_backend, not(test)))]
    {
        use opengel::camera::nokhwa_backend;
        if let Ok(cams) = nokhwa_backend::list_cameras() {
            out.extend(
                cams.into_iter()
                    .map(|c| (c.name, CameraChoice::Nokhwa(c.index))),
            );
        }
    }
    if out.is_empty() {
        out.extend(
            mock::list_cameras()
                .into_iter()
                .map(|c| (c.name, CameraChoice::Mock(c.index))),
        );
    }
    out
}

thread_local! {
    /// The list [`list_camera_names`] last handed out, so [`open_camera_by_index`]
    /// resolves an index against exactly what the caller saw.
    static LISTED: RefCell<Vec<(String, CameraChoice)>> = const { RefCell::new(Vec::new()) };
}

/// Names of the available cameras. Order defines the index used by
/// [`open_camera_by_index`].
pub fn list_camera_names() -> Vec<String> {
    let choices = enumerate();
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
        let choices = enumerate();
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
