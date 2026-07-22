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

/// Commands sent from the UI thread to the camera worker.
pub enum CamCommand {
    ListCameras,
    Open(usize),
    SetExposure(f64),
    StartPreview,
    StopPreview,
    CaptureHdr { exposures: Vec<f64>, group: u32 },
    CaptureSingle { exposure: f64 },
    Shutdown,
}

/// Events sent from the camera worker back to the UI thread.
pub enum CamEvent {
    Cameras(Vec<String>),
    Opened { name: String, manual_exposure: bool },
    OpenFailed(String),
    Preview(GrayF32),
    CaptureProgress { done: usize, total: usize },
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
    pub fn capture_single(&self, exposure: f64) {
        self.cancel.store(false, Ordering::SeqCst);
        let _ = self.tx.send(CamCommand::CaptureSingle { exposure });
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

fn worker_main(rx: Receiver<CamCommand>, tx: Sender<CamEvent>, cancel: Arc<AtomicBool>) {
    let mut cam: Option<Box<dyn Camera>> = None;
    let mut previewing = false;

    loop {
        // When previewing we poll for commands and keep grabbing frames; when
        // idle we block so the thread sleeps until the next command.
        let cmd = if previewing && cam.is_some() {
            match rx.try_recv() {
                Ok(c) => Some(c),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => return,
            }
        } else {
            match rx.recv() {
                Ok(c) => Some(c),
                Err(_) => return,
            }
        };

        if let Some(cmd) = cmd {
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
                        let _ = tx.send(CamEvent::Opened { name, manual_exposure });
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
                CamCommand::CaptureSingle { exposure } => {
                    ensure_open(&mut cam, &tx);
                    if let Some(c) = cam.as_deref_mut() {
                        do_capture(c, &[exposure], 0, &tx, &cancel);
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
            if let Some(c) = cam.as_deref_mut() {
                if let Ok(frame) = c.capture() {
                    let _ = tx.send(CamEvent::Preview(GrayF32::from_dynamic(&frame)));
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            } else {
                previewing = false;
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
            let _ = tx.send(CamEvent::Opened { name, manual_exposure });
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
