//! What to shoot: one image per selected *channel*.
//!
//! **A channel is a light source.** On this instrument the light source is the
//! tray — the sense word reports which tray is in, and that alone decides which
//! lamps fire. There is no filter wheel and no per-dye optical state: the camera
//! is monochrome behind one fixed emission filter, so ethidium bromide and SYBR
//! Green on the UV tray are the *same acquisition*. Nothing here can be
//! spectrally unmixed and nothing tries to be, which is why the dye is not part
//! of the plan at all.
//!
//! That leaves an acquisition as a short, honest list: for each channel you
//! picked, take one image — a single frame, or an HDR bracket when one exposure
//! cannot hold both the faint and the bright bands.
//!
//! Only one tray is physically inserted at a time, so a multi-channel plan is
//! run with a pause between channels while the user swaps the tray. The plan
//! says what to take; the run state machine in `gui::geldoc` sequences it.

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

/// One channel of an acquisition: a light source and how to expose it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChannelPlan {
    /// The light source, which on this instrument is the tray.
    pub tray: TrayType,
    /// Whether this channel is part of the run.
    pub selected: bool,
    pub mode: CaptureMode,
    /// Exposure time for a single capture, seconds.
    pub exposure_s: f64,
    /// Bracket bounds and length for an HDR capture.
    pub hdr_min_s: f64,
    pub hdr_max_s: f64,
    pub hdr_steps: usize,
    /// Stain-free UV activation before the exposure, seconds. Only the
    /// stain-free tray activates; 0 skips the step.
    pub activation_s: f64,
}

impl ChannelPlan {
    pub fn new(tray: TrayType) -> Self {
        Self {
            tray,
            // The UV tray is what the instrument ships with, so a fresh plan is
            // already runnable without ticking anything.
            selected: tray == TrayType::Uv,
            mode: CaptureMode::Single,
            exposure_s: 0.1,
            hdr_min_s: 0.01,
            hdr_max_s: 1.0,
            hdr_steps: 3,
            activation_s: if tray == TrayType::StainFree {
                DEFAULT_ACTIVATION_S
            } else {
                0.0
            },
        }
    }

    pub fn label(&self) -> &'static str {
        self.tray.label()
    }

    /// Whether this channel runs the stain-free UV activation. Inherently
    /// stain-free-only, whatever the stored time says — otherwise a channel that
    /// inherited a time would sit there burning UV into a Coomassie gel.
    pub fn activates(&self) -> bool {
        self.tray == TrayType::StainFree && self.activation_s > 0.0
    }

    /// The activation time this channel should actually use.
    pub fn effective_activation_s(&self) -> f64 {
        if self.activates() {
            // A negative or absurd activation time is a UV exposure nobody asked
            // for; bound it at five minutes.
            self.activation_s.clamp(0.0, 300.0)
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

    /// Adopt what an auto exposure metered for this channel.
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

    /// One line describing what this channel will shoot.
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

/// The whole acquisition: which channels to take, and how.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcquisitionPlan {
    /// One entry per light source, in [`TrayType::ALL`] order.
    pub channels: Vec<ChannelPlan>,
    /// Render saturated pixels red in the resulting image.
    #[serde(default = "default_true")]
    pub highlight_saturated: bool,
}

fn default_true() -> bool {
    true
}

impl Default for AcquisitionPlan {
    fn default() -> Self {
        Self::new()
    }
}

impl AcquisitionPlan {
    pub fn new() -> Self {
        Self {
            channels: TrayType::ALL.into_iter().map(ChannelPlan::new).collect(),
            highlight_saturated: true,
        }
    }

    /// Repair a plan loaded from an older config: every tray gets an entry, in
    /// catalogue order, whatever the file held.
    pub fn normalized(mut self) -> Self {
        let mut channels: Vec<ChannelPlan> = TrayType::ALL
            .into_iter()
            .map(|tray| {
                self.channels
                    .iter()
                    .find(|c| c.tray == tray)
                    .copied()
                    .unwrap_or_else(|| ChannelPlan::new(tray))
            })
            .collect();
        // Nothing selected is a plan that cannot run; fall back to the shipped
        // tray rather than leaving a Run button that refuses forever.
        if !channels.iter().any(|c| c.selected) {
            if let Some(uv) = channels.iter_mut().find(|c| c.tray == TrayType::Uv) {
                uv.selected = true;
            }
        }
        self.channels = channels;
        self
    }

    pub fn get(&self, tray: TrayType) -> &ChannelPlan {
        self.channels
            .iter()
            .find(|c| c.tray == tray)
            .expect("a plan has one channel per tray")
    }

    pub fn get_mut(&mut self, tray: TrayType) -> &mut ChannelPlan {
        self.channels
            .iter_mut()
            .find(|c| c.tray == tray)
            .expect("a plan has one channel per tray")
    }

    /// The channels the run will take, in tray order.
    pub fn selected(&self) -> Vec<TrayType> {
        self.channels
            .iter()
            .filter(|c| c.selected)
            .map(|c| c.tray)
            .collect()
    }

    pub fn index_of(tray: TrayType) -> usize {
        TrayType::ALL.iter().position(|&t| t == tray).unwrap_or(0)
    }

    pub fn tray_at(index: usize) -> TrayType {
        TrayType::ALL
            .get(index)
            .copied()
            .unwrap_or(TrayType::Uv)
    }
}

/// Human-readable exposure time: milliseconds under a second, else seconds.
pub fn fmt_seconds(t: f64) -> String {
    if t < 1.0 {
        format!("{:.0} ms", t * 1000.0)
    } else {
        format!("{t:.2} s")
    }
}
