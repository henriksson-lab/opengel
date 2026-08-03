//! Protocols — saved, re-runnable acquisition recipes.
//!
//! A protocol is the unit of reproducibility: pick it, press Run, get the same
//! acquisition you got last week. It carries the application, the exposure
//! settings, the stain-free activation time and which steps are enabled.
//!
//! **One default protocol per tray type.** That is what the instrument's green
//! Run button executes: the user inserts a tray, presses the button, and the
//! default protocol for *that* tray runs. Keeping the invariant here (rather
//! than in the UI) is what makes the button's behaviour predictable — see
//! [`ProtocolLibrary::set_default`].

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::application::{self, Application};
use super::TrayType;

/// Exposure range the instrument supports, in seconds.
pub const EXPOSURE_MIN_S: f64 = 0.001;
pub const EXPOSURE_MAX_S: f64 = 10.0;

/// Default stain-free activation time, in seconds.
pub const DEFAULT_ACTIVATION_S: f64 = 45.0;

/// How the exposure time is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExposureMode {
    /// Optimise so all bands are within range — nothing clips.
    AutoIntense,
    /// Expose longer so faint bands are visible, accepting that bright bands
    /// may saturate. Saturated pixels are not quantifiable, which is why the
    /// saturation overlay matters most in this mode.
    AutoFaint,
    /// A fixed time the user set.
    Manual,
}

impl ExposureMode {
    pub fn label(self) -> &'static str {
        match self {
            ExposureMode::AutoIntense => "Auto — intense bands",
            ExposureMode::AutoFaint => "Auto — faint bands",
            ExposureMode::Manual => "Manual",
        }
    }
    pub fn is_auto(self) -> bool {
        !matches!(self, ExposureMode::Manual)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ExposureSetting {
    pub mode: ExposureMode,
    /// Manual exposure time in seconds. Also carries the time a previous auto
    /// run settled on, so switching to Manual starts from what worked.
    pub manual_s: f64,
}

impl Default for ExposureSetting {
    fn default() -> Self {
        Self {
            mode: ExposureMode::AutoIntense,
            manual_s: 0.1,
        }
    }
}

impl ExposureSetting {
    pub fn clamped_manual(&self) -> f64 {
        self.manual_s.clamp(EXPOSURE_MIN_S, EXPOSURE_MAX_S)
    }
}

/// The steps of a protocol, in the order they run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ProtocolStep {
    Application,
    Exposure,
    /// Stain-free UV activation, before the exposure.
    Activation,
    Capture,
}

impl ProtocolStep {
    pub const ALL: [ProtocolStep; 4] = [
        ProtocolStep::Application,
        ProtocolStep::Exposure,
        ProtocolStep::Activation,
        ProtocolStep::Capture,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ProtocolStep::Application => "Application",
            ProtocolStep::Exposure => "Image exposure",
            ProtocolStep::Activation => "Gel activation",
            ProtocolStep::Capture => "Capture",
        }
    }

    /// Steps that cannot be switched off: without an application there is no
    /// tray to check, and without a capture there is no image.
    pub fn is_mandatory(self) -> bool {
        matches!(self, ProtocolStep::Application | ProtocolStep::Capture)
    }
}

/// A saved acquisition recipe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Protocol {
    pub name: String,
    /// Application id (see [`application::by_id`]).
    pub application: String,
    pub exposure: ExposureSetting,
    /// Stain-free UV activation time, seconds.
    pub activation_s: f64,
    /// Steps the user has switched off. Stored as the *disabled* set so a
    /// protocol saved before a new step existed still runs that step.
    #[serde(default)]
    pub disabled_steps: BTreeSet<ProtocolStep>,
    /// Render saturated pixels red in the resulting image.
    #[serde(default = "default_true")]
    pub highlight_saturated: bool,
}

fn default_true() -> bool {
    true
}

impl Protocol {
    /// A protocol preset for `application`.
    pub fn for_application(name: impl Into<String>, app: &Application) -> Self {
        Self {
            name: name.into(),
            application: app.id.to_string(),
            exposure: ExposureSetting::default(),
            activation_s: DEFAULT_ACTIVATION_S,
            disabled_steps: BTreeSet::new(),
            highlight_saturated: true,
        }
    }

    /// A starting protocol for a tray, named after it.
    pub fn default_for_tray(tray: TrayType) -> Self {
        let app = application::default_for_tray(tray);
        Self::for_application(format!("{} default", tray.label()), app)
    }

    pub fn application(&self) -> Option<&'static Application> {
        application::by_id(&self.application)
    }

    /// The tray this protocol needs, via its application.
    pub fn tray(&self) -> Option<TrayType> {
        self.application().map(|a| a.tray)
    }

    /// Whether a step runs. Activation is inherently stain-free-only, so it is
    /// reported off for applications that do not need it however the flag is
    /// set — otherwise a protocol copied from a stain-free one would sit there
    /// burning UV into a Coomassie gel.
    pub fn step_enabled(&self, step: ProtocolStep) -> bool {
        if step == ProtocolStep::Activation && !self.application().is_some_and(|a| a.needs_activation)
        {
            return false;
        }
        step.is_mandatory() || !self.disabled_steps.contains(&step)
    }

    pub fn set_step_enabled(&mut self, step: ProtocolStep, enabled: bool) {
        if step.is_mandatory() {
            return;
        }
        if enabled {
            self.disabled_steps.remove(&step);
        } else {
            self.disabled_steps.insert(step);
        }
    }

    /// The steps that will actually run, in order, with their display numbers.
    pub fn active_steps(&self) -> Vec<ProtocolStep> {
        ProtocolStep::ALL
            .into_iter()
            .filter(|&s| self.step_enabled(s))
            .collect()
    }

    pub fn activation_s_clamped(&self) -> f64 {
        // A negative or absurd activation time is a UV exposure nobody asked
        // for; bound it at five minutes.
        self.activation_s.clamp(0.0, 300.0)
    }
}

/// Every saved protocol, plus which one is default for each tray.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProtocolLibrary {
    #[serde(default)]
    pub protocols: Vec<Protocol>,
    /// Tray → protocol name. At most one entry per tray, by construction.
    #[serde(default)]
    pub default_by_tray: BTreeMap<TrayType, String>,
}

impl ProtocolLibrary {
    /// A library with one starter protocol per tray, the stain-free one made
    /// default — that is the tray the instrument ships with.
    pub fn starter() -> Self {
        let mut library = Self::default();
        for tray in TrayType::ALL {
            library.protocols.push(Protocol::default_for_tray(tray));
        }
        for tray in TrayType::ALL {
            let name = Protocol::default_for_tray(tray).name;
            library.default_by_tray.insert(tray, name);
        }
        library
    }

    pub fn get(&self, name: &str) -> Option<&Protocol> {
        self.protocols.iter().find(|p| p.name == name)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut Protocol> {
        self.protocols.iter_mut().find(|p| p.name == name)
    }

    pub fn names(&self) -> Vec<String> {
        self.protocols.iter().map(|p| p.name.clone()).collect()
    }

    /// Insert or replace a protocol by name. Returns its index.
    pub fn save(&mut self, protocol: Protocol) -> usize {
        match self.protocols.iter().position(|p| p.name == protocol.name) {
            Some(i) => {
                self.protocols[i] = protocol;
                i
            }
            None => {
                self.protocols.push(protocol);
                self.protocols.len() - 1
            }
        }
    }

    /// Remove a protocol, and any default binding that pointed at it — a green
    /// button bound to a protocol that no longer exists would do nothing, with
    /// no way for the user to see why.
    pub fn remove(&mut self, name: &str) {
        self.protocols.retain(|p| p.name != name);
        self.default_by_tray.retain(|_, n| n != name);
    }

    /// A name not already taken, derived from `base`.
    pub fn unique_name(&self, base: &str) -> String {
        if self.get(base).is_none() {
            return base.to_string();
        }
        (2..)
            .map(|n| format!("{base} {n}"))
            .find(|candidate| self.get(candidate).is_none())
            .unwrap_or_else(|| base.to_string())
    }

    /// Make `name` the default for its own tray.
    ///
    /// Binds to the protocol's tray, not to a caller-supplied one, so the
    /// invariant "the default for a tray is runnable on that tray" cannot be
    /// broken from the UI. A protocol whose application is unknown (an id from a
    /// newer build) cannot be a default at all.
    pub fn set_default(&mut self, name: &str) -> bool {
        let Some(tray) = self.get(name).and_then(|p| p.tray()) else {
            return false;
        };
        self.default_by_tray.insert(tray, name.to_string());
        true
    }

    pub fn is_default(&self, name: &str) -> bool {
        self.default_by_tray.values().any(|n| n == name)
    }

    /// The protocol the green Run button should execute for `tray`.
    pub fn default_for(&self, tray: TrayType) -> Option<&Protocol> {
        let name = self.default_by_tray.get(&tray)?;
        self.get(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_starter_library_binds_every_tray() {
        let library = ProtocolLibrary::starter();
        for tray in TrayType::ALL {
            let protocol = library.default_for(tray).expect("a default per tray");
            assert_eq!(protocol.tray(), Some(tray), "bound to the wrong tray");
        }
    }

    #[test]
    fn a_default_binds_to_its_own_tray() {
        let mut library = ProtocolLibrary::starter();
        let blue = Protocol::for_application("my blue run", application::by_id("blue-sybr-safe").expect("app"));
        library.save(blue);
        assert!(library.set_default("my blue run"));
        assert_eq!(
            library.default_for(TrayType::Blue).map(|p| p.name.as_str()),
            Some("my blue run")
        );
        // The tray it did not belong to keeps its own default.
        assert_ne!(
            library.default_for(TrayType::Uv).map(|p| p.name.as_str()),
            Some("my blue run")
        );
    }

    #[test]
    fn only_one_default_per_tray() {
        let mut library = ProtocolLibrary::starter();
        let a = Protocol::for_application("uv a", application::by_id("uv-etbr").expect("app"));
        let b = Protocol::for_application("uv b", application::by_id("uv-gelred").expect("app"));
        library.save(a);
        library.save(b);
        library.set_default("uv a");
        library.set_default("uv b");
        assert!(!library.is_default("uv a"), "the earlier default must be replaced");
        assert!(library.is_default("uv b"));
    }

    #[test]
    fn deleting_a_default_unbinds_the_button() {
        // A green button pointing at a deleted protocol would silently do
        // nothing, with no way for the user to see why.
        let mut library = ProtocolLibrary::starter();
        let name = library
            .default_for(TrayType::Uv)
            .expect("a uv default")
            .name
            .clone();
        library.remove(&name);
        assert!(library.default_for(TrayType::Uv).is_none());
        assert!(!library.default_by_tray.values().any(|n| n == &name));
    }

    #[test]
    fn activation_is_off_for_applications_that_do_not_need_it() {
        let mut coomassie = Protocol::for_application(
            "coomassie",
            application::by_id("white-coomassie-blue").expect("app"),
        );
        coomassie.set_step_enabled(ProtocolStep::Activation, true);
        assert!(
            !coomassie.step_enabled(ProtocolStep::Activation),
            "a non-stain-free protocol must never run a UV activation"
        );
        assert!(!coomassie.active_steps().contains(&ProtocolStep::Activation));

        let stain_free =
            Protocol::for_application("sf", application::by_id("stainfree-gel").expect("app"));
        assert!(stain_free.step_enabled(ProtocolStep::Activation));
    }

    #[test]
    fn mandatory_steps_cannot_be_switched_off() {
        let mut p = Protocol::default_for_tray(TrayType::Uv);
        p.set_step_enabled(ProtocolStep::Capture, false);
        p.set_step_enabled(ProtocolStep::Application, false);
        assert!(p.step_enabled(ProtocolStep::Capture));
        assert!(p.step_enabled(ProtocolStep::Application));
        // Optional ones can.
        p.set_step_enabled(ProtocolStep::Exposure, false);
        assert!(!p.step_enabled(ProtocolStep::Exposure));
    }

    #[test]
    fn unique_name_does_not_collide() {
        let mut library = ProtocolLibrary::default();
        library.save(Protocol::default_for_tray(TrayType::Uv));
        let taken = library.protocols[0].name.clone();
        let fresh = library.unique_name(&taken);
        assert_ne!(fresh, taken);
        assert!(library.get(&fresh).is_none());
    }

    #[test]
    fn a_library_round_trips_through_json() {
        let library = ProtocolLibrary::starter();
        let json = serde_json::to_string(&library).expect("serialize");
        let back: ProtocolLibrary = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(library, back);
    }

    #[test]
    fn a_protocol_saved_before_a_step_existed_still_runs_it() {
        // Disabled steps are stored, not enabled ones, so an older file does not
        // silently switch off a step it never heard of.
        let json = r#"{"name":"old","application":"uv-etbr",
            "exposure":{"mode":"AutoIntense","manual_s":0.1},"activation_s":45.0}"#;
        let p: Protocol = serde_json::from_str(json).expect("deserialize");
        assert!(p.step_enabled(ProtocolStep::Exposure));
        assert!(p.highlight_saturated);
    }
}
