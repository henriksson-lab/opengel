//! Camera capture for the app. Uses the real nokhwa backend when built with the
//! `camera` feature and a device is available; otherwise falls back to the mock
//! backend so the UI is fully usable without hardware.

use image::DynamicImage;
use opengel::camera::mock;
use opengel::camera::{capture_bracket, Camera};
use opengel::core::model::CaptureMeta;

/// Default exposure bracket (seconds) for HDR capture.
pub const DEFAULT_BRACKET: [f64; 3] = [0.05, 0.2, 0.8];

/// Names of the available cameras (real devices when built with the camera
/// backend, else the single mock). Order defines the index used by
/// [`open_camera_by_index`].
pub fn list_camera_names() -> Vec<String> {
    #[cfg(all(camera_backend, not(test)))]
    {
        use opengel::camera::nokhwa_backend;
        if let Ok(cams) = nokhwa_backend::list_cameras() {
            if !cams.is_empty() {
                return cams.into_iter().map(|c| c.name).collect();
            }
        }
    }
    mock::list_cameras().into_iter().map(|c| c.name).collect()
}

/// Open the camera at `index` (position in [`list_camera_names`]). Falls back to
/// the mock backend when the real device can't be opened. Returns `(name, handle)`.
pub fn open_camera_by_index(index: usize) -> anyhow::Result<(String, Box<dyn Camera>)> {
    #[cfg(all(camera_backend, not(test)))]
    {
        use opengel::camera::nokhwa_backend;
        if let Ok(cam) = nokhwa_backend::open(index) {
            let name = cam.info().name.clone();
            return Ok((name, Box::new(cam)));
        }
    }
    let cam = mock::open(index)?;
    let name = cam.info().name.clone();
    Ok((name, Box::new(cam)))
}

/// Capture an exposure bracket, returning `(source, frames)`.
pub fn capture_bracket_frames(
    bracket_group: u32,
) -> anyhow::Result<(String, Vec<(DynamicImage, CaptureMeta)>)> {
    #[cfg(all(camera_backend, not(test)))]
    {
        use opengel::camera::nokhwa_backend;
        if let Ok(cams) = nokhwa_backend::list_cameras() {
            if let Some(first) = cams.first() {
                if let Ok(mut cam) = nokhwa_backend::open(first.index) {
                    let frames = capture_bracket(&mut cam, &DEFAULT_BRACKET, bracket_group)?;
                    return Ok((format!("camera '{}'", cam.info().name), frames));
                }
            }
        }
    }

    let mut cam = mock::open(0)?;
    let frames = capture_bracket(&mut cam, &DEFAULT_BRACKET, bracket_group)?;
    Ok((cam.info().name.clone(), frames))
}
