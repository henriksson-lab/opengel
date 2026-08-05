//! What to shoot: **one image**.
//!
//! An acquisition here is a single channel, always. The camera is monochrome
//! behind one fixed emission filter and the light source is whichever tray is in
//! the machine — there is no filter wheel and no per-dye optical state, so
//! ethidium bromide and SYBR Green on the UV tray are the *same acquisition*.
//! Nothing here can be spectrally unmixed and nothing tries to be, which is why
//! neither the dye nor a list of channels is part of the plan.
//!
//! Only one tray is physically inserted at a time, so "which light source" is
//! not a choice the software makes: it is a fact it reads. The plan says how to
//! expose — a single frame, or an HDR bracket when one exposure cannot hold both
//! the faint and the bright bands — and the instrument says what that exposure
//! *is a picture of*. With no instrument the exposure settings are all there is,
//! and the image is simply an image.

use serde::{Deserialize, Serialize};

use super::TrayType;

/// Exposure range the instrument supports, in seconds.
pub const EXPOSURE_MIN_S: f64 = 0.001;
pub const EXPOSURE_MAX_S: f64 = 10.0;

/// Default stain-free activation time, in seconds.
pub const DEFAULT_ACTIVATION_S: f64 = 45.0;

/// Step counts offered for an HDR bracket.
pub const HDR_STEP_OPTIONS: [usize; 4] = [2, 3, 5, 7];

/// How much brighter than the just-below-clipping exposure the longest frame of
/// an auto-set bracket is, in stops. Four EV lifts a faint band well clear of
/// the background while the short end keeps the bright bands quantifiable.
const AUTO_HDR_EV: f64 = 4.0;

/// Single frame, or an exposure bracket that is HDR-merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptureMode {
    Single,
    Hdr,
}

impl CaptureMode {
    pub fn label(self) -> &'static str {
        match self {
            CaptureMode::Single => "Single",
            CaptureMode::Hdr => "HDR",
        }
    }
    pub fn index(self) -> usize {
        match self {
            CaptureMode::Single => 0,
            CaptureMode::Hdr => 1,
        }
    }
    pub fn from_index(index: usize) -> CaptureMode {
        if index == 1 {
            CaptureMode::Hdr
        } else {
            CaptureMode::Single
        }
    }
}

/// How to expose the one image an acquisition takes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CapturePlan {
    pub mode: CaptureMode,
    /// Exposure time for a single capture, seconds.
    pub exposure_s: f64,
    /// Bracket bounds and length for an HDR capture.
    pub hdr_min_s: f64,
    pub hdr_max_s: f64,
    pub hdr_steps: usize,
    /// Stain-free UV activation before the exposure, seconds. Only ever run
    /// under the stain-free tray; 0 skips the step.
    pub activation_s: f64,
    /// Render saturated pixels red in the resulting image.
    #[serde(default = "default_true")]
    pub highlight_saturated: bool,
}

fn default_true() -> bool {
    true
}

impl Default for CapturePlan {
    fn default() -> Self {
        Self::new()
    }
}

impl CapturePlan {
    pub fn new() -> Self {
        Self {
            mode: CaptureMode::Single,
            exposure_s: 0.1,
            hdr_min_s: 0.01,
            hdr_max_s: 1.0,
            hdr_steps: 3,
            activation_s: DEFAULT_ACTIVATION_S,
            highlight_saturated: true,
        }
    }

    /// Clamp a plan loaded from an older config back into legal bounds, so a
    /// hand-edited or stale settings file cannot produce an inverted bracket or
    /// a zero-length exposure.
    pub fn normalized(mut self) -> Self {
        self.exposure_s = self.exposure_s.clamp(EXPOSURE_MIN_S, EXPOSURE_MAX_S);
        self.hdr_min_s = self.hdr_min_s.clamp(EXPOSURE_MIN_S, EXPOSURE_MAX_S);
        self.hdr_max_s = self.hdr_max_s.clamp(self.hdr_min_s, EXPOSURE_MAX_S);
        self.hdr_steps = self.hdr_steps.max(2);
        self.activation_s = self.activation_s.clamp(0.0, MAX_ACTIVATION_S);
        self
    }

    pub fn set_exposure_s(&mut self, seconds: f64) {
        self.exposure_s = seconds.clamp(EXPOSURE_MIN_S, EXPOSURE_MAX_S);
    }

    /// Adopt an exposure as the bracket's lower / upper bound. The bounds never
    /// cross: each is clamped against the other.
    pub fn set_hdr_min_s(&mut self, seconds: f64) {
        self.hdr_min_s = seconds.clamp(EXPOSURE_MIN_S, self.hdr_max_s);
    }
    pub fn set_hdr_max_s(&mut self, seconds: f64) {
        self.hdr_max_s = seconds.clamp(self.hdr_min_s, EXPOSURE_MAX_S);
    }
    pub fn set_hdr_steps(&mut self, steps: usize) {
        self.hdr_steps = steps.max(2);
    }
    pub fn set_activation_s(&mut self, seconds: f64) {
        self.activation_s = seconds.max(0.0);
    }

    /// Whether a stain-free UV activation runs before the exposure, under the
    /// tray that is in. Inherently stain-free-only, whatever the stored time
    /// says — otherwise a leftover time would sit there burning UV into a
    /// Coomassie gel.
    pub fn activates(&self, tray: Option<TrayType>) -> bool {
        tray == Some(TrayType::StainFree) && self.activation_s > 0.0
    }

    /// The activation time this acquisition should actually use.
    pub fn effective_activation_s(&self, tray: Option<TrayType>) -> f64 {
        if self.activates(tray) {
            // A negative or absurd activation time is a UV exposure nobody asked
            // for; bound it at five minutes.
            self.activation_s.clamp(0.0, MAX_ACTIVATION_S)
        } else {
            0.0
        }
    }

    /// The exposure times to shoot, in order. One entry for a single capture; a
    /// log-even bracket for HDR, because exposure is linear in time and the
    /// merge wants even coverage in stops, not in seconds.
    pub fn exposures(&self) -> Vec<f64> {
        match self.mode {
            CaptureMode::Single => vec![self.exposure_s.clamp(EXPOSURE_MIN_S, EXPOSURE_MAX_S)],
            CaptureMode::Hdr => {
                let n = self.hdr_steps.max(2);
                let lo = self.hdr_min_s.clamp(EXPOSURE_MIN_S, EXPOSURE_MAX_S);
                let hi = self.hdr_max_s.clamp(lo, EXPOSURE_MAX_S);
                if hi <= lo {
                    return vec![lo];
                }
                (0..n)
                    .map(|i| {
                        let f = i as f64 / (n - 1) as f64;
                        lo * (hi / lo).powf(f)
                    })
                    .collect()
            }
        }
    }

    /// Dynamic range an HDR bracket covers, in EV (stops).
    pub fn hdr_range_ev(&self) -> f64 {
        (self.hdr_max_s.max(1e-9) / self.hdr_min_s.max(1e-9)).log2()
    }

    /// Adopt what an auto exposure metered.
    ///
    /// `metered_s` is the time at which the brightest pixels land just below
    /// saturation. For a single capture that *is* the answer: nothing clips, so
    /// every band stays quantifiable. For a bracket it is the short end, and the
    /// long end is [`AUTO_HDR_EV`] stops above it to lift the faint bands.
    pub fn apply_metered(&mut self, metered_s: f64) {
        let metered = metered_s.clamp(EXPOSURE_MIN_S, EXPOSURE_MAX_S);
        self.exposure_s = metered;
        self.hdr_min_s = metered;
        self.hdr_max_s = (metered * 2f64.powf(AUTO_HDR_EV)).min(EXPOSURE_MAX_S);
        // A metered time already at the top of the range leaves no room for a
        // bracket; keep it a legal (if degenerate) range rather than an inverted
        // one.
        if self.hdr_max_s <= self.hdr_min_s {
            self.hdr_min_s = (self.hdr_max_s / 2f64.powf(AUTO_HDR_EV)).max(EXPOSURE_MIN_S);
        }
    }

    /// One line describing what will be shot.
    pub fn summary(&self) -> String {
        match self.mode {
            CaptureMode::Single => format!("single {}", fmt_seconds(self.exposure_s)),
            CaptureMode::Hdr => format!(
                "HDR {} – {} ×{}  ({:.1} EV)",
                fmt_seconds(self.hdr_min_s),
                fmt_seconds(self.hdr_max_s),
                self.hdr_steps.max(2),
                self.hdr_range_ev()
            ),
        }
    }
}

/// Longest stain-free activation the plan will run, in seconds.
const MAX_ACTIVATION_S: f64 = 300.0;

/// Human-readable exposure time: milliseconds under a second, else seconds.
pub fn fmt_seconds(t: f64) -> String {
    if t < 1.0 {
        format!("{:.0} ms", t * 1000.0)
    } else {
        format!("{t:.2} s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_runs_only_under_the_stain_free_tray() {
        let mut plan = CapturePlan::new();
        plan.set_activation_s(45.0);
        assert!(plan.activates(Some(TrayType::StainFree)));
        assert_eq!(plan.effective_activation_s(Some(TrayType::StainFree)), 45.0);
        // Every other light source — and no instrument at all — skips it, so a
        // stored time cannot leak a UV pre-exposure onto the wrong gel.
        for tray in [
            Some(TrayType::Uv),
            Some(TrayType::White),
            Some(TrayType::Blue),
            None,
        ] {
            assert!(!plan.activates(tray), "{tray:?} must not activate");
            assert_eq!(plan.effective_activation_s(tray), 0.0);
        }
    }

    #[test]
    fn a_stale_plan_is_clamped_into_legal_bounds() {
        let plan = CapturePlan {
            mode: CaptureMode::Hdr,
            exposure_s: 0.0,
            hdr_min_s: 5.0,
            hdr_max_s: 0.5,
            hdr_steps: 0,
            activation_s: -3.0,
            highlight_saturated: true,
        }
        .normalized();
        assert!(plan.exposure_s >= EXPOSURE_MIN_S);
        assert!(plan.hdr_min_s <= plan.hdr_max_s);
        assert!(plan.hdr_steps >= 2);
        assert_eq!(plan.activation_s, 0.0);
    }
}
