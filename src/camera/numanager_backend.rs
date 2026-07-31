//! Cameras provided as **nu-manager devices**.
//!
//! [nu-manager](https://github.com/henriksson-lab/numanager) is the device
//! layer: it owns the hardware protocols and presents each instrument as a
//! typed device with properties (`exposure`, `gain`, …) and capabilities
//! (`CameraCapture`). This module adapts one such device to OpenGel's
//! [`Camera`] trait, so the rest of the app keeps talking to a plain
//! `capture()` / `set_exposure()` handle and knows nothing about USB.
//!
//! Discovery goes through nu-manager's USB VID/PID autodiscovery, so OpenGel
//! names no driver: whatever camera nu-manager grows support for shows up here.
//! The bench camera arrives through its Toupcam/ToupTek driver, itself derived
//! from OpenGel's original clean-room backend.

use std::cell::RefCell;
use std::time::Duration;

use image::{DynamicImage, GrayImage, RgbImage};
use numanager_core::config::HardwareConfig;
use numanager_core::runtime::{DiscoveryRegistry, DriverCandidate, LocalRuntime, Runtime};
use numanager_core::{
    CameraCaptureRequest, CapabilityKind, CapturedFrame, Command, DeviceDescriptor, Frame,
    ImageEncoding, PropertySchema, TimeInterval, Value,
};
use numanager_drivers::register_builtin_discovery;

use crate::camera::{Camera, CameraError, CameraInfo, Capabilities, Exposure, Result};

/// Budget for a frame. Generous: a long exposure on a large sensor is slow, and
/// the caller runs on the camera worker thread, never the UI thread.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(20);
/// Budget for a control transfer (property read/write).
const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);

fn backend_err(e: impl std::fmt::Display) -> CameraError {
    CameraError::Backend(e.to_string())
}

/// Every camera nu-manager finds on this machine, as claimable candidates.
///
/// This is nu-manager's own builtin discovery over every driver it has — USB
/// VID/PID matching, plus V4L2 on Linux — so OpenGel names no driver and picks
/// up new camera support for free. Non-camera instruments (stages, IO boards)
/// can come back too; those are filtered out by device kind.
///
/// The hardware config is empty: OpenGel has no instrument config file, so only
/// the automatic probes contribute. (Point this at a `HardwareConfig::load`ed
/// file to also support cameras that must be described rather than discovered.)
///
/// **This is not a cheap probe.** nu-manager's two-stage discovery builds each
/// candidate around an *opened* driver — the Toupcam driver's open claims the
/// USB interface and replays the init sequence. A candidate that is listed and
/// then dropped has therefore cost a full open/close cycle, which is why
/// [`list_cameras`] caches what it detected for the following [`open`].
fn detect() -> Result<Vec<DriverCandidate>> {
    let config = HardwareConfig::default();
    let mut registry = DiscoveryRegistry::new();
    register_builtin_discovery(&mut registry, &config).map_err(backend_err)?;
    let candidates = registry.detect_all().map_err(backend_err)?;
    Ok(candidates.into_iter().filter(is_camera).collect())
}

/// Whether a candidate offers a camera, by the device's kind tags.
fn is_camera(candidate: &DriverCandidate) -> bool {
    candidate
        .devices()
        .iter()
        .any(|device| device.kinds.iter().any(|kind| kind == "camera"))
}

thread_local! {
    /// Candidates from the last [`list_cameras`], waiting to be claimed by
    /// [`open`]. Thread-local because the camera worker owns its devices on one
    /// thread and `DriverCandidate` is not `Send`.
    static DETECTED: RefCell<Vec<DriverCandidate>> = const { RefCell::new(Vec::new()) };
}

/// List the cameras nu-manager exposes. Order defines the index [`open`] takes.
pub fn list_cameras() -> Result<Vec<CameraInfo>> {
    let candidates = detect()?;
    let infos = candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| CameraInfo {
            index,
            name: candidate.label().to_string(),
        })
        .collect();
    DETECTED.with(|cell| *cell.borrow_mut() = candidates);
    Ok(infos)
}

/// Take candidate `index` out of the [`list_cameras`] cache, dropping the rest.
///
/// The others are dropped rather than kept because their devices are *open*
/// (see [`detect`]): holding them would keep a USB interface claimed for a
/// camera nobody selected.
fn take_detected(index: usize) -> Option<DriverCandidate> {
    DETECTED.with(|cell| {
        let mut cell = cell.borrow_mut();
        if index >= cell.len() {
            // Leave the cache intact: an index we don't have says nothing about
            // the candidates we do have.
            return None;
        }
        let mut detected = std::mem::take(&mut *cell);
        Some(detected.swap_remove(index))
    })
}

/// Open the camera at `index` (position in [`list_cameras`]).
pub fn open(index: usize) -> Result<NumanagerCamera> {
    let candidate = match take_detected(index) {
        Some(candidate) => candidate,
        None => {
            let mut candidates = detect()?;
            if index >= candidates.len() {
                return Err(CameraError::NotFound(index));
            }
            candidates.swap_remove(index)
        }
    };
    NumanagerCamera::claim(index, candidate)
}

/// A nu-manager camera device driven through its own [`LocalRuntime`].
///
/// One runtime per camera keeps this adapter self-contained and matches how
/// OpenGel uses cameras (open one, shoot a bracket, close). An app driving a
/// whole instrument would instead hold one runtime and add every driver to it.
pub struct NumanagerCamera {
    info: CameraInfo,
    runtime: LocalRuntime,
    device: DeviceDescriptor,
    exposure_s: Option<f64>,
}

impl NumanagerCamera {
    fn claim(index: usize, candidate: DriverCandidate) -> Result<Self> {
        let name = candidate.label().to_string();
        let mut runtime = LocalRuntime::new();
        runtime.add_candidate(candidate).map_err(backend_err)?;
        let device = runtime
            .device_by_capability(CapabilityKind::CameraCapture)
            .map_err(backend_err)?
            .clone();
        let exposure_s = read_exposure_s(&runtime, &device);
        Ok(Self {
            info: CameraInfo { index, name },
            runtime,
            device,
            exposure_s,
        })
    }

    fn property(&self, key: &str) -> Option<&PropertySchema> {
        self.device
            .properties
            .iter()
            .find(|property| property.key == key)
    }
}

/// The exposure the device reports, in seconds — the value actually in effect,
/// which may differ from what was requested (the driver quantizes it to sensor
/// lines). `None` if the device doesn't report one.
fn read_exposure_s(runtime: &LocalRuntime, device: &DeviceDescriptor) -> Option<f64> {
    let value = runtime
        .execute(
            Command::read_property(device, "exposure"),
            CONTROL_TIMEOUT,
        )
        .ok()?;
    time_seconds(&value)
}

fn time_seconds(value: &Value) -> Option<f64> {
    match value {
        Value::TimeInterval(t) => Some(t.seconds()),
        _ => None,
    }
}

/// The writable exposure range in seconds, from the device's property schema.
fn exposure_range_s(schema: &PropertySchema) -> Option<(f64, f64)> {
    let range = schema.range.as_ref()?;
    Some((time_seconds(&range.min)?, time_seconds(&range.max)?))
}

impl Camera for NumanagerCamera {
    fn info(&self) -> &CameraInfo {
        &self.info
    }

    fn capabilities(&self) -> Capabilities {
        let exposure = self.property("exposure");
        Capabilities {
            manual_exposure: exposure.is_some_and(|schema| schema.writable),
            gain: self.property("gain").is_some_and(|schema| schema.writable),
            exposure_range_s: exposure.and_then(exposure_range_s),
        }
    }

    fn set_exposure(&mut self, exposure: Exposure) -> Result<bool> {
        // Scientific cameras are manual-exposure devices; nu-manager models
        // exposure as a plain typed property with no auto mode to hand back to.
        let Exposure::Manual(seconds) = exposure else {
            return Ok(false);
        };
        let Some(schema) = self.property("exposure").filter(|schema| schema.writable) else {
            return Ok(false);
        };
        // Clamp rather than let the driver reject an out-of-range write: an
        // exposure bracket is a request, and shooting the nearest achievable
        // time is more useful than failing the capture.
        let seconds = match exposure_range_s(schema) {
            Some((min, max)) => seconds.clamp(min, max),
            None => seconds,
        };
        self.runtime
            .execute(
                Command::write_property(
                    &self.device,
                    "exposure",
                    Value::TimeInterval(TimeInterval::from_seconds(seconds)),
                ),
                CONTROL_TIMEOUT,
            )
            .map_err(backend_err)?;
        self.exposure_s = read_exposure_s(&self.runtime, &self.device).or(Some(seconds));
        Ok(true)
    }

    fn current_exposure_s(&self) -> Option<f64> {
        self.exposure_s
    }

    fn capture(&mut self) -> Result<DynamicImage> {
        // Mono8: gel imaging is monochrome, and for a Bayer sensor this hands
        // back the raw mosaic unconverted — the same bytes the old in-tree
        // Toupcam backend produced.
        let operation = self
            .runtime
            .submit_request(
                &self.device,
                CameraCaptureRequest {
                    encoding: Some(ImageEncoding::Mono8),
                    buffer: None,
                },
            )
            .map_err(backend_err)?;
        let completion = self
            .runtime
            .wait_completed(operation.id, CAPTURE_TIMEOUT)
            .map_err(backend_err)?;
        let captured = CapturedFrame::from_completion(&completion).map_err(backend_err)?;
        let frame = self
            .runtime
            .frame(captured.handle)
            .map_err(backend_err)?
            .ok_or_else(|| {
                CameraError::Capture("camera reported a frame the runtime did not buffer".into())
            })?;
        frame_to_image(&frame)
    }
}

/// Convert a nu-manager frame into an [`image`] buffer.
fn frame_to_image(frame: &Frame) -> Result<DynamicImage> {
    let (width, height) = (frame.width, frame.height);
    let size_err = || {
        CameraError::Capture(format!(
            "{} bytes do not fill a {width}x{height} {} frame",
            frame.data.len(),
            frame.pixel_format
        ))
    };
    match frame.pixel_format.as_str() {
        // Raw8 is a Bayer mosaic; we read it as gray on purpose (see `capture`).
        "Mono8" | "Raw8" | "Native" => GrayImage::from_raw(width, height, frame.data.clone())
            .map(DynamicImage::ImageLuma8)
            .ok_or_else(size_err),
        "Rgb8" => RgbImage::from_raw(width, height, frame.data.clone())
            .map(DynamicImage::ImageRgb8)
            .ok_or_else(size_err),
        "Bgr8" => {
            let mut data = frame.data.clone();
            for pixel in data.chunks_exact_mut(3) {
                pixel.swap(0, 2);
            }
            RgbImage::from_raw(width, height, data)
                .map(DynamicImage::ImageRgb8)
                .ok_or_else(size_err)
        }
        other => Err(CameraError::Capture(format!(
            "unsupported nu-manager pixel format {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use numanager_core::DriverId;
    use numanager_drivers::toupcam::ToupcamDriver;

    /// The adapter driving nu-manager's *simulated* Toupcam device — the same
    /// code path a real camera takes (claim → properties → capture), minus the
    /// USB transport, so the wiring is testable without the bench camera.
    fn simulated_camera() -> NumanagerCamera {
        let candidate = DriverCandidate::from_driver(
            "Simulated Toupcam camera",
            Box::new(ToupcamDriver::simulated(DriverId(1))),
        );
        NumanagerCamera::claim(0, candidate).expect("claim the simulated camera")
    }

    #[test]
    fn capabilities_come_from_the_device_property_schema() {
        let caps = simulated_camera().capabilities();
        assert!(caps.manual_exposure, "exposure is a writable property");
        assert!(caps.gain, "gain is a writable property");
        let (min, max) = caps.exposure_range_s.expect("device advertises a range");
        assert!(min > 0.0 && min < max, "sane range, got {min}..{max}");
    }

    #[test]
    fn set_exposure_clamps_to_the_device_range_and_reads_back() {
        let mut cam = simulated_camera();
        let (min, max) = cam.capabilities().exposure_range_s.expect("range");

        assert!(cam.set_exposure(Exposure::Manual(0.2)).expect("in range"));
        let applied = cam.current_exposure_s().expect("device reports exposure");
        assert!((applied - 0.2).abs() < 1e-6, "got {applied}");

        // Beyond what the sensor can do: shoot the nearest achievable time
        // rather than fail the capture.
        assert!(cam.set_exposure(Exposure::Manual(max * 10.0)).expect("clamped"));
        assert!((cam.current_exposure_s().expect("exposure") - max).abs() < 1e-6);
        assert!(cam.set_exposure(Exposure::Manual(min / 10.0)).expect("clamped"));
        assert!((cam.current_exposure_s().expect("exposure") - min).abs() < 1e-6);
    }

    #[test]
    fn capture_returns_a_gray_frame_of_the_device_geometry() {
        let mut cam = simulated_camera();
        cam.set_exposure(Exposure::Manual(0.2)).expect("set exposure");
        let frame = cam.capture().expect("capture a frame");
        let gray = frame.as_luma8().expect("Mono8 arrives as a gray image");
        assert_eq!((gray.width(), gray.height()), (640, 480));
        assert!(
            gray.pixels().any(|p| p.0[0] > 0),
            "the simulated gel scene is not blank"
        );
    }

    #[test]
    fn brighter_exposure_yields_more_signal() {
        // Proves the exposure write actually reached the device rather than
        // just being recorded locally.
        let mut cam = simulated_camera();
        let mean = |cam: &mut NumanagerCamera, t: f64| {
            cam.set_exposure(Exposure::Manual(t)).expect("set exposure");
            let frame = cam.capture().expect("capture");
            let gray = frame.to_luma8();
            gray.pixels().map(|p| p.0[0] as f64).sum::<f64>() / gray.pixels().len() as f64
        };
        let dim = mean(&mut cam, 0.05);
        let bright = mean(&mut cam, 0.5);
        assert!(bright > dim, "expected {bright} > {dim}");
    }
}
