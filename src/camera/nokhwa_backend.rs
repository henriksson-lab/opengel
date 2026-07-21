//! Real USB camera backend via [`nokhwa`] (V4L2 / AVFoundation / MediaFoundation).
//!
//! Compiled only with the `camera` feature. On Linux this needs the
//! V4L development libraries (`libv4l-dev`) at build time.
//!
//! **Exposure caveat.** UVC exposure is device-specific. This backend maps
//! seconds to the V4L "exposure (absolute)" unit of 100 µs, which is the common
//! case but *not* guaranteed across cameras/OSes — hence [`Camera::set_exposure`]
//! reports whether the request was actually honored and callers should read
//! back [`Camera::current_exposure_s`]. This path requires validation on real
//! hardware.

use crate::core::model::CaptureMeta;
use image::DynamicImage;
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{
    ApiBackend, CameraIndex, ControlValueSetter, KnownCameraControl, RequestedFormat,
    RequestedFormatType,
};
use nokhwa::{query, Camera as NokhwaCamera};

use crate::camera::{Camera, CameraError, CameraInfo, Capabilities, Exposure, Result};

/// V4L exposure-absolute unit is 100 microseconds.
const EXPOSURE_UNIT_S: f64 = 100e-6;

fn backend_err(e: impl std::fmt::Display) -> CameraError {
    CameraError::Backend(e.to_string())
}

/// List available cameras via the platform's native backend.
pub fn list_cameras() -> Result<Vec<CameraInfo>> {
    let cams = query(ApiBackend::Auto).map_err(backend_err)?;
    Ok(cams
        .into_iter()
        .map(|c| CameraInfo {
            index: index_to_usize(c.index()),
            name: c.human_name(),
        })
        .collect())
}

fn index_to_usize(idx: &CameraIndex) -> usize {
    match idx {
        CameraIndex::Index(i) => *i as usize,
        // String indices (e.g. macOS unique ids) don't map to a usize; use 0.
        CameraIndex::String(_) => 0,
    }
}

pub struct NokhwaBackend {
    info: CameraInfo,
    camera: NokhwaCamera,
    exposure_s: Option<f64>,
    manual_exposure: bool,
}

/// Open a camera by index at the highest available resolution.
pub fn open(index: usize) -> Result<NokhwaBackend> {
    let cam_index = CameraIndex::Index(index as u32);
    let format = RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestResolution);
    let mut camera = NokhwaCamera::new(cam_index, format).map_err(backend_err)?;
    camera.open_stream().map_err(backend_err)?;

    // Probe manual-exposure support.
    let manual_exposure = camera
        .camera_control(KnownCameraControl::Exposure)
        .is_ok();

    let name = camera.info().human_name();
    Ok(NokhwaBackend {
        info: CameraInfo { index, name },
        camera,
        exposure_s: None,
        manual_exposure,
    })
}

impl Camera for NokhwaBackend {
    fn info(&self) -> &CameraInfo {
        &self.info
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            manual_exposure: self.manual_exposure,
            gain: self.camera.camera_control(KnownCameraControl::Gain).is_ok(),
            exposure_range_s: self
                .camera
                .camera_control(KnownCameraControl::Exposure)
                .ok()
                .and_then(|_| Some((EXPOSURE_UNIT_S, 1.0))),
        }
    }

    fn set_exposure(&mut self, exposure: Exposure) -> Result<bool> {
        match exposure {
            Exposure::Auto => {
                // Best-effort: many V4L devices expose auto mode as a menu
                // control; without a portable handle we report "not honored".
                Ok(false)
            }
            Exposure::Manual(t) => {
                let raw = (t / EXPOSURE_UNIT_S).round().max(1.0) as i64;
                match self.camera.set_camera_control(
                    KnownCameraControl::Exposure,
                    ControlValueSetter::Integer(raw),
                ) {
                    Ok(()) => {
                        self.exposure_s = Some(raw as f64 * EXPOSURE_UNIT_S);
                        Ok(true)
                    }
                    Err(_) => Ok(false),
                }
            }
        }
    }

    fn current_exposure_s(&self) -> Option<f64> {
        self.exposure_s
    }

    fn capture(&mut self) -> Result<DynamicImage> {
        let frame = self.camera.frame().map_err(backend_err)?;
        let decoded = frame
            .decode_image::<RgbFormat>()
            .map_err(|e| CameraError::Capture(e.to_string()))?;
        Ok(DynamicImage::ImageRgb8(decoded))
    }
}

/// Open the first available camera and capture one frame with metadata.
pub fn capture_first() -> Result<(DynamicImage, CaptureMeta)> {
    let cams = list_cameras()?;
    let first = cams.first().ok_or(CameraError::NotFound(0))?;
    let mut cam = open(first.index)?;
    cam.capture_with_meta()
}
