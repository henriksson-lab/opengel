//! Camera capture for the app. Uses the real nokhwa backend when built with the
//! `camera` feature and a device is available; otherwise falls back to the mock
//! backend so the UI is fully usable without hardware.

use opengel::camera::mock;
use opengel::camera::{capture_bracket, Camera};
use opengel::core::model::CaptureMeta;
use image::DynamicImage;

/// Default exposure bracket (seconds) for HDR capture.
pub const DEFAULT_BRACKET: [f64; 3] = [0.05, 0.2, 0.8];

/// Capture an exposure bracket, returning `(source, frames)`.
pub fn capture_bracket_frames(
    bracket_group: u32,
) -> anyhow::Result<(String, Vec<(DynamicImage, CaptureMeta)>)> {
    #[cfg(feature = "camera")]
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
