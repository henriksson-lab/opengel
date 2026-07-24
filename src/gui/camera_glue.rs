//! Camera capture for the app. Uses real camera backends when built with the
//! `camera` feature and a device is available; otherwise falls back to the mock
//! backend so the UI is fully usable without hardware.

use image::DynamicImage;
use opengel::camera::mock;
use opengel::camera::{capture_bracket, Camera};
use opengel::core::model::CaptureMeta;

/// Default exposure bracket (seconds) for HDR capture.
pub const DEFAULT_BRACKET: [f64; 3] = [0.05, 0.2, 0.8];

#[derive(Debug, Clone, Copy)]
enum CameraChoice {
    #[cfg(all(toupcam_backend, not(test)))]
    Toupcam(usize),
    #[cfg(all(nokhwa_backend, not(test)))]
    Nokhwa(usize),
    Mock(usize),
}

fn camera_choices() -> Vec<(String, CameraChoice)> {
    let mut out = Vec::new();
    #[cfg(all(toupcam_backend, not(test)))]
    {
        use opengel::camera::toupcam_backend;
        if let Ok(cams) = toupcam_backend::list_cameras() {
            out.extend(
                cams.into_iter()
                    .map(|c| (c.name, CameraChoice::Toupcam(c.index))),
            );
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

/// Names of the available cameras (real devices when built with the camera
/// backend, else the single mock). Order defines the index used by
/// [`open_camera_by_index`].
pub fn list_camera_names() -> Vec<String> {
    camera_choices().into_iter().map(|(name, _)| name).collect()
}

/// Open the camera at `index` (position in [`list_camera_names`]). Falls back to
/// the mock backend when the real device can't be opened. Returns `(name, handle)`.
pub fn open_camera_by_index(index: usize) -> anyhow::Result<(String, Box<dyn Camera>)> {
    let choices = camera_choices();
    let choice = choices
        .get(index)
        .map(|(_, choice)| *choice)
        .unwrap_or(CameraChoice::Mock(0));
    match choice {
        #[cfg(all(toupcam_backend, not(test)))]
        CameraChoice::Toupcam(device_index) => {
            if let Ok(cam) = opengel::camera::toupcam_backend::open(device_index) {
                let name = cam.info().name.clone();
                Ok((name, Box::new(cam)))
            } else {
                let cam = mock::open(0)?;
                let name = cam.info().name.clone();
                Ok((name, Box::new(cam)))
            }
        }
        #[cfg(all(nokhwa_backend, not(test)))]
        CameraChoice::Nokhwa(device_index) => {
            if let Ok(cam) = opengel::camera::nokhwa_backend::open(device_index) {
                let name = cam.info().name.clone();
                Ok((name, Box::new(cam)))
            } else {
                let cam = mock::open(0)?;
                let name = cam.info().name.clone();
                Ok((name, Box::new(cam)))
            }
        }
        CameraChoice::Mock(device_index) => {
            let cam = mock::open(device_index)?;
            let name = cam.info().name.clone();
            Ok((name, Box::new(cam)))
        }
    }
}

/// Capture an exposure bracket, returning `(source, frames)`.
pub fn capture_bracket_frames(
    bracket_group: u32,
) -> anyhow::Result<(String, Vec<(DynamicImage, CaptureMeta)>)> {
    for (_, choice) in camera_choices() {
        match choice {
            #[cfg(all(toupcam_backend, not(test)))]
            CameraChoice::Toupcam(device_index) => {
                if let Ok(mut cam) = opengel::camera::toupcam_backend::open(device_index) {
                    let frames = capture_bracket(&mut cam, &DEFAULT_BRACKET, bracket_group)?;
                    return Ok((format!("camera '{}'", cam.info().name), frames));
                }
            }
            #[cfg(all(nokhwa_backend, not(test)))]
            CameraChoice::Nokhwa(device_index) => {
                if let Ok(mut cam) = opengel::camera::nokhwa_backend::open(device_index) {
                    let frames = capture_bracket(&mut cam, &DEFAULT_BRACKET, bracket_group)?;
                    return Ok((format!("camera '{}'", cam.info().name), frames));
                }
            }
            CameraChoice::Mock(device_index) => {
                let mut cam = mock::open(device_index)?;
                let frames = capture_bracket(&mut cam, &DEFAULT_BRACKET, bracket_group)?;
                return Ok((cam.info().name.clone(), frames));
            }
        }
    }

    let mut cam = mock::open(0)?;
    let frames = capture_bracket(&mut cam, &DEFAULT_BRACKET, bracket_group)?;
    Ok((cam.info().name.clone(), frames))
}
