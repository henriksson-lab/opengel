//! Simulation configuration: scene layout plus each optical/geometric effect.

use crate::core::model::GelType;

use crate::sim::rng::Rng;

#[derive(Debug, Clone)]
pub struct SimConfig {
    pub width: u32,
    pub height: u32,
    pub gel_type: GelType,
    /// Built-in ladder used for the ladder lane.
    pub ladder_name: String,
    pub n_sample_lanes: usize,
    pub seed: u64,

    // ---- geometric effects ----
    /// Whole-gel rotation in degrees (|value| ≤ 50 typically).
    pub rotation_deg: f64,
    /// Quadratic "smile/frown" vertical warp amplitude (px at the edges).
    pub smile_px: f64,
    /// Low-frequency wobble warp amplitude (px).
    pub wobble_px: f64,
    /// Translation (px) — large values run the gel partly out of frame.
    pub shift_px: (f64, f64),

    // ---- optical effects ----
    /// Uneven background amplitude in normalized intensity (0..1).
    pub background: f64,
    /// Exposure gain; > 1 drives bright bands into saturation (overexposure).
    pub exposure: f64,
    /// Photon scale for Poisson shot noise (higher = less noise; 0 disables).
    pub photons: f64,
}

impl SimConfig {
    /// A clean gel with no effects — useful as a baseline / sanity check.
    pub fn clean(seed: u64) -> Self {
        SimConfig {
            width: 240,
            height: 320,
            gel_type: GelType::Dna,
            ladder_name: "NEB 1 kb DNA Ladder".to_string(),
            n_sample_lanes: 4,
            seed,
            rotation_deg: 0.0,
            smile_px: 0.0,
            wobble_px: 0.0,
            shift_px: (0.0, 0.0),
            background: 0.0,
            exposure: 1.0,
            photons: 0.0,
        }
    }

    /// A randomized, heavily-degraded gel exercising every effect. Deterministic
    /// per `seed`.
    pub fn randomized(seed: u64) -> Self {
        let mut r = Rng::new(seed);
        SimConfig {
            width: 240,
            height: 320,
            gel_type: GelType::Dna,
            ladder_name: "NEB 1 kb DNA Ladder".to_string(),
            n_sample_lanes: 3 + (r.range(0.0, 3.0) as usize),
            seed,
            rotation_deg: r.range(-50.0, 50.0),
            smile_px: r.range(0.0, 18.0),
            wobble_px: r.range(0.0, 8.0),
            shift_px: (r.range(-40.0, 40.0), r.range(-30.0, 30.0)),
            background: r.range(0.05, 0.35),
            exposure: r.range(1.0, 2.5),
            photons: r.range(40.0, 300.0),
        }
    }

    /// Like [`randomized`](Self::randomized) but without rotation — for pure
    /// detection benchmarking where lanes should stay axis-aligned.
    pub fn randomized_upright(seed: u64) -> Self {
        let mut c = Self::randomized(seed);
        c.rotation_deg = 0.0;
        c
    }
}
