//! Simulation configuration, split into three visibly-separate concerns:
//!
//! * [`GelParams`] — everything about the *gel and how it is imaged* (size,
//!   geometry/warp, camera/optics, substrate, band-shape/migration physics) —
//!   but *not* which lanes exist or what they contain.
//! * [`SimLane`] — one lane of the layout, ordered left→right.
//! * [`LaneContent`] — what a lane holds: a named ladder template, or a sample
//!   with explicit band sizes.
//!
//! A [`SimConfig`] is simply the gel parameters plus the ordered lanes.

use crate::core::ladders;
use crate::core::model::GelType;

use crate::sim::rng::Rng;

/// How the gel warp is modeled. Both reproduce the same smile/wobble shape; the
/// difference is the machinery (and, for `Nurbs`, the ability to also warp in x
/// via `warp_2d`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WarpMode {
    /// Closed-form analytic vertical smile/wobble warp (fast, exactly
    /// invertible; the original model).
    SmileWobble,
    /// Tensor-product B-spline (NURBS) warp `S(u,v)->(x,y)`, inverted per pixel
    /// with Newton iteration. The default; required for `warp_2d`.
    #[default]
    Nurbs,
}

/// The gel and how it is imaged: dimensions, geometry/warp, camera/optics,
/// substrate and band-shape/migration physics. Deliberately holds *nothing*
/// about which lanes exist or what they contain — that lives in
/// [`SimConfig::lanes`].
#[derive(Debug, Clone)]
pub struct GelParams {
    // ---- dimensions & identity ----
    pub width: u32,
    pub height: u32,
    /// Master seed: drives every deterministic RNG stream in the renderer.
    pub seed: u64,
    pub gel_type: GelType,

    // ---- geometric effects ----
    /// Whole-gel rotation in degrees (|value| ≤ 50 typically).
    pub rotation_deg: f64,
    /// Quadratic "smile/frown" vertical warp amplitude (px at the edges).
    pub smile_px: f64,
    /// Low-frequency wobble warp amplitude (px).
    pub wobble_px: f64,
    /// Which warp model to use (default [`WarpMode::Nurbs`]). Both produce the
    /// same smile/wobble geometry; `SmileWobble` is the fast analytic path.
    pub warp_mode: WarpMode,
    /// Enable a small lateral (x) perturbation of the B-spline warp control
    /// points so the gel warp becomes genuinely 2-D (lanes bow sideways), rather
    /// than a pure vertical smile/wobble. Off by default: the baseline warp only
    /// displaces control points vertically, reproducing the old smile/wobble.
    pub warp_2d: bool,
    /// Translation (px) — large values run the gel partly out of frame.
    pub shift_px: (f64, f64),

    // ---- camera / optical effects ----
    /// Uneven background amplitude in normalized intensity (0..1).
    pub background: f64,
    /// Exposure gain; > 1 drives bright bands into saturation (overexposure).
    pub exposure: f64,
    /// Photon scale for Poisson shot noise (higher = less noise; 0 disables).
    pub photons: f64,

    // ---- gel substrate / geometry of the gel rectangle ----
    /// Fraction of each dimension used as a dark camera border around the gel
    /// rectangle (the gel is strictly smaller than the frame). ~0.06–0.10.
    pub gel_margin_frac: f64,
    /// Peak amplitude (normalized intensity) of the agarose background
    /// fluorescence *inside* the gel rectangle. This is the dominant look of a
    /// real stained gel — the substrate glows, brightest just below the wells
    /// and fading toward the dye front, so bands sit at fairly low contrast on a
    /// bright cloudy background. Typically 0.25–0.45 for a realistic gel.
    pub fluorescence: f64,
    /// Sparse bright shot-noise specks (dust / detector hot pixels): probability
    /// that a given pixel is a bright speck. ~5e-4 gives a scattering of dots.
    pub speck_density: f64,
    /// Downward DNA smear as a fraction of each band's peak: vertical trailing of
    /// material toward the dye front (0 = crisp bands, ~0.05–0.12 = visible smear).
    pub smear: f64,
    /// Render loading wells (a slot per lane near the top of the gel).
    pub wells: bool,

    // ---- band shape / electrophoresis physics ----
    /// Overall multiplier on band (and smear) brightness. 1.0 leaves the
    /// densitometry mapping as-is; a small value (e.g. ~0.03) makes bands sit at
    /// low contrast over a bright fluorescent background, like a real gel photo.
    pub band_gain: f64,
    /// Band half-width in x as a fraction of half the lane pitch, so bands read
    /// as flat-topped rectangles spanning (most of) the lane. ~0.6–0.8.
    pub lane_width_frac: f64,
    /// Base migration band spread (Gaussian sigma_y, px) at the well, before
    /// diffusion broadening.
    pub band_sigma_y: f64,
    /// Diffusion broadening: extra sigma_y (px) added at the dye front, i.e.
    /// sigma_y grows linearly with migration distance.
    pub diffusion: f64,
    /// Strength (0..1) of the semilog migration non-linearity: large fragments
    /// pile up near the top and small fragments compress toward the dye front.
    pub migration_compression: f64,
}

/// What a single lane contains.
#[derive(Debug, Clone)]
pub enum LaneContent {
    /// A ladder lane reproducing the named built-in template's rungs.
    Ladder(String),
    /// A sample lane with the given band sizes (in the gel-type's unit, e.g. bp).
    Sample(Vec<f64>),
}

/// One lane of the layout (ordered left→right). Currently just its content, but
/// a struct so per-lane attributes (loading, label, ...) can be added later.
#[derive(Debug, Clone)]
pub struct SimLane {
    pub content: LaneContent,
}

impl SimLane {
    pub fn ladder(name: impl Into<String>) -> Self {
        SimLane {
            content: LaneContent::Ladder(name.into()),
        }
    }
    pub fn sample(sizes: Vec<f64>) -> Self {
        SimLane {
            content: LaneContent::Sample(sizes),
        }
    }
    pub fn is_ladder(&self) -> bool {
        matches!(self.content, LaneContent::Ladder(_))
    }
}

/// A full simulation: the gel/optics parameters plus the ordered lanes and their
/// contents.
#[derive(Debug, Clone)]
pub struct SimConfig {
    /// The gel and how it is imaged.
    pub gel: GelParams,
    /// The lane layout (ordered left→right) and what each lane contains.
    pub lanes: Vec<SimLane>,
}

/// Default ladder used to build ladder lanes and the migration calibration.
const DEFAULT_LADDER: &str = "NEB 1 kb DNA Ladder";

/// `(ln_min, ln_max)` of a named ladder's rung sizes (for drawing sample sizes
/// within the resolvable range).
fn ladder_ln_range(name: &str) -> (f64, f64) {
    let ladder = ladders::by_name(name).expect("built-in ladder");
    let mut lo = f64::MAX;
    let mut hi = f64::MIN;
    for b in &ladder.bands {
        lo = lo.min(b.size.ln());
        hi = hi.max(b.size.ln());
    }
    (lo, hi)
}

impl SimConfig {
    /// A clean gel with no effects — useful as a baseline / sanity check. One
    /// ladder lane (index 0) plus four fixed sample lanes.
    pub fn clean(seed: u64) -> Self {
        let gel = GelParams {
            width: 240,
            height: 320,
            seed,
            gel_type: GelType::Dna,
            rotation_deg: 0.0,
            smile_px: 0.0,
            wobble_px: 0.0,
            warp_mode: WarpMode::Nurbs,
            warp_2d: false,
            shift_px: (0.0, 0.0),
            background: 0.0,
            exposure: 1.0,
            photons: 0.0,
            gel_margin_frac: 0.07,
            // "clean" is the ideal sanity baseline: no substrate effects
            // (fluorescence, wells off), sharp bands, no diffusion broadening,
            // and linear semilog migration (compression 0) so every ladder rung
            // is resolved. The realistic effects (fluorescence, wells,
            // diffusion, compression) all live in `randomized`.
            fluorescence: 0.0,
            speck_density: 0.0,
            smear: 0.0,
            wells: false,
            band_gain: 1.0,
            lane_width_frac: 0.7,
            band_sigma_y: 2.2,
            diffusion: 0.0,
            migration_compression: 0.0,
        };
        // Fixed, deterministic sample lanes (within the NEB 1 kb range) so the
        // clean baseline is fully reproducible regardless of seed.
        let lanes = vec![
            SimLane::ladder(DEFAULT_LADDER),
            SimLane::sample(vec![3000.0, 1000.0, 500.0]),
            SimLane::sample(vec![6000.0, 2000.0, 800.0]),
            SimLane::sample(vec![4000.0, 1500.0]),
            SimLane::sample(vec![5000.0, 1200.0, 700.0, 400.0]),
        ];
        SimConfig { gel, lanes }
    }

    /// A randomized, heavily-degraded gel exercising every effect. Deterministic
    /// per `seed`: a ladder lane (index 0) plus N random sample lanes.
    pub fn randomized(seed: u64) -> Self {
        let mut r = Rng::new(seed);
        // Draw the sample-lane count first (as before), then the gel params in
        // their original order, so every per-seed gel parameter is unchanged.
        let n_sample_lanes = 3 + (r.range(0.0, 3.0) as usize);
        let gel = GelParams {
            width: 240,
            height: 320,
            seed,
            gel_type: GelType::Dna,
            rotation_deg: r.range(-50.0, 50.0),
            smile_px: r.range(0.0, 18.0),
            wobble_px: r.range(0.0, 8.0),
            warp_mode: WarpMode::Nurbs,
            // Off by default; no RNG draw here so all other per-seed values are
            // unchanged.
            warp_2d: false,
            shift_px: (r.range(-40.0, 40.0), r.range(-30.0, 30.0)),
            background: r.range(0.05, 0.35),
            exposure: r.range(1.0, 2.5),
            photons: r.range(40.0, 300.0),
            // New draws appended after the existing ones so the pre-existing
            // fields keep their per-seed values (determinism preserved).
            gel_margin_frac: r.range(0.06, 0.10),
            fluorescence: r.range(0.02, 0.06),
            // Fixed (no RNG draw) so the appended lane-size draws below keep
            // their per-seed values and the eval datasets stay stable.
            speck_density: 0.0005,
            smear: 0.04,
            band_gain: 1.0,
            lane_width_frac: r.range(0.6, 0.8),
            band_sigma_y: r.range(2.5, 4.5),
            diffusion: r.range(2.0, 5.0),
            migration_compression: r.range(0.2, 0.5),
            wells: r.unit() < 0.9,
        };
        // Build the lanes: ladder at index 0, then random sample lanes. Sizes
        // are drawn from the tail of the same stream (after every gel param), so
        // the gel parameters above are byte-for-byte the old per-seed values.
        let (ln_min, ln_max) = ladder_ln_range(DEFAULT_LADDER);
        let mut lanes = vec![SimLane::ladder(DEFAULT_LADDER)];
        for _ in 0..n_sample_lanes {
            let n = 2 + (r.range(0.0, 4.0) as usize);
            let sizes = (0..n).map(|_| r.range(ln_min, ln_max).exp()).collect();
            lanes.push(SimLane::sample(sizes));
        }
        SimConfig { gel, lanes }
    }

    /// Like [`randomized`](Self::randomized) but without rotation — for pure
    /// detection benchmarking where lanes should stay axis-aligned.
    pub fn randomized_upright(seed: u64) -> Self {
        let mut c = Self::randomized(seed);
        c.gel.rotation_deg = 0.0;
        c
    }
}
