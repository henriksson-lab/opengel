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
    ApiBackend, CameraIndex, ControlValueDescription, ControlValueSetter, KnownCameraControl,
    RequestedFormat, RequestedFormatType,
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
    // Prefer the highest frame rate (typically 720p/1080p) over the absolute
    // highest resolution: a live gel-framing preview needs smooth updates, and
    // decoding 4K frames every tick caps the rate regardless of exposure.
    let format = RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate);
    let mut camera = NokhwaCamera::new(cam_index, format).map_err(backend_err)?;
    camera.open_stream().map_err(backend_err)?;

    // Probe whether the device can actually do *manual* exposure (not just
    // expose an exposure control that is auto-only).
    let manual_exposure = supports_manual_exposure(&camera);

    let name = camera.info().human_name();
    Ok(NokhwaBackend {
        info: CameraInfo { index, name },
        camera,
        exposure_s: None,
        manual_exposure,
    })
}

/// Whether the device supports manual exposure. On macOS this means the
/// AVFoundation "custom" exposure mode (3) is available *and* there is a usable
/// exposure-duration range; elsewhere it means the exposure control reports a
/// real (non-degenerate) range.
#[cfg(target_os = "macos")]
fn supports_manual_exposure(camera: &NokhwaCamera) -> bool {
    let custom_mode = matches!(
        camera.camera_control(KnownCameraControl::Exposure).as_ref().map(|c| c.description()),
        Ok(ControlValueDescription::Enum { possible, .. }) if possible.contains(&3)
    );
    let duration_range = matches!(
        camera.camera_control(KnownCameraControl::Gamma).as_ref().map(|c| c.description()),
        Ok(ControlValueDescription::IntegerRange { min, max, .. }) if max > min
    );
    custom_mode && duration_range
}

#[cfg(not(target_os = "macos"))]
fn supports_manual_exposure(camera: &NokhwaCamera) -> bool {
    matches!(
        camera.camera_control(KnownCameraControl::Exposure).as_ref().map(|c| c.description()),
        Ok(ControlValueDescription::IntegerRange { min, max, .. }) if max > min
    )
}

impl NokhwaBackend {
    /// Set a manual exposure time (seconds). Returns whether the device honored
    /// it. Exposure control is mapped very differently per platform.
    ///
    /// NOTE: on macOS this is best-effort and known-incomplete — nokhwa's
    /// AVFoundation mapping puts exposure MODE on `Exposure` (3 = custom) and
    /// DURATION on `Gamma` (a CMTime value; timescale assumed nanoseconds here).
    /// Many built-in cameras (e.g. the FaceTime HD) only report modes [0, 2]
    /// (locked / continuous-auto) and no duration range, so this returns
    /// `Ok(false)`. Proper manual exposure on macOS is deferred (likely a
    /// different capture library or a camera that supports custom exposure).
    #[cfg(target_os = "macos")]
    fn set_manual_exposure(&mut self, t: f64) -> Result<bool> {
        if self
            .camera
            .set_camera_control(
                KnownCameraControl::Exposure,
                ControlValueSetter::EnumValue(3),
            )
            .is_err()
        {
            return Ok(false); // custom mode unsupported → auto only
        }
        let Ok(ctrl) = self.camera.camera_control(KnownCameraControl::Gamma) else {
            return Ok(false);
        };
        let (min, max) = match ctrl.description() {
            ControlValueDescription::IntegerRange { min, max, .. } if *max > *min => (*min, *max),
            _ => return Ok(false), // no usable duration range
        };
        let raw = ((t * 1_000_000_000.0) as i64).clamp(min, max);
        match self
            .camera
            .set_camera_control(KnownCameraControl::Gamma, ControlValueSetter::Integer(raw))
        {
            Ok(()) => {
                self.exposure_s = Some(raw as f64 / 1_000_000_000.0);
                Ok(true)
            }
            Err(_) => Ok(false),
        }
    }

    /// V4L / MSMF: the `Exposure` control is the absolute exposure time.
    #[cfg(not(target_os = "macos"))]
    fn set_manual_exposure(&mut self, t: f64) -> Result<bool> {
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
        // If the device has no real manual-exposure support, do not touch the
        // exposure controls at all. On macoOS especially, poking the AVFoundation
        // exposure mode/duration on a camera that only advertises auto modes
        // (e.g. the built-in FaceTime HD) stalls the capture stream — which froze
        // the live preview and made captures hang. Leave it in its native auto.
        if !self.manual_exposure {
            return Ok(false);
        }
        match exposure {
            Exposure::Auto => {
                #[cfg(target_os = "macos")]
                {
                    // AVFoundation: exposure MODE lives on the `Exposure` control
                    // (2 = continuous auto).
                    let _ = self.camera.set_camera_control(
                        KnownCameraControl::Exposure,
                        ControlValueSetter::EnumValue(2),
                    );
                }
                Ok(false)
            }
            Exposure::Manual(t) => self.set_manual_exposure(t),
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
