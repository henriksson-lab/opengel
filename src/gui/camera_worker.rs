//! A dedicated camera thread so all camera I/O (open, preview, capture) runs off
//! the UI thread — the GUI never blocks on a slow device or a long exposure.
//!
//! The worker owns the [`Camera`] handle (created *inside* the thread, so it
//! never crosses threads — important for backends with thread affinity such as
//! AVFoundation). The UI drives it with [`CamCommand`]s and receives
//! [`CamEvent`]s, which the UI thread drains from a timer. Captures report
//! per-frame progress and honor a shared cancel flag between frames.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;

use image::DynamicImage;
use opengel::camera::{Camera, Exposure};
use opengel::core::model::CaptureMeta;
use opengel::core::GrayF32;

/// How an auto exposure resolves the trade-off between seeing faint bands and
/// not clipping bright ones.
///
/// One mode, deliberately: metering to "nothing clips" is the only setting that
/// leaves every band quantifiable, and the faint end is reached by bracketing up
/// from it (an HDR channel) rather than by over-exposing a single frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoExposureMode {
    /// Expose so that nothing clips: the brightest pixels land just below
    /// saturation, and every band stays quantifiable.
    IntenseBands,
}

/// Commands sent from the UI thread to the camera worker.
pub enum CamCommand {
    ListCameras,
    Open(usize),
    SetExposure(f64),
    StartPreview,
    StopPreview,
    CaptureHdr {
        exposures: Vec<f64>,
        group: u32,
    },
    /// Meter the scene and capture at the exposure it settles on.
    CaptureAuto {
        mode: AutoExposureMode,
        min_s: f64,
        max_s: f64,
    },
    Shutdown,
}

/// Events sent from the camera worker back to the UI thread.
pub enum CamEvent {
    Cameras(Vec<String>),
    Opened {
        name: String,
        manual_exposure: bool,
    },
    OpenFailed(String),
    Preview(GrayF32),
    /// The camera refused to hand over a preview frame. Reported rather than
    /// swallowed: a camera erroring on every grab looks exactly like one staring
    /// at a dark scene, and the difference is the whole diagnosis.
    PreviewFailed(String),
    /// The camera stopped answering altogether and the handle has been dropped —
    /// unplugged, or wedged past recovering. The preview is over; a rescan is
    /// what picks it up again.
    CameraLost(String),
    /// An auto-exposure metering attempt, so the user sees it converging rather
    /// than watching a still dialog through several exposures.
    Metering {
        attempt: usize,
        exposure_s: f64,
    },
    CaptureProgress {
        done: usize,
        total: usize,
    },
    CaptureDone(Vec<(DynamicImage, CaptureMeta)>),
    CaptureFailed(String),
    Cancelled,
}

/// UI-side handle to the camera worker. Cheap to hold; all methods are
/// non-blocking (they only enqueue a command or set the cancel flag).
pub struct CameraHandle {
    tx: Sender<CamCommand>,
    cancel: Arc<AtomicBool>,
    _join: JoinHandle<()>,
}

impl CameraHandle {
    pub fn list_cameras(&self) {
        let _ = self.tx.send(CamCommand::ListCameras);
    }
    pub fn open(&self, index: usize) {
        let _ = self.tx.send(CamCommand::Open(index));
    }
    pub fn set_exposure(&self, t: f64) {
        let _ = self.tx.send(CamCommand::SetExposure(t));
    }
    pub fn start_preview(&self) {
        let _ = self.tx.send(CamCommand::StartPreview);
    }
    pub fn stop_preview(&self) {
        let _ = self.tx.send(CamCommand::StopPreview);
    }
    pub fn capture_hdr(&self, exposures: Vec<f64>, group: u32) {
        self.cancel.store(false, Ordering::SeqCst);
        let _ = self.tx.send(CamCommand::CaptureHdr { exposures, group });
    }
    pub fn capture_auto(&self, mode: AutoExposureMode, min_s: f64, max_s: f64) {
        self.cancel.store(false, Ordering::SeqCst);
        let _ = self.tx.send(CamCommand::CaptureAuto { mode, min_s, max_s });
    }
    /// Request cancellation of an in-progress capture (takes effect between
    /// frames — a single in-flight exposure still completes).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

impl Drop for CameraHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(CamCommand::Shutdown);
    }
}

/// Spawn the camera worker thread. Returns the UI-side handle plus the event
/// receiver the UI drains.
pub fn spawn() -> (CameraHandle, Receiver<CamEvent>) {
    let (cmd_tx, cmd_rx) = channel::<CamCommand>();
    let (evt_tx, evt_rx) = channel::<CamEvent>();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = cancel.clone();
    let join = std::thread::Builder::new()
        .name("camera-worker".into())
        .spawn(move || worker_main(cmd_rx, evt_tx, cancel_worker))
        .expect("spawn camera worker");
    (
        CameraHandle {
            tx: cmd_tx,
            cancel,
            _join: join,
        },
        evt_rx,
    )
}

/// Consecutive failed preview grabs after which the camera is treated as gone
/// rather than as dropping frames.
const PREVIEW_FAILURES_BEFORE_LOST: usize = 5;

fn worker_main(rx: Receiver<CamCommand>, tx: Sender<CamEvent>, cancel: Arc<AtomicBool>) {
    let mut cam: Option<Box<dyn Camera>> = None;
    let mut previewing = false;
    // The last preview error reported, so a camera failing 50 times a second
    // says so once instead of flooding the channel — and says so again if the
    // failure changes.
    let mut preview_error: Option<String> = None;
    // Consecutive failed grabs, reset by the first frame that arrives.
    let mut preview_failures: usize = 0;

    loop {
        // Take everything that is waiting, not one command per iteration. Each
        // iteration also grabs a frame, so a queue drained one command at a time
        // advances at the frame rate — and dragging the exposure slider emits a
        // command per step, which left the camera seconds behind the control
        // that was being dragged.
        //
        // When previewing we poll; when idle we block so the thread sleeps until
        // the next command.
        let mut pending: Vec<CamCommand> = Vec::new();
        if !(previewing && cam.is_some()) {
            match rx.recv() {
                Ok(c) => pending.push(c),
                Err(_) => return,
            }
        }
        loop {
            match rx.try_recv() {
                Ok(c) => pending.push(c),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }

        // Exposure is a level, not an event: within one batch only the last
        // value means anything, and applying each superseded one costs a whole
        // frame. Every other command is kept, in order.
        let latest_exposure = pending
            .iter()
            .rposition(|c| matches!(c, CamCommand::SetExposure(_)));
        for (position, cmd) in pending.into_iter().enumerate() {
            if matches!(cmd, CamCommand::SetExposure(_)) && Some(position) != latest_exposure {
                continue;
            }
            match cmd {
                CamCommand::Shutdown => return,
                CamCommand::ListCameras => {
                    let names = crate::camera_glue::list_camera_names();
                    let _ = tx.send(CamEvent::Cameras(names));
                }
                CamCommand::Open(index) => match crate::camera_glue::open_camera_by_index(index) {
                    Ok((name, c)) => {
                        let manual_exposure = c.capabilities().manual_exposure;
                        cam = Some(c);
                        let _ = tx.send(CamEvent::Opened {
                            name,
                            manual_exposure,
                        });
                    }
                    Err(e) => {
                        let _ = tx.send(CamEvent::OpenFailed(e.to_string()));
                    }
                },
                CamCommand::SetExposure(t) => {
                    if let Some(c) = cam.as_deref_mut() {
                        let _ = c.set_exposure(Exposure::Manual(t));
                    }
                }
                CamCommand::StartPreview => previewing = true,
                CamCommand::StopPreview => previewing = false,
                // Capture runs synchronously here, so no preview frames are
                // grabbed meanwhile; `previewing` is preserved and resumes after.
                CamCommand::CaptureHdr { exposures, group } => {
                    ensure_open(&mut cam, &tx);
                    if let Some(c) = cam.as_deref_mut() {
                        do_capture(c, &exposures, group, &tx, &cancel);
                    } else {
                        let _ = tx.send(CamEvent::CaptureFailed("no camera".into()));
                    }
                }
                CamCommand::CaptureAuto { mode, min_s, max_s } => {
                    ensure_open(&mut cam, &tx);
                    if let Some(c) = cam.as_deref_mut() {
                        do_capture_auto(c, mode, min_s, max_s, &tx, &cancel);
                    } else {
                        let _ = tx.send(CamEvent::CaptureFailed("no camera".into()));
                    }
                }
            }
        }

        // One preview frame per loop iteration while previewing. The short
        // sleep caps the rate (~50 fps) so a fast camera can't spin the thread
        // or flood the event channel; a slow device is unaffected.
        if previewing {
            // The grab is taken out of the borrow before anything is decided, so
            // a camera that has to be dropped can be.
            match cam.as_deref_mut().map(|c| c.capture()) {
                None => previewing = false,
                Some(Ok(frame)) => {
                    preview_error = None;
                    preview_failures = 0;
                    let _ = tx.send(CamEvent::Preview(GrayF32::from_dynamic(&frame)));
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Some(Err(e)) => {
                    // A dropped frame is often transient, so keep trying — but
                    // never in silence: an unreported failure is a black
                    // rectangle the user reads as "the camera sees nothing".
                    let message = e.to_string();
                    preview_failures += 1;
                    if preview_error.as_deref() != Some(message.as_str()) {
                        let _ = tx.send(CamEvent::PreviewFailed(message.clone()));
                        preview_error = Some(message.clone());
                    }
                    // Past this many in a row it is not a dropped frame, it is a
                    // camera that has gone. Let the handle go rather than
                    // spinning on a dead device: the USB interface it holds is
                    // also what a reconnected camera needs to be claimed with.
                    if preview_failures >= PREVIEW_FAILURES_BEFORE_LOST {
                        cam = None;
                        previewing = false;
                        preview_failures = 0;
                        preview_error = None;
                        let _ = tx.send(CamEvent::CameraLost(message));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
        }
    }
}

/// Open the default camera if none is open yet (for a capture with no preview).
fn ensure_open(cam: &mut Option<Box<dyn Camera>>, tx: &Sender<CamEvent>) {
    if cam.is_none() {
        if let Ok((name, c)) = crate::camera_glue::open_camera_by_index(0) {
            let manual_exposure = c.capabilities().manual_exposure;
            *cam = Some(c);
            let _ = tx.send(CamEvent::Opened {
                name,
                manual_exposure,
            });
        }
    }
}

/// How many metering exposures an auto run may take before settling.
const AUTO_MAX_ATTEMPTS: usize = 6;
/// Close enough: stop metering when the statistic is within this fraction of
/// its target. Chasing further costs another full exposure for no visible gain.
const AUTO_TOLERANCE: f64 = 0.12;

impl AutoExposureMode {
    /// The brightness quantile this mode steers, and where it should land.
    ///
    /// Steers the very top of the distribution to just under saturation, so
    /// nothing clips and every band stays quantifiable.
    fn target(self) -> (f64, f64) {
        match self {
            AutoExposureMode::IntenseBands => (0.999, 0.85),
        }
    }
}

/// The value at `quantile` of an image's brightness, in 0..1.
fn brightness_quantile(img: &DynamicImage, quantile: f64) -> f64 {
    let gray = img.to_luma8();
    let mut histogram = [0u32; 256];
    for pixel in gray.pixels() {
        histogram[pixel.0[0] as usize] += 1;
    }
    let total: u64 = histogram.iter().map(|&c| c as u64).sum();
    if total == 0 {
        return 0.0;
    }
    let wanted = (total as f64 * quantile.clamp(0.0, 1.0)).ceil() as u64;
    let mut seen = 0u64;
    for (value, &count) in histogram.iter().enumerate() {
        seen += count as u64;
        if seen >= wanted {
            return value as f64 / 255.0;
        }
    }
    1.0
}

/// Meter the scene, then keep the frame that lands on target.
///
/// Exposure is very nearly linear in time on a scientific sensor, so each
/// attempt scales the time by `target / measured` and converges in two or three
/// frames from almost anywhere. The frame that is *returned* is always one
/// actually taken at the accepted exposure — never a rescaled earlier one — so
/// the image and its recorded exposure time agree, which quantitation depends
/// on.
fn do_capture_auto(
    cam: &mut dyn Camera,
    mode: AutoExposureMode,
    min_s: f64,
    max_s: f64,
    tx: &Sender<CamEvent>,
    cancel: &Arc<AtomicBool>,
) {
    let (quantile, target) = mode.target();
    let (min_s, max_s) = (min_s.min(max_s), max_s.max(min_s));
    // Start from whatever the camera is on, if that is usable; it is usually the
    // preview exposure the user has already framed with, and therefore close.
    let mut exposure = cam
        .current_exposure_s()
        .filter(|t| t.is_finite() && *t > 0.0)
        .unwrap_or(0.1)
        .clamp(min_s, max_s);
    let mut last: Option<(DynamicImage, CaptureMeta)> = None;

    for attempt in 0..AUTO_MAX_ATTEMPTS {
        if cancel.load(Ordering::SeqCst) {
            let _ = tx.send(CamEvent::Cancelled);
            return;
        }
        let _ = tx.send(CamEvent::Metering {
            attempt: attempt + 1,
            exposure_s: exposure,
        });
        let _ = cam.set_exposure(Exposure::Manual(exposure));
        let img = match cam.capture() {
            Ok(img) => img,
            Err(e) => {
                let _ = tx.send(CamEvent::CaptureFailed(e.to_string()));
                return;
            }
        };
        let applied = cam.current_exposure_s().unwrap_or(exposure);
        let measured = brightness_quantile(&img, quantile);
        let meta = CaptureMeta {
            exposure_seconds: applied,
            camera_name: Some(cam.info().name.clone()),
            ..Default::default()
        };
        last = Some((img, meta));

        if (measured - target).abs() <= target * AUTO_TOLERANCE {
            break;
        }
        // A frame with no signal at all carries no scale information — step up
        // hard rather than divide by something near zero.
        let next = if measured <= 1e-4 {
            exposure * 8.0
        } else {
            exposure * (target / measured)
        }
        .clamp(min_s, max_s);
        // Already against a limit and still off target: this camera cannot do
        // better, so keep the frame we have instead of re-shooting it.
        if (next - exposure).abs() <= f64::EPSILON.max(exposure * 1e-3) {
            break;
        }
        exposure = next;
    }

    if cancel.load(Ordering::SeqCst) {
        let _ = tx.send(CamEvent::Cancelled);
        return;
    }
    match last {
        Some(frame) => {
            let _ = tx.send(CamEvent::CaptureProgress { done: 1, total: 1 });
            let _ = tx.send(CamEvent::CaptureDone(vec![frame]));
        }
        None => {
            let _ = tx.send(CamEvent::CaptureFailed(
                "auto exposure took no frames".into(),
            ));
        }
    }
}

/// Shoot each exposure in turn, reporting progress and honoring `cancel`
/// between frames. A single frame (`exposures.len() == 1`) carries no bracket
/// group, so the document builder leaves it un-merged.
fn do_capture(
    cam: &mut dyn Camera,
    exposures: &[f64],
    group: u32,
    tx: &Sender<CamEvent>,
    cancel: &Arc<AtomicBool>,
) {
    let total = exposures.len();
    let mut frames = Vec::with_capacity(total);
    for (i, &t) in exposures.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            let _ = tx.send(CamEvent::Cancelled);
            return;
        }
        let _ = tx.send(CamEvent::CaptureProgress { done: i, total });
        // No-ops on devices without real manual-exposure support (the backend
        // leaves the stream in its native auto mode); "take the picture as-is".
        let _ = cam.set_exposure(Exposure::Manual(t));
        match cam.capture() {
            Ok(img) => {
                let meta = CaptureMeta {
                    exposure_seconds: cam.current_exposure_s().unwrap_or(t),
                    camera_name: Some(cam.info().name.clone()),
                    bracket_group: if total > 1 { Some(group) } else { None },
                    ..Default::default()
                };
                frames.push((img, meta));
            }
            Err(e) => {
                let _ = tx.send(CamEvent::CaptureFailed(e.to_string()));
                return;
            }
        }
    }
    if cancel.load(Ordering::SeqCst) {
        let _ = tx.send(CamEvent::Cancelled);
        return;
    }
    let _ = tx.send(CamEvent::CaptureProgress { done: total, total });
    let _ = tx.send(CamEvent::CaptureDone(frames));
}
