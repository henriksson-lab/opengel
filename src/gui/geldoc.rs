//! State for the capture tab: the connected enclosure, how to expose, and the
//! run state machine that sequences enclosure and camera.
//!
//! A bare camera takes a picture. An *instrument* does more: the tray decides
//! the light source, the door gates the lamps, faults latch, and a run is a
//! scripted sequence rather than a button that grabs a frame.
//!
//! **An acquisition is one channel.** There is nothing to pick between: the
//! camera is monochrome behind a fixed filter, and only one tray is in the
//! machine at a time. So the software never chooses a light source — it reads
//! which one is there and labels the image with it. With no instrument the image
//! is simply an image, and nothing about a tray is invented for it.

use image::DynamicImage;
use opengel::core::model::{CaptureMeta, ChannelColor};
use opengel::instrument::acquisition::{CapturePlan, EXPOSURE_MAX_S, EXPOSURE_MIN_S};
use opengel::instrument::{Faults, InstrumentInfo, Sense, TrayType};

use crate::camera_worker::AutoExposureMode;
use crate::instrument_worker::InstrumentHandle;

/// What a run is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunGoal {
    /// Take the pictures and make a document out of them.
    Acquire,
    /// Meter each selected channel and write the exposure it settles on back
    /// into the plan. No document is produced — this is the "Auto" button.
    AutoExpose,
}

/// Where a run has got to. The UI reads this to decide what to show and what to
/// allow; the transitions are driven by events from both workers.
#[derive(Debug, Clone, PartialEq)]
pub enum RunPhase {
    Idle,
    /// Stain-free UV activation is running.
    Activating { elapsed_s: f64, total_s: f64 },
    /// Waiting for the lamps to light and stabilise.
    LightingUp,
    /// Lamps are on and the camera is exposing.
    Exposing,
    /// Frames are in hand; waiting for the instrument to confirm the run was
    /// clean before they are adopted.
    Verifying,
}

impl RunPhase {
    pub fn is_running(&self) -> bool {
        !matches!(self, RunPhase::Idle)
    }

    /// Whether the lamps are lit in this phase. Activation is a UV pre-exposure,
    /// so it counts — that is exactly when the user should not open the door.
    pub fn lamps_on(&self) -> bool {
        matches!(
            self,
            RunPhase::Activating { .. } | RunPhase::LightingUp | RunPhase::Exposing
        )
    }

    pub fn label(&self) -> String {
        match self {
            RunPhase::Idle => "Ready.".into(),
            RunPhase::Activating { elapsed_s, total_s } => {
                format!("Activating gel — {elapsed_s:.0} of {total_s:.0} s")
            }
            RunPhase::LightingUp => "Lighting the lamps…".into(),
            RunPhase::Exposing => "Exposing…".into(),
            RunPhase::Verifying => "Checking the run…".into(),
        }
    }
}

/// The run in progress: what it is for, what it is a picture of, and what has
/// been shot.
#[derive(Debug, Default)]
struct Run {
    goal: Option<RunGoal>,
    /// The light source the run is imaging — the tray that was in when it
    /// started. Recorded rather than re-read at the end, so the image is
    /// labelled with what actually lit it even if the tray is pulled the moment
    /// the shutter closes.
    tray: Option<TrayType>,
    /// The frames, held until the instrument confirms the door stayed shut. An
    /// exposure interrupted by an opened door is not data, so it is never
    /// adopted on optimism.
    pending: Vec<(DynamicImage, CaptureMeta)>,
}

/// Everything the Gel Doc EZ tab needs.
pub struct GelDocState {
    /// Handle to the instrument worker. `None` until the GUI starts it.
    pub inst: Option<InstrumentHandle>,
    /// Enclosures the worker found, and which one is selected.
    pub instruments: Vec<String>,
    pub selected_instrument: usize,
    pub connected: bool,
    pub simulated: bool,
    pub info: InstrumentInfo,

    /// Last reading from the instrument.
    pub sense: Option<Sense>,
    pub faults: Faults,
    /// Sense bits with no known meaning, as a mask.
    pub undecoded: u16,
    /// The lamps are lit for framing (outside a run), as last reported by the
    /// worker. Tracked rather than assumed: the instrument can refuse to light
    /// them, and a lamp indicator that shows what we asked for rather than what
    /// happened is worse than none.
    pub lamps_on: bool,

    /// How to expose the one image an acquisition takes.
    pub plan: CapturePlan,

    pub phase: RunPhase,
    run: Run,
    /// The exposure the last capture actually used — the reference for judging
    /// whether the plan's times are in the right region.
    pub last_exposure_s: Option<f64>,
    /// One-line message for the tab's own status area.
    pub message: String,
}

impl Default for GelDocState {
    fn default() -> Self {
        Self::new()
    }
}

impl GelDocState {
    pub fn new() -> Self {
        Self {
            inst: None,
            instruments: Vec::new(),
            selected_instrument: 0,
            connected: false,
            simulated: false,
            info: InstrumentInfo::default(),
            sense: None,
            faults: Faults::NONE,
            undecoded: 0,
            lamps_on: false,
            plan: CapturePlan::new(),
            phase: RunPhase::Idle,
            run: Run::default(),
            last_exposure_s: None,
            message: "Not connected.".into(),
        }
    }

    // ---- the one channel ----

    /// The light source the next image will be taken under, if anything can say.
    ///
    /// This is the whole of "which channel": the tray *is* the light source, so
    /// with an instrument connected the answer is read off the sense line, and
    /// with a bare camera there is no answer to give. Nothing is guessed — an
    /// image labelled "UV" that was not taken under UV is worse than one with no
    /// label at all.
    pub fn channel_tray(&self) -> Option<TrayType> {
        if self.connected {
            self.inserted_tray()
        } else {
            None
        }
    }

    /// What the tab calls the channel on screen — the light source when one is
    /// known, otherwise the camera it comes from.
    pub fn channel_label(&self) -> String {
        match self.channel_tray() {
            Some(tray) => tray.label().to_string(),
            None if self.connected => "No tray".to_string(),
            None => "Camera".to_string(),
        }
    }

    // ---- instrument state ----

    pub fn inserted_tray(&self) -> Option<TrayType> {
        self.sense.and_then(|s| s.tray)
    }

    pub fn door_closed(&self) -> bool {
        self.sense.is_some_and(|s| s.door_closed)
    }

    /// Whether there is a light source to image at all: with an instrument, a
    /// tray in the machine; with a bare camera, whatever the room provides.
    pub fn channel_ready(&self) -> bool {
        !self.connected || self.inserted_tray().is_some()
    }

    /// Whether the gel is lit right now, for framing or by a run.
    pub fn lamps_lit(&self) -> bool {
        self.lamps_on || self.phase.lamps_on()
    }

    /// Why the lamps cannot be switched on for framing, if they cannot. The
    /// instrument enforces the door interlock itself; this is so the button can
    /// explain itself before being pressed.
    pub fn lamp_blocker(&self) -> Option<String> {
        if !self.connected {
            return Some("No instrument connected.".into());
        }
        if self.phase.is_running() {
            return Some("A run is in progress.".into());
        }
        if self.inserted_tray().is_none() {
            return Some("No sample tray is inserted — the tray is the light source.".into());
        }
        if !self.door_closed() {
            return Some("The door is open.".into());
        }
        None
    }

    /// Why a run cannot start, if it cannot. Checked here as well as in the
    /// worker so the Run button can explain itself before being pressed.
    pub fn run_blocker(&self) -> Option<String> {
        if !self.connected {
            return Some("No instrument connected.".into());
        }
        if self.phase.is_running() {
            return Some("A run is already in progress.".into());
        }
        if self.inserted_tray().is_none() {
            return Some("No sample tray is inserted.".into());
        }
        if !self.door_closed() {
            return Some("The door is open.".into());
        }
        if self.faults.no_light_tray() {
            return Some("The lamp assembly is not seated.".into());
        }
        None
    }

    /// "Blue light", for the run status line.
    pub fn run_progress_label(&self) -> String {
        self.run
            .tray
            .map(|tray| tray.label().to_string())
            .unwrap_or_default()
    }

    pub fn abort_run(&mut self, message: impl Into<String>) {
        self.run = Run::default();
        self.phase = RunPhase::Idle;
        self.lamps_on = false;
        self.message = message.into();
        if let Some(inst) = &self.inst {
            inst.abort();
        }
    }
}

impl crate::state::AppState {
    /// Light the tray that is in, or put the lamps out if it cannot be lit.
    ///
    /// One rule: **the lamps burn while you are looking at the preview**. There
    /// is nothing to choose — the tray in the machine is the light source — so
    /// the only question is whether anyone is watching, and stopping the preview
    /// puts them out: 302 nm lamps have a finite life. Called on every tray or
    /// door change and every preview start/stop, so what is burning always
    /// matches what is on screen.
    pub fn geldoc_sync_lamps(&mut self) {
        // Never touch the lamps mid-run: the run owns them, and switching them
        // under an exposure would spoil it.
        if self.geldoc.phase.is_running() {
            return;
        }
        let want = self.live_running && self.geldoc.lamp_blocker().is_none();
        if want == self.geldoc.lamps_on {
            return;
        }
        if let Some(inst) = &self.geldoc.inst {
            inst.illuminate(want);
        }
    }
}

/// What to call an image taken under `tray` in the document, and the colour to
/// draw it in. The one place a tray becomes a channel — with no instrument this
/// is never reached, and the image is left unlabelled rather than given a light
/// source nobody observed.
///
/// Grey for a single-channel gel is what everyone expects, so the tray that is
/// almost always used alone (UV) stays grey; the others get distinct colours, so
/// two acquisitions of one gel can still be told apart in a document that holds
/// both.
pub fn channel_identity(tray: TrayType) -> (String, ChannelColor) {
    let color = match tray {
        TrayType::Uv => ChannelColor::Gray,
        TrayType::White => ChannelColor::Yellow,
        TrayType::Blue => ChannelColor::Cyan,
        TrayType::StainFree => ChannelColor::Magenta,
    };
    (tray.label().to_string(), color)
}

/// The run state machine, spanning the instrument and camera workers.
///
/// These live on [`AppState`] because a run needs both devices: the enclosure
/// handle for the light and the interlocks, the camera handle for the exposure.
impl crate::state::AppState {
    /// Take the picture. Returns the line to show.
    pub fn geldoc_run(&mut self) -> String {
        self.geldoc_start(RunGoal::Acquire)
    }

    /// Meter under the lamps and write what it settles on into the plan.
    pub fn geldoc_auto(&mut self) -> String {
        self.geldoc_start(RunGoal::AutoExpose)
    }

    fn geldoc_start(&mut self, goal: RunGoal) -> String {
        if let Some(blocker) = self.geldoc.run_blocker() {
            self.geldoc.message = blocker.clone();
            return format!("Cannot run: {blocker}");
        }
        // A run images what is in the machine. There is no queue and no tray
        // swap to wait for: `run_blocker` has already established that a tray is
        // in and the door is shut.
        let tray = self.geldoc.inserted_tray();
        self.geldoc.run = Run {
            goal: Some(goal),
            tray,
            pending: Vec::new(),
        };
        let activation_s = self.geldoc.plan.effective_activation_s(tray);
        self.geldoc.phase = if activation_s > 0.0 {
            RunPhase::Activating {
                elapsed_s: 0.0,
                total_s: activation_s,
            }
        } else {
            RunPhase::LightingUp
        };
        self.geldoc.message = self.geldoc.phase.label();
        if let Some(inst) = &self.geldoc.inst {
            inst.begin_run(activation_s);
        }
        let what = match goal {
            RunGoal::Acquire => "Running",
            RunGoal::AutoExpose => "Metering",
        };
        match tray {
            Some(tray) => format!("{what} — {}…", tray.label()),
            None => format!("{what}…"),
        }
    }

    /// The physical Run button: take the picture, exactly as the on-screen
    /// button does. There is nothing else it could sensibly mean.
    pub fn geldoc_button_run(&mut self) -> String {
        if self.geldoc.phase.is_running() {
            return "Run button pressed while a run is in progress — ignored.".into();
        }
        self.geldoc_run()
    }

    /// The lamps are lit and stable: take the picture.
    pub fn geldoc_lights_ready(&mut self) {
        if self.geldoc.run.goal.is_none() {
            self.geldoc.abort_run("No run in progress.");
            return;
        }
        let plan = self.geldoc.plan;
        self.geldoc.phase = RunPhase::Exposing;
        self.geldoc.message = match self.geldoc.run.tray {
            Some(tray) => format!("Exposing {}…", tray.label()),
            None => "Exposing…".into(),
        };
        self.capturing = true;
        self.cancel_requested = false;
        self.capture_status = self.geldoc.message.clone();
        let goal = self.geldoc.run.goal;
        let group = self.next_bracket_group();
        let Some(cam) = &self.cam else {
            self.geldoc.abort_run("No camera available for the exposure.");
            self.capturing = false;
            return;
        };
        match goal {
            // Metering steers the brightest pixels to just below saturation, so
            // what it settles on is the exposure at which nothing clips.
            Some(RunGoal::AutoExpose) => {
                cam.capture_auto(AutoExposureMode::IntenseBands, EXPOSURE_MIN_S, EXPOSURE_MAX_S)
            }
            Some(RunGoal::Acquire) => cam.capture_hdr(plan.exposures(), group),
            None => {
                self.capturing = false;
                self.geldoc.abort_run("No run in progress.");
            }
        }
    }

    /// The camera is done with this channel. Hold the frames and ask the
    /// instrument whether the exposure was clean before they are used.
    pub fn geldoc_capture_done(&mut self, frames: Vec<(DynamicImage, CaptureMeta)>) {
        self.geldoc.last_exposure_s = frames.first().map(|(_, meta)| meta.exposure_seconds);
        self.geldoc.run.pending = frames;
        self.geldoc.phase = RunPhase::Verifying;
        self.geldoc.message = "Checking the run…".into();
        if let Some(inst) = &self.geldoc.inst {
            inst.end_run();
        }
    }

    /// The instrument has reported on the run. Keep the image, or discard it if
    /// the door was opened mid-exposure.
    pub fn geldoc_run_finished(&mut self, faults: Faults, door_violation: bool) -> String {
        self.geldoc.faults = faults;
        self.capturing = false;
        let frames = std::mem::take(&mut self.geldoc.run.pending);
        let tray = self.geldoc.run.tray;
        let goal = self.geldoc.run.goal;
        let what = tray.map(|t| t.label()).unwrap_or("The exposure");

        if door_violation {
            self.geldoc.abort_run("Discarded: the door was opened during the exposure.");
            return "Run discarded — the door was opened during the exposure.".into();
        }
        let Some(goal) = goal else {
            self.geldoc.run = Run::default();
            self.geldoc.phase = RunPhase::Idle;
            return "Run finished.".into();
        };
        if frames.is_empty() {
            self.geldoc.abort_run(format!("{what} produced no image."));
            return format!("Run stopped — {what} produced no image.");
        }

        self.geldoc.run = Run::default();
        self.geldoc.phase = RunPhase::Idle;

        match goal {
            RunGoal::Acquire => {
                // The tray the run recorded is what lit the gel, so it is what
                // names the channel — not whatever tray happens to be in by the
                // time this lands.
                self.adopt_capture_frames(frames, tray);
                // Saturated pixels carry no intensity, so anything measured from
                // them is wrong and the user should see them at a glance.
                let highlight = self.geldoc.plan.highlight_saturated;
                self.set_show_overexposed(highlight);
                self.geldoc.message = format!("Captured {what}.");
                format!("Captured {what}.")
            }
            RunGoal::AutoExpose => {
                // The frame that came back was taken at the exposure the
                // metering settled on; that is the number the plan wants.
                let metered = frames[0].1.exposure_seconds;
                self.geldoc.plan.apply_metered(metered);
                self.apply_live_exposure();
                let summary = self.geldoc.plan.summary();
                self.geldoc.message = format!("Auto set {summary}");
                format!("Auto exposure — {summary}")
            }
        }
    }
}
