//! # gel-camera
//!
//! Backend-agnostic USB camera capture with best-effort exposure control.
//!
//! Exposure support is uneven across UVC cameras and OS backends, so the API
//! models it as *best-effort*: [`Camera::set_exposure`] returns whether the
//! request was honored, and [`Camera::capabilities`] advertises what a device
//! supports.
//!
//! * [`mock`] — a synthetic backend (always available) used for headless tests,
//!   demos, and developing the UI without hardware. Its exposure scales frame
//!   brightness so exposure bracketing and HDR merge can be exercised.
//! * `nokhwa` backend — real capture (V4L2 / AVFoundation / MediaFoundation).
//! * `toupcam` backend — clean-room userspace USB support for ToupTek/Toupcam
//!   Cypress FX3 cameras.

pub mod mock;
#[cfg(nokhwa_backend)]
pub mod nokhwa_backend;
#[cfg(toupcam_backend)]
pub mod toupcam_backend;

use crate::core::model::CaptureMeta;
use image::DynamicImage;

#[derive(Debug, thiserror::Error)]
pub enum CameraError {
    #[error("no camera at index {0}")]
    NotFound(usize),
    #[error("capture failed: {0}")]
    Capture(String),
    #[error("backend error: {0}")]
    Backend(String),
}

pub type Result<T> = std::result::Result<T, CameraError>;

/// A discovered camera device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraInfo {
    pub index: usize,
    pub name: String,
}

/// Requested exposure setting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Exposure {
    /// Let the camera auto-expose.
    Auto,
    /// Manual exposure in seconds (best-effort; clamped to device range).
    Manual(f64),
}

/// What a device supports.
#[derive(Debug, Clone, Copy, Default)]
pub struct Capabilities {
    pub manual_exposure: bool,
    pub gain: bool,
    /// Inclusive manual-exposure range in seconds, if known.
    pub exposure_range_s: Option<(f64, f64)>,
}

/// A live camera handle.
pub trait Camera {
    fn info(&self) -> &CameraInfo;
    fn capabilities(&self) -> Capabilities;

    /// Request an exposure. Returns `Ok(true)` if honored, `Ok(false)` if the
    /// device ignored it (e.g. manual exposure unsupported).
    fn set_exposure(&mut self, exposure: Exposure) -> Result<bool>;

    /// The exposure currently in effect, in seconds, if known.
    fn current_exposure_s(&self) -> Option<f64>;

    /// Capture a single frame.
    fn capture(&mut self) -> Result<DynamicImage>;

    /// Capture a frame together with populated [`CaptureMeta`].
    fn capture_with_meta(&mut self) -> Result<(DynamicImage, CaptureMeta)> {
        let frame = self.capture()?;
        let meta = CaptureMeta {
            exposure_seconds: self.current_exposure_s().unwrap_or(0.0),
            camera_name: Some(self.info().name.clone()),
            ..Default::default()
        };
        Ok((frame, meta))
    }
}

/// Capture an exposure bracket: set each exposure in turn, capture, and tag all
/// frames with a shared `bracket_group` id for later HDR merge.
pub fn capture_bracket(
    cam: &mut dyn Camera,
    exposures_s: &[f64],
    bracket_group: u32,
) -> Result<Vec<(DynamicImage, CaptureMeta)>> {
    let mut out = Vec::with_capacity(exposures_s.len());
    for &t in exposures_s {
        cam.set_exposure(Exposure::Manual(t))?;
        let frame = cam.capture()?;
        let meta = CaptureMeta {
            exposure_seconds: cam.current_exposure_s().unwrap_or(t),
            camera_name: Some(cam.info().name.clone()),
            bracket_group: Some(bracket_group),
            ..Default::default()
        };
        out.push((frame, meta));
    }
    Ok(out)
}
