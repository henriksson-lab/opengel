//! State for the Gel Doc EZ tab: the connected enclosure, the protocol library,
//! and the run state machine that sequences enclosure and camera.
//!
//! The Live tab drives a bare camera. This tab drives an *instrument*: the tray
//! decides the light source, the door gates the lamps, faults latch, and a run
//! is a scripted sequence rather than a button that grabs a frame. That is why
//! it is a separate tab rather than more controls on the existing one.

use image::DynamicImage;
use opengel::core::model::CaptureMeta;
use opengel::instrument::application::{self, Application};
use opengel::instrument::protocol::{
    ExposureMode, Protocol, ProtocolLibrary, ProtocolStep, EXPOSURE_MAX_S, EXPOSURE_MIN_S,
};
use opengel::instrument::{Faults, InstrumentInfo, Sense, TrayType};

use crate::camera_worker::AutoExposureMode;
use crate::instrument_worker::InstrumentHandle;

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
    /// Whether a rising undecoded bit runs the default protocol.
    pub watch_run_button: bool,

    pub library: ProtocolLibrary,
    /// Index into `library.protocols` of the protocol being edited.
    pub selected_protocol: usize,
    /// Which step's options are shown.
    pub selected_step: ProtocolStep,

    pub phase: RunPhase,
    /// Frames captured by the current run, held until the instrument confirms
    /// the door stayed shut. An exposure interrupted by an opened door is not
    /// data, so it is never adopted on optimism.
    pending_frames: Vec<(DynamicImage, CaptureMeta)>,
    /// The exposure the last run actually used — the reference the manual calls
    /// for when switching from auto to manual.
    pub last_exposure_s: Option<f64>,
    /// One-line message for the tab's own status area.
    pub message: String,
    /// Which protocol the name field currently shows. The tab refreshes on every
    /// instrument poll, so the field is only rewritten when the *selection*
    /// changes — otherwise a refresh mid-keystroke would fight the typing.
    pub name_field_for: std::cell::Cell<usize>,
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
            watch_run_button: true,
            library: ProtocolLibrary::starter(),
            selected_protocol: 0,
            selected_step: ProtocolStep::Application,
            phase: RunPhase::Idle,
            pending_frames: Vec::new(),
            last_exposure_s: None,
            message: "Not connected.".into(),
            name_field_for: std::cell::Cell::new(usize::MAX),
        }
    }

    // ---- protocol editing ----

    pub fn protocol(&self) -> Option<&Protocol> {
        self.library.protocols.get(self.selected_protocol)
    }

    pub fn protocol_mut(&mut self) -> Option<&mut Protocol> {
        self.library.protocols.get_mut(self.selected_protocol)
    }

    pub fn select_protocol(&mut self, index: usize) {
        if index < self.library.protocols.len() {
            self.selected_protocol = index;
        }
    }

    /// The application the selected protocol uses.
    pub fn application(&self) -> Option<&'static Application> {
        self.protocol().and_then(|p| p.application())
    }

    /// Add a protocol for the inserted tray (or the selected one's tray), select
    /// it, and return its name.
    pub fn new_protocol(&mut self) -> String {
        let tray = self
            .inserted_tray()
            .or_else(|| self.protocol().and_then(|p| p.tray()))
            .unwrap_or(TrayType::StainFree);
        let mut protocol = Protocol::default_for_tray(tray);
        protocol.name = self.library.unique_name(&format!("{} protocol", tray.label()));
        let name = protocol.name.clone();
        self.selected_protocol = self.library.save(protocol);
        name
    }

    pub fn delete_selected_protocol(&mut self) {
        let Some(name) = self.protocol().map(|p| p.name.clone()) else {
            return;
        };
        self.library.remove(&name);
        self.selected_protocol = self
            .selected_protocol
            .min(self.library.protocols.len().saturating_sub(1));
    }

    /// Make the selected protocol the default for its tray — the one the green
    /// Run button executes when that tray is in.
    pub fn make_selected_default(&mut self) -> bool {
        let Some(name) = self.protocol().map(|p| p.name.clone()) else {
            return false;
        };
        self.library.set_default(&name)
    }

    pub fn rename_selected_protocol(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let Some(old) = self.protocol().map(|p| p.name.clone()) else {
            return;
        };
        if old == name || self.library.get(name).is_some() {
            return;
        }
        let was_default = self.library.is_default(&old);
        if let Some(p) = self.protocol_mut() {
            p.name = name.to_string();
        }
        // Move the default binding with the name, or renaming a default would
        // silently unbind the green button.
        if was_default {
            self.library.default_by_tray.retain(|_, n| n != &old);
            self.library.set_default(name);
        }
    }

    pub fn set_application(&mut self, id: &str) {
        if application::by_id(id).is_none() {
            return;
        }
        if let Some(p) = self.protocol_mut() {
            p.application = id.to_string();
        }
    }

    pub fn set_exposure_mode(&mut self, mode: ExposureMode) {
        if let Some(p) = self.protocol_mut() {
            p.exposure.mode = mode;
        }
    }

    pub fn set_manual_exposure_s(&mut self, seconds: f64) {
        if let Some(p) = self.protocol_mut() {
            p.exposure.manual_s = seconds.clamp(EXPOSURE_MIN_S, EXPOSURE_MAX_S);
        }
    }

    pub fn set_activation_s(&mut self, seconds: f64) {
        if let Some(p) = self.protocol_mut() {
            p.activation_s = seconds.max(0.0);
        }
    }

    pub fn set_step_enabled(&mut self, step: ProtocolStep, enabled: bool) {
        if let Some(p) = self.protocol_mut() {
            p.set_step_enabled(step, enabled);
        }
    }

    /// Map the exposure slider (log scale over the instrument's own 0.001–10 s
    /// range) to a time, and back.
    pub fn exposure_from_slider(f: f32) -> f64 {
        let f = f.clamp(0.0, 1.0) as f64;
        EXPOSURE_MIN_S * (EXPOSURE_MAX_S / EXPOSURE_MIN_S).powf(f)
    }
    pub fn slider_from_exposure(t: f64) -> f32 {
        let t = t.clamp(EXPOSURE_MIN_S, EXPOSURE_MAX_S);
        ((t / EXPOSURE_MIN_S).ln() / (EXPOSURE_MAX_S / EXPOSURE_MIN_S).ln()) as f32
    }

    // ---- instrument state ----

    pub fn inserted_tray(&self) -> Option<TrayType> {
        self.sense.and_then(|s| s.tray)
    }

    pub fn door_closed(&self) -> bool {
        self.sense.is_some_and(|s| s.door_closed)
    }

    /// The tray the selected protocol needs but which is not the one inserted.
    /// This is the commonest setup mistake — the right dye on the wrong tray
    /// images as a blank gel — so it is surfaced before the run, not after.
    pub fn tray_mismatch(&self) -> Option<(TrayType, Option<TrayType>)> {
        let wanted = self.application()?.tray;
        let inserted = self.inserted_tray();
        (inserted != Some(wanted)).then_some((wanted, inserted))
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
        if self.protocol().is_none() {
            return Some("No protocol selected.".into());
        }
        if self.application().is_none() {
            return Some("This protocol's application is not available.".into());
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
        if let Some((wanted, _)) = self.tray_mismatch() {
            return Some(format!(
                "This application needs the {} tray.",
                wanted.label()
            ));
        }
        None
    }

    /// The activation time this run should use: only stain-free applications
    /// activate, and only when the step is enabled.
    pub fn effective_activation_s(&self) -> f64 {
        let Some(protocol) = self.protocol() else {
            return 0.0;
        };
        if protocol.step_enabled(ProtocolStep::Activation) {
            protocol.activation_s_clamped()
        } else {
            0.0
        }
    }

    /// How the capture should be taken, once the lamps are lit.
    pub fn capture_plan(&self) -> Option<CapturePlan> {
        let protocol = self.protocol()?;
        // With the exposure step switched off, the protocol's own time is not
        // applied — the camera keeps whatever it is already set to.
        if !protocol.step_enabled(ProtocolStep::Exposure) {
            return Some(CapturePlan::AsIs);
        }
        Some(match protocol.exposure.mode {
            ExposureMode::AutoIntense => CapturePlan::Auto(AutoExposureMode::IntenseBands),
            ExposureMode::AutoFaint => CapturePlan::Auto(AutoExposureMode::FaintBands),
            ExposureMode::Manual => CapturePlan::Manual(protocol.exposure.clamped_manual()),
        })
    }

    // ---- run state machine ----

    pub fn take_pending_frames(&mut self) -> Vec<(DynamicImage, CaptureMeta)> {
        std::mem::take(&mut self.pending_frames)
    }

    pub fn hold_frames(&mut self, frames: Vec<(DynamicImage, CaptureMeta)>) {
        self.last_exposure_s = frames.first().map(|(_, meta)| meta.exposure_seconds);
        self.pending_frames = frames;
        self.phase = RunPhase::Verifying;
    }

    pub fn abort_run(&mut self, message: impl Into<String>) {
        self.pending_frames.clear();
        self.phase = RunPhase::Idle;
        self.message = message.into();
        if let Some(inst) = &self.inst {
            inst.abort();
        }
    }
}

/// The run state machine, spanning the instrument and camera workers.
///
/// These live on [`AppState`] because a run needs both devices: the enclosure
/// handle for the light and the interlocks, the camera handle for the exposure.
impl crate::state::AppState {
    /// Start a run of the selected protocol. Returns the line to show.
    pub fn geldoc_run(&mut self) -> String {
        if let Some(blocker) = self.geldoc.run_blocker() {
            self.geldoc.message = blocker.clone();
            return format!("Cannot run: {blocker}");
        }
        let activation_s = self.geldoc.effective_activation_s();
        self.geldoc.phase = if activation_s > 0.0 {
            RunPhase::Activating {
                elapsed_s: 0.0,
                total_s: activation_s,
            }
        } else {
            RunPhase::LightingUp
        };
        let name = self
            .geldoc
            .protocol()
            .map(|p| p.name.clone())
            .unwrap_or_default();
        self.geldoc.message = self.geldoc.phase.label();
        if let Some(inst) = &self.geldoc.inst {
            inst.begin_run(activation_s);
        }
        format!("Running protocol “{name}”…")
    }

    /// The hardware Run button: run the default protocol for the tray that is
    /// actually inserted, which is what the button means on the instrument.
    pub fn geldoc_button_run(&mut self) -> String {
        let Some(tray) = self.geldoc.inserted_tray() else {
            return "Run button pressed, but no sample tray is inserted.".into();
        };
        let Some(name) = self
            .geldoc
            .library
            .default_for(tray)
            .map(|p| p.name.clone())
        else {
            return format!(
                "Run button pressed, but no default protocol is set for the {} tray.",
                tray.label()
            );
        };
        if let Some(index) = self
            .geldoc
            .library
            .protocols
            .iter()
            .position(|p| p.name == name)
        {
            self.geldoc.selected_protocol = index;
        }
        self.geldoc_run()
    }

    /// The lamps are lit and stable: take the picture.
    pub fn geldoc_lights_ready(&mut self) {
        self.geldoc.phase = RunPhase::Exposing;
        self.geldoc.message = "Exposing…".into();
        let plan = self.geldoc.capture_plan();
        self.capturing = true;
        self.cancel_requested = false;
        self.capture_status = "Exposing…".into();
        let Some(cam) = &self.cam else {
            self.geldoc.abort_run("No camera available for the exposure.");
            self.capturing = false;
            return;
        };
        match plan {
            Some(CapturePlan::Auto(mode)) => cam.capture_auto(mode, EXPOSURE_MIN_S, EXPOSURE_MAX_S),
            Some(CapturePlan::Manual(seconds)) => cam.capture_single(seconds),
            Some(CapturePlan::AsIs) => cam.capture_single(self.live_exposure_s.max(EXPOSURE_MIN_S)),
            None => {
                self.capturing = false;
                self.geldoc.abort_run("No protocol to run.");
            }
        }
    }

    /// The camera is done. Hold the frames and ask the instrument whether the
    /// run was clean before they become a document.
    pub fn geldoc_capture_done(&mut self, frames: Vec<(DynamicImage, CaptureMeta)>) {
        self.geldoc.hold_frames(frames);
        self.geldoc.message = "Checking the run…".into();
        if let Some(inst) = &self.geldoc.inst {
            inst.end_run();
        }
    }

    /// The instrument has reported on the finished run. Adopt the image, or
    /// discard it if the door was opened mid-exposure.
    pub fn geldoc_run_finished(&mut self, faults: Faults, door_violation: bool) -> String {
        self.geldoc.faults = faults;
        self.capturing = false;
        let frames = self.geldoc.take_pending_frames();
        self.geldoc.phase = RunPhase::Idle;

        if door_violation {
            self.geldoc.message =
                "Discarded: the door was opened during the exposure.".to_string();
            return "Run discarded — the door was opened during the exposure.".into();
        }
        if frames.is_empty() {
            self.geldoc.message = "Ready.".into();
            return "Run finished with no image.".into();
        }

        // The application says what is on the gel, so the new document starts
        // as the right kind — base pairs or daltons — without the user setting
        // it again.
        if let Some(app) = self.geldoc.application() {
            self.gel_type = app.gel_type();
        }
        let application = self
            .geldoc
            .application()
            .map(|a| a.label())
            .unwrap_or_else(|| "unknown application".into());
        let exposure = self.geldoc.last_exposure_s.unwrap_or(0.0);
        let highlight = self
            .geldoc
            .protocol()
            .is_some_and(|p| p.highlight_saturated);
        let (imgs, metas): (Vec<_>, Vec<_>) = frames.into_iter().unzip();
        self.adopt_capture(imgs, metas);
        // The protocol's display option, applied to the document it produced:
        // saturated pixels carry no intensity, so anything measured from them is
        // wrong and the user should be able to see them at a glance.
        self.set_show_overexposed(highlight);
        self.geldoc.message = format!("Captured at {exposure:.3} s.");
        format!("{application} — captured at {exposure:.3} s.")
    }
}

/// How the camera should take this run's image.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CapturePlan {
    Auto(AutoExposureMode),
    Manual(f64),
    /// Exposure step disabled — shoot at whatever the camera is set to.
    AsIs,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with(tray: Option<TrayType>, door_closed: bool) -> GelDocState {
        let mut st = GelDocState::new();
        st.connected = true;
        st.sense = Some(Sense {
            tray,
            door_closed,
            busy: false,
            raw: 0,
        });
        // Select the stain-free protocol, whose tray matches the default sense.
        st.selected_protocol = st
            .library
            .protocols
            .iter()
            .position(|p| p.tray() == Some(TrayType::StainFree))
            .expect("a stain-free protocol");
        st
    }

    #[test]
    fn an_open_door_blocks_the_run() {
        let st = state_with(Some(TrayType::StainFree), false);
        assert_eq!(st.run_blocker().as_deref(), Some("The door is open."));
    }

    #[test]
    fn a_missing_tray_blocks_the_run() {
        let st = state_with(None, true);
        assert!(st.run_blocker().is_some_and(|b| b.contains("tray")));
    }

    #[test]
    fn the_wrong_tray_blocks_the_run_and_names_the_right_one() {
        // The failure this catches: the right dye on the wrong tray, which
        // images as a blank gel with nothing obviously wrong.
        let st = state_with(Some(TrayType::White), true);
        let blocker = st.run_blocker().expect("blocked");
        assert!(blocker.contains("Stain-free"), "{blocker}");
        assert_eq!(
            st.tray_mismatch(),
            Some((TrayType::StainFree, Some(TrayType::White)))
        );
    }

    #[test]
    fn a_matching_setup_is_not_blocked() {
        let st = state_with(Some(TrayType::StainFree), true);
        assert_eq!(st.run_blocker(), None);
        assert_eq!(st.tray_mismatch(), None);
    }

    #[test]
    fn only_stain_free_protocols_activate() {
        let mut st = state_with(Some(TrayType::StainFree), true);
        assert!(st.effective_activation_s() > 0.0);

        // Switch to a Coomassie protocol: no activation, whatever the stored time.
        st.set_application("white-coomassie-blue");
        st.set_activation_s(60.0);
        assert_eq!(st.effective_activation_s(), 0.0);
    }

    #[test]
    fn disabling_the_activation_step_skips_it() {
        let mut st = state_with(Some(TrayType::StainFree), true);
        st.set_step_enabled(ProtocolStep::Activation, false);
        assert_eq!(st.effective_activation_s(), 0.0);
    }

    #[test]
    fn the_capture_plan_follows_the_exposure_mode() {
        let mut st = state_with(Some(TrayType::StainFree), true);
        st.set_exposure_mode(ExposureMode::AutoFaint);
        assert_eq!(
            st.capture_plan(),
            Some(CapturePlan::Auto(AutoExposureMode::FaintBands))
        );
        st.set_exposure_mode(ExposureMode::Manual);
        st.set_manual_exposure_s(0.25);
        assert_eq!(st.capture_plan(), Some(CapturePlan::Manual(0.25)));
        st.set_step_enabled(ProtocolStep::Exposure, false);
        assert_eq!(st.capture_plan(), Some(CapturePlan::AsIs));
    }

    #[test]
    fn manual_exposure_is_held_to_the_instrument_range() {
        let mut st = state_with(Some(TrayType::StainFree), true);
        st.set_manual_exposure_s(500.0);
        assert_eq!(st.protocol().expect("protocol").exposure.manual_s, EXPOSURE_MAX_S);
        st.set_manual_exposure_s(0.0);
        assert_eq!(st.protocol().expect("protocol").exposure.manual_s, EXPOSURE_MIN_S);
    }

    #[test]
    fn the_exposure_slider_round_trips() {
        for t in [0.001, 0.01, 0.1, 1.0, 10.0] {
            let back = GelDocState::exposure_from_slider(GelDocState::slider_from_exposure(t));
            assert!((back - t).abs() < t * 1e-6, "{t} -> {back}");
        }
    }

    #[test]
    fn renaming_a_default_protocol_keeps_it_bound_to_the_button() {
        // Otherwise a rename silently unbinds the green button, and the user
        // finds out by pressing it and getting nothing.
        let mut st = state_with(Some(TrayType::StainFree), true);
        st.make_selected_default();
        st.rename_selected_protocol("My stain-free run");
        assert!(st.library.is_default("My stain-free run"));
        assert_eq!(
            st.library
                .default_for(TrayType::StainFree)
                .map(|p| p.name.as_str()),
            Some("My stain-free run")
        );
    }

    #[test]
    fn frames_are_held_not_adopted_until_the_run_is_verified() {
        let mut st = state_with(Some(TrayType::StainFree), true);
        let frame = (
            DynamicImage::new_luma8(2, 2),
            CaptureMeta {
                exposure_seconds: 0.3,
                ..Default::default()
            },
        );
        st.hold_frames(vec![frame]);
        assert_eq!(st.phase, RunPhase::Verifying);
        assert_eq!(st.last_exposure_s, Some(0.3));

        // A door violation discards them rather than handing back a bad image.
        st.abort_run("The door was opened during imaging.");
        assert!(st.take_pending_frames().is_empty());
        assert_eq!(st.phase, RunPhase::Idle);
    }
}
