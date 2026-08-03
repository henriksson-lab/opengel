//! State for the Gel Doc EZ tab: the connected enclosure, the acquisition plan,
//! and the run state machine that sequences enclosure and camera.
//!
//! The Live tab drives a bare camera. This tab drives an *instrument*: the tray
//! decides the light source, the door gates the lamps, faults latch, and a run
//! is a scripted sequence rather than a button that grabs a frame. That is why
//! it is a separate tab rather than more controls on the existing one.
//!
//! A run walks the plan's selected channels in tray order. Because only one tray
//! is inserted at a time, a multi-channel run parks in
//! [`RunPhase::WaitingForTray`] between channels until the user has swapped the
//! tray and shut the door — the instrument cannot do it, so the run waits rather
//! than pretending.

use image::DynamicImage;
use opengel::core::model::{CaptureMeta, ChannelColor};
use opengel::core::CapturedChannel;
use opengel::instrument::acquisition::{
    AcquisitionPlan, CaptureMode, ChannelPlan, EXPOSURE_MAX_S, EXPOSURE_MIN_S,
};
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
    /// The next channel needs a different tray than the one inserted. The run
    /// resumes by itself once the sense line agrees.
    WaitingForTray { tray: TrayType },
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
            RunPhase::WaitingForTray { tray } => {
                format!("Insert the {} tray and close the door…", tray.label())
            }
            RunPhase::Activating { elapsed_s, total_s } => {
                format!("Activating gel — {elapsed_s:.0} of {total_s:.0} s")
            }
            RunPhase::LightingUp => "Lighting the lamps…".into(),
            RunPhase::Exposing => "Exposing…".into(),
            RunPhase::Verifying => "Checking the run…".into(),
        }
    }
}

/// The run in progress: which channels, how far along, and what has been shot.
#[derive(Debug, Default)]
struct Run {
    goal: Option<RunGoal>,
    /// The channels to visit, in tray order.
    queue: Vec<TrayType>,
    /// Index into `queue` of the channel being shot.
    at: usize,
    /// Frames captured so far, one entry per finished channel. Held until the
    /// whole run is over, because a multi-channel acquisition is one document.
    captured: Vec<(TrayType, Vec<(DynamicImage, CaptureMeta)>)>,
    /// The current channel's frames, held until the instrument confirms the door
    /// stayed shut. An exposure interrupted by an opened door is not data, so it
    /// is never adopted on optimism.
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
    /// The tray as of the previous reading, so a *change* can be told from a
    /// repeat: swapping the tray selects the channel it lights, but only when it
    /// actually moves — otherwise every poll would drag the selection back and
    /// the user could never look at another channel's settings.
    last_tray: Option<Option<TrayType>>,
    pub faults: Faults,
    /// Sense bits with no known meaning, as a mask.
    pub undecoded: u16,
    /// The lamps are lit for framing (outside a run), as last reported by the
    /// worker. Tracked rather than assumed: the instrument can refuse to light
    /// them, and a lamp indicator that shows what we asked for rather than what
    /// happened is worse than none.
    pub lamps_on: bool,

    /// What to shoot: one image per selected channel.
    pub plan: AcquisitionPlan,
    /// The channel whose settings are being edited, and which the live view is
    /// framing. Not necessarily one that is selected for the run.
    pub current_channel: TrayType,

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
            last_tray: None,
            faults: Faults::NONE,
            undecoded: 0,
            lamps_on: false,
            plan: AcquisitionPlan::new(),
            current_channel: TrayType::Uv,
            phase: RunPhase::Idle,
            run: Run::default(),
            last_exposure_s: None,
            message: "Not connected.".into(),
        }
    }

    // ---- plan editing ----

    /// The channel whose settings the tab is showing.
    pub fn channel(&self) -> &ChannelPlan {
        self.plan.get(self.current_channel)
    }

    pub fn channel_mut(&mut self) -> &mut ChannelPlan {
        let tray = self.current_channel;
        self.plan.get_mut(tray)
    }

    pub fn select_channel(&mut self, index: usize) {
        self.current_channel = AcquisitionPlan::tray_at(index);
    }

    pub fn current_channel_index(&self) -> usize {
        AcquisitionPlan::index_of(self.current_channel)
    }

    pub fn set_channel_selected(&mut self, index: usize, selected: bool) {
        let tray = AcquisitionPlan::tray_at(index);
        self.plan.get_mut(tray).selected = selected;
    }

    pub fn set_capture_mode(&mut self, mode: CaptureMode) {
        self.channel_mut().mode = mode;
    }

    pub fn set_exposure_s(&mut self, seconds: f64) {
        self.channel_mut().exposure_s = seconds.clamp(EXPOSURE_MIN_S, EXPOSURE_MAX_S);
    }

    /// Adopt the current exposure as this channel's lower / upper HDR bound.
    pub fn set_hdr_min_s(&mut self, seconds: f64) {
        let c = self.channel_mut();
        c.hdr_min_s = seconds.clamp(EXPOSURE_MIN_S, c.hdr_max_s);
    }
    pub fn set_hdr_max_s(&mut self, seconds: f64) {
        let c = self.channel_mut();
        c.hdr_max_s = seconds.clamp(c.hdr_min_s, EXPOSURE_MAX_S);
    }
    pub fn set_hdr_steps(&mut self, steps: usize) {
        self.channel_mut().hdr_steps = steps.max(2);
    }

    pub fn set_activation_s(&mut self, seconds: f64) {
        self.channel_mut().activation_s = seconds.max(0.0);
    }

    // ---- instrument state ----

    pub fn inserted_tray(&self) -> Option<TrayType> {
        self.sense.and_then(|s| s.tray)
    }

    pub fn door_closed(&self) -> bool {
        self.sense.is_some_and(|s| s.door_closed)
    }

    /// Whether the inserted tray is the channel being edited. Imaging on the
    /// wrong light source gives a blank gel with nothing obviously wrong, so the
    /// tab says which tray the current channel wants.
    pub fn current_channel_ready(&self) -> bool {
        self.inserted_tray() == Some(self.current_channel)
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
        if self.plan.selected().is_empty() {
            return Some("No channels selected.".into());
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

    /// The channel a run is currently on, if one is running.
    pub fn running_channel(&self) -> Option<TrayType> {
        self.run.queue.get(self.run.at).copied()
    }

    /// "Channel 2 of 3 — Blue light", for the run status line.
    pub fn run_progress_label(&self) -> String {
        match self.running_channel() {
            Some(tray) if self.run.queue.len() > 1 => format!(
                "Channel {} of {} — {}",
                self.run.at + 1,
                self.run.queue.len(),
                tray.label()
            ),
            Some(tray) => tray.label().to_string(),
            None => String::new(),
        }
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
    /// Light the selected channel, or put the lamps out if it cannot be lit.
    ///
    /// Selecting a channel *is* the light switch: there is nothing else the
    /// selection could mean on an instrument whose light source is the tray, and
    /// a separate lamp button would only be a second place to say the same
    /// thing. Called on every selection and on every tray/door change, so what
    /// is burning always matches what is selected.
    pub fn geldoc_sync_lamps(&mut self) {
        // Never touch the lamps mid-run: the run owns them, and switching them
        // under an exposure would spoil it.
        if self.geldoc.phase.is_running() {
            return;
        }
        let want = self.geldoc.lamp_blocker().is_none() && self.geldoc.current_channel_ready();
        if want == self.geldoc.lamps_on {
            return;
        }
        if let Some(inst) = &self.geldoc.inst {
            inst.illuminate(want);
        }
    }
}

/// The document channel colour for a light source.
///
/// Grey for a single-channel gel is what everyone expects, so the tray that is
/// almost always used alone (UV) stays grey; the others get distinct colours so
/// a multi-channel document can be told apart at a glance.
fn channel_color(tray: TrayType) -> ChannelColor {
    match tray {
        TrayType::Uv => ChannelColor::Gray,
        TrayType::White => ChannelColor::Yellow,
        TrayType::Blue => ChannelColor::Cyan,
        TrayType::StainFree => ChannelColor::Magenta,
    }
}

/// The run state machine, spanning the instrument and camera workers.
///
/// These live on [`AppState`] because a run needs both devices: the enclosure
/// handle for the light and the interlocks, the camera handle for the exposure.
impl crate::state::AppState {
    /// Start a run over the plan's selected channels. Returns the line to show.
    pub fn geldoc_run(&mut self) -> String {
        self.geldoc_start(RunGoal::Acquire)
    }

    /// Meter every selected channel and write the result into the plan.
    pub fn geldoc_auto(&mut self) -> String {
        self.geldoc_start(RunGoal::AutoExpose)
    }

    fn geldoc_start(&mut self, goal: RunGoal) -> String {
        if let Some(blocker) = self.geldoc.run_blocker() {
            self.geldoc.message = blocker.clone();
            return format!("Cannot run: {blocker}");
        }
        let queue = self.geldoc.plan.selected();
        // Start on whichever selected channel is already under the inserted
        // tray, so the common single-tray case never asks for a swap it does not
        // need.
        let inserted = self.geldoc.inserted_tray();
        let mut queue = queue;
        if let Some(pos) = queue.iter().position(|t| Some(*t) == inserted) {
            queue.rotate_left(pos);
        }
        let count = queue.len();
        self.geldoc.run = Run {
            goal: Some(goal),
            queue,
            at: 0,
            captured: Vec::new(),
            pending: Vec::new(),
        };
        let what = match goal {
            RunGoal::Acquire => "Running",
            RunGoal::AutoExpose => "Metering",
        };
        self.geldoc_begin_channel();
        format!(
            "{what} {count} channel{}…",
            if count == 1 { "" } else { "s" }
        )
    }

    /// Start (or park before) the channel at the head of the queue.
    fn geldoc_begin_channel(&mut self) {
        let Some(tray) = self.geldoc.running_channel() else {
            self.geldoc_finish_run();
            return;
        };
        // Follow the run with the live view and the settings panel, so what is
        // on screen is what is being shot.
        self.geldoc.current_channel = tray;

        if self.geldoc.inserted_tray() != Some(tray) || !self.geldoc.door_closed() {
            self.geldoc.phase = RunPhase::WaitingForTray { tray };
            self.geldoc.message = self.geldoc.phase.label();
            return;
        }
        let activation_s = self.geldoc.plan.get(tray).effective_activation_s();
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
    }

    /// A fresh sense reading arrived. If a run is parked waiting for a tray
    /// swap, this is what releases it; otherwise a tray change selects the
    /// channel that tray lights.
    pub fn geldoc_sense_changed(&mut self) {
        if let RunPhase::WaitingForTray { tray } = self.geldoc.phase {
            if self.geldoc.inserted_tray() == Some(tray) && self.geldoc.door_closed() {
                self.geldoc_begin_channel();
            }
            return;
        }
        let tray = self.geldoc.inserted_tray();
        if self.geldoc.last_tray != Some(tray) {
            self.geldoc.last_tray = Some(tray);
            // Swapping the tray *is* choosing the channel — it is the light
            // source, and the user has just picked it up and put it in.
            if let Some(tray) = tray {
                if !self.geldoc.phase.is_running() {
                    self.geldoc.current_channel = tray;
                    self.apply_live_exposure();
                }
            }
        }
    }

    /// The physical Run button: run the plan, exactly as the on-screen button
    /// does. There is nothing else it could sensibly mean now that a run is the
    /// plan rather than one of several saved recipes.
    pub fn geldoc_button_run(&mut self) -> String {
        if self.geldoc.phase.is_running() {
            return "Run button pressed while a run is in progress — ignored.".into();
        }
        self.geldoc_run()
    }

    /// The lamps are lit and stable: take the picture.
    pub fn geldoc_lights_ready(&mut self) {
        let Some(tray) = self.geldoc.running_channel() else {
            self.geldoc.abort_run("No channel to expose.");
            return;
        };
        let channel = *self.geldoc.plan.get(tray);
        self.geldoc.phase = RunPhase::Exposing;
        self.geldoc.message = format!("Exposing {}…", tray.label());
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
            Some(RunGoal::Acquire) => cam.capture_hdr(channel.exposures(), group),
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

    /// The instrument has reported on the finished channel. Keep it, or discard
    /// the whole run if the door was opened mid-exposure.
    pub fn geldoc_run_finished(&mut self, faults: Faults, door_violation: bool) -> String {
        self.geldoc.faults = faults;
        self.capturing = false;
        let frames = std::mem::take(&mut self.geldoc.run.pending);
        let tray = self.geldoc.running_channel();

        if door_violation {
            self.geldoc.abort_run("Discarded: the door was opened during the exposure.");
            return "Run discarded — the door was opened during the exposure.".into();
        }
        let (Some(tray), Some(goal)) = (tray, self.geldoc.run.goal) else {
            self.geldoc.phase = RunPhase::Idle;
            return "Run finished.".into();
        };
        if frames.is_empty() {
            self.geldoc.abort_run(format!("{} produced no image.", tray.label()));
            return format!("Run stopped — {} produced no image.", tray.label());
        }

        match goal {
            RunGoal::Acquire => self.geldoc.run.captured.push((tray, frames)),
            RunGoal::AutoExpose => {
                // The frame that came back was taken at the exposure the
                // metering settled on; that is the number the plan wants.
                let metered = frames[0].1.exposure_seconds;
                self.geldoc.plan.get_mut(tray).apply_metered(metered);
            }
        }

        self.geldoc.run.at += 1;
        if self.geldoc.run.at < self.geldoc.run.queue.len() {
            self.geldoc_begin_channel();
            return format!("{} done — {}", tray.label(), self.geldoc.phase.label());
        }
        self.geldoc_finish_run()
    }

    /// Every channel is done: build the document (or report what Auto found).
    fn geldoc_finish_run(&mut self) -> String {
        let goal = self.geldoc.run.goal;
        let captured = std::mem::take(&mut self.geldoc.run.captured);
        self.geldoc.run = Run::default();
        self.geldoc.phase = RunPhase::Idle;

        match goal {
            Some(RunGoal::AutoExpose) => {
                let summary = self
                    .geldoc
                    .plan
                    .selected()
                    .iter()
                    .map(|&t| format!("{}: {}", t.label(), self.geldoc.plan.get(t).summary()))
                    .collect::<Vec<_>>()
                    .join(" · ");
                self.geldoc.message = format!("Auto set {summary}");
                format!("Auto exposure — {summary}")
            }
            Some(RunGoal::Acquire) => {
                if captured.is_empty() {
                    self.geldoc.message = "Ready.".into();
                    return "Run finished with no image.".into();
                }
                let names: Vec<String> =
                    captured.iter().map(|(t, _)| t.label().to_string()).collect();
                let channels: Vec<CapturedChannel> = captured
                    .into_iter()
                    .map(|(tray, frames)| CapturedChannel {
                        name: tray.label().to_string(),
                        color: channel_color(tray),
                        frames,
                    })
                    .collect();
                self.adopt_capture_channels(channels);
                // Saturated pixels carry no intensity, so anything measured from
                // them is wrong and the user should see them at a glance.
                let highlight = self.geldoc.plan.highlight_saturated;
                self.set_show_overexposed(highlight);
                self.geldoc.message = format!("Captured {}.", names.join(" + "));
                format!("Captured {}.", names.join(" + "))
            }
            None => {
                self.geldoc.message = "Ready.".into();
                "Run finished.".into()
            }
        }
    }
}
