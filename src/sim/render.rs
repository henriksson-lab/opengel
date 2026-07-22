//! Procedural gel renderer.
//!
//! The scene is defined in an ideal ("canonical") coordinate frame with exactly
//! known band positions. A geometric transform (a tensor-product B-spline gel
//! warp `S(u,v) -> (x,y)`, then rotation + translation) maps canonical → final
//! image space. Rendering uses *inverse* mapping (for each output pixel, map
//! back to canonical and sample), which handles rotation, warp and
//! run-out-of-frame cropping without re-rasterization artifacts. Ground truth is
//! the *forward*-mapped band centers, so it stays exact under every effect.

use crate::core::ladders;
use crate::core::warp::GelWarp;
use crate::detect::eval::{GroundTruth, GtBand, GtLane};
use image::{DynamicImage, GrayImage, Luma};

use crate::sim::config::{LaneContent, SimConfig, WarpMode};
use crate::sim::rng::Rng;

/// A band in canonical space. Rendered as a flat-topped rectangle across the
/// lane width (super-Gaussian in x) with a Gaussian migration profile in y.
struct SceneBand {
    x: f64,
    y: f64,
    size: f64,
    /// Peak brightness = loaded mass / band area (densitometry): a fixed mass in
    /// a broader (more diffuse) band is dimmer.
    peak: f64,
    /// Lane half-width in x (flat-top radius of the super-Gaussian).
    half_x: f64,
    /// Migration-broadened vertical spread (grows with migration distance).
    sigma_y: f64,
    lane: usize,
}

/// A lane in canonical space.
struct SceneLane {
    is_ladder: bool,
    ladder_name: Option<String>,
    /// Lane center x (canonical).
    x: f64,
    /// Lane half-width in x.
    half_x: f64,
    /// Brightness of the residual un-migrated material stuck in the well.
    well_residual: f64,
}

/// The old analytic smile/wobble vertical offset as a function of canonical `x`.
/// Retained only to *shape* the B-spline warp's control points so the new warp
/// reproduces the previous behavior; it is no longer evaluated per pixel.
fn smile_wobble_offset(
    x: f64,
    cx: f64,
    width: f64,
    smile: f64,
    wobble_amp: f64,
    wobble_freq: f64,
    wobble_phase: f64,
) -> f64 {
    let t = (x - cx) / cx.max(1.0);
    smile * t * t
        + wobble_amp * (x / width * std::f64::consts::TAU * wobble_freq + wobble_phase).sin()
}

/// Number of B-spline control points along each axis of the gel warp. `NU` is
/// generous along the lane axis so the wobble (up to a few cycles across the
/// width) is reproduced with little attenuation; `NV = 4` makes the migration
/// (v) axis reproduce the linear `v -> y` map exactly (regular lattice = Greville
/// abscissae), so band y-positions match the old renderer.
const WARP_NU: usize = 25;
const WARP_NV: usize = 4;
/// Amplitude (px) of the optional lateral warp perturbation (`warp_2d`).
const WARP_X_PERTURB: f64 = 5.0;

/// Build the gel warp from the smile/wobble knobs: start from the identity
/// lattice over the canonical rectangle `[0,W] × [0,H]`, then displace every
/// control column vertically by `smile_wobble_offset(x_column)`. Iso-v curves
/// then bow exactly like the old smile/wobble. When `warp_2d` is set, a small
/// lateral perturbation (zero at top/bottom, peaking mid-gel) is added so the
/// warp is genuinely 2-D.
fn build_warp(
    w: f64,
    h: f64,
    smile: f64,
    wobble_amp: f64,
    wobble_freq: f64,
    wobble_phase: f64,
    warp_2d: bool,
) -> GelWarp {
    let cx = (w - 1.0) / 2.0;
    let mut warp = GelWarp::identity_grid(w, h, WARP_NU, WARP_NV);
    let (gnu, gnv) = warp.grid_size();
    let vdenom = (gnv.max(2) - 1) as f64;
    for iu in 0..gnu {
        // x of this control column (identity lattice: unchanged by the warp).
        let (xcol, _) = warp.control_point(iu, 0);
        let dy = smile_wobble_offset(xcol, cx, w, smile, wobble_amp, wobble_freq, wobble_phase);
        for iv in 0..gnv {
            let (x, y) = warp.control_point(iu, iv);
            let dx = if warp_2d {
                let vv = iv as f64 / vdenom;
                WARP_X_PERTURB * (std::f64::consts::PI * vv).sin() * (x / w - 0.5)
            } else {
                0.0
            };
            warp.set_control_point(iu, iv, x + dx, y + dy);
        }
    }
    warp
}

/// Geometric transform canonical → final: the B-spline gel warp `S(u,v)->(x,y)`
/// (u along lanes, v along migration; the canonical rectangle maps to `[0,1]²`)
/// followed by rotation about `(cx, cy)` and translation.
struct Xform {
    theta: f64,
    tx: f64,
    ty: f64,
    cx: f64,
    cy: f64,
    /// GelWarp model in effect.
    mode: WarpMode,
    /// B-spline warp (used when `mode == Nurbs`).
    warp: GelWarp,
    /// Smile/wobble parameters (used when `mode == SmileWobble` for the closed-
    /// form path; the `Nurbs` warp was built from these same values).
    smile: f64,
    wobble_amp: f64,
    wobble_freq: f64,
    wobble_phase: f64,
    width: f64,
    height: f64,
}

impl Xform {
    /// Canonical (gx,gy) → warped-gel space (before rotation/translation).
    fn warp_forward(&self, gx: f64, gy: f64) -> (f64, f64) {
        match self.mode {
            WarpMode::Nurbs => self.warp.eval(gx / self.width, gy / self.height),
            WarpMode::SmileWobble => {
                let dy = smile_wobble_offset(
                    gx,
                    self.cx,
                    self.width,
                    self.smile,
                    self.wobble_amp,
                    self.wobble_freq,
                    self.wobble_phase,
                );
                (gx, gy + dy)
            }
        }
    }

    /// Warped-gel space (wx,wy) → canonical (gx,gy).
    fn warp_inverse(&self, wx: f64, wy: f64) -> (f64, f64) {
        match self.mode {
            WarpMode::Nurbs => {
                let (u, v) = self.warp.invert(wx, wy);
                (u * self.width, v * self.height)
            }
            WarpMode::SmileWobble => {
                // Δy depends on x only → closed-form, exact inverse.
                let dy = smile_wobble_offset(
                    wx,
                    self.cx,
                    self.width,
                    self.smile,
                    self.wobble_amp,
                    self.wobble_freq,
                    self.wobble_phase,
                );
                (wx, wy - dy)
            }
        }
    }

    fn forward(&self, gx: f64, gy: f64) -> (f64, f64) {
        let (wx, wy) = self.warp_forward(gx, gy);
        let (c, s) = (self.theta.to_radians().cos(), self.theta.to_radians().sin());
        let rx = self.cx + (wx - self.cx) * c - (wy - self.cy) * s;
        let ry = self.cy + (wx - self.cx) * s + (wy - self.cy) * c;
        (rx + self.tx, ry + self.ty)
    }

    fn inverse(&self, fx: f64, fy: f64) -> (f64, f64) {
        let rx = fx - self.tx;
        let ry = fy - self.ty;
        let (c, s) = (self.theta.to_radians().cos(), self.theta.to_radians().sin());
        // R(-theta): undo rotation + translation to reach warped-gel space.
        let wx = self.cx + (rx - self.cx) * c + (ry - self.cy) * s;
        let wy = self.cy - (rx - self.cx) * s + (ry - self.cy) * c;
        self.warp_inverse(wx, wy)
    }
}

/// A rendered gel plus its exact ground truth.
pub struct RenderedGel {
    pub image: DynamicImage,
    pub truth: GroundTruth,
    /// The rotation applied (degrees); auto-straighten should recover ~this.
    pub rotation_deg: f64,
    /// The true canonical→image gel warp, as a `GelWarp` (the exact geometry a
    /// detector's warp fit should recover).
    pub true_warp: GelWarp,
    pub config: SimConfig,
}

/// Super-Gaussian order for the flat-topped band/well profile in x. The actual
/// exponent applied to (dx/half_x) is `2 * SUPER_GAUSS_P`, so p≈3 gives a plateau
/// across the lane with soft edges rather than a round blob.
const SUPER_GAUSS_P: i32 = 3;

/// Densitometry constant relating loaded mass to peak brightness:
/// `peak = MASS_TO_PEAK * mass / (half_x * sigma_y)`. Tuned so typical ladder
/// rungs land around 0.3–0.9 in normalized intensity before exposure gain.
const MASS_TO_PEAK: f64 = 25.0;

/// Vertical layout of the gel derived from the margins: the loading-well row and
/// the migration band `[top, bot]` — all strictly inside the gel rectangle.
struct GelLayout {
    /// Canonical y of the loading-well row.
    well_y: f64,
    /// Canonical y where the largest fragment sits (just below the wells).
    top: f64,
    /// Canonical y of the dye front (smallest fragment), near the gel bottom.
    bot: f64,
}

fn gel_layout(cfg: &SimConfig) -> GelLayout {
    let h = cfg.gel.height as f64;
    let gel_top = cfg.gel.gel_margin_frac * h;
    let gel_bot = h - cfg.gel.gel_margin_frac * h;
    let well_y = gel_top + 0.045 * h;
    GelLayout {
        well_y,
        top: well_y + 0.02 * h,
        bot: gel_bot - 0.03 * h,
    }
}

/// The reference ladder for a config: the first ladder lane's template, else a
/// default for the gel type. Used only for the migration (size→y) calibration.
fn reference_ladder(cfg: &SimConfig) -> &'static crate::core::model::LadderTemplate {
    cfg.lanes
        .iter()
        .find_map(|l| match &l.content {
            LaneContent::Ladder(name) => ladders::by_name(name),
            LaneContent::Sample(_) => None,
        })
        .or_else(|| ladders::for_gel_type(cfg.gel.gel_type).first().copied())
        .expect("a ladder for the gel type")
}

/// Build the canonical scene (lanes + bands) for a config, reading the lane
/// layout and each lane's content from `cfg.lanes`.
fn build_scene(cfg: &SimConfig, r: &mut Rng) -> (Vec<SceneLane>, Vec<SceneBand>) {
    let w = cfg.gel.width as f64;
    // Migration calibration uses the reference ladder's size range.
    let ref_ladder = reference_ladder(cfg);
    let ladder_sizes: Vec<f64> = ref_ladder.bands.iter().map(|b| b.size).collect();
    let ln_max = ladder_sizes.iter().cloned().fold(f64::MIN, f64::max).ln();
    let ln_min = ladder_sizes.iter().cloned().fold(f64::MAX, f64::min).ln();

    let layout = gel_layout(cfg);
    let (top, bot) = (layout.top, layout.bot);
    // Semilog migration with gentle non-linearity: u∈[0,1] is the raw semilog
    // coordinate (0 = largest fragment near the top, 1 = smallest near the dye
    // front). A smoothstep blend compresses both extremes — big fragments pile
    // up at the top, small ones approach the dye-front asymptote — while staying
    // strictly monotonic, so the y we return is exact ground truth.
    let c = cfg.gel.migration_compression.clamp(0.0, 1.0);
    let size_to_y = |size: f64| {
        let u = ((ln_max - size.ln()) / (ln_max - ln_min).max(1e-6)).clamp(0.0, 1.0);
        let ss = u * u * (3.0 - 2.0 * u);
        let f = (1.0 - c) * u + c * ss;
        top + (bot - top) * f
    };
    // Diffusion broadening: sigma_y grows linearly with migration distance.
    let sigma_at = |y: f64| {
        let run = ((y - top) / (bot - top).max(1e-6)).clamp(0.0, 1.0);
        cfg.gel.band_sigma_y + cfg.gel.diffusion * run
    };

    // Lane geometry: lanes are spread across the gel rectangle (not the whole
    // frame) with an edge gap wide enough that a band's half-width never pokes
    // out of the gel. edge_factor > lane_width_frac/2 guarantees that margin.
    let n_lanes = cfg.lanes.len();
    let gel_left = cfg.gel.gel_margin_frac * w;
    let gel_right = w - cfg.gel.gel_margin_frac * w;
    let gel_w = (gel_right - gel_left).max(1.0);
    let lwf = cfg.gel.lane_width_frac.clamp(0.2, 0.95);
    let edge_factor = lwf * 0.5 + 0.08;
    let (spacing, first_x) = if n_lanes > 1 {
        let sp = gel_w / (n_lanes as f64 - 1.0 + 2.0 * edge_factor);
        (sp, gel_left + edge_factor * sp)
    } else {
        (gel_w, gel_left + gel_w * 0.5)
    };
    let half_x = (lwf * 0.5 * spacing).max(2.0);

    let mut lanes = Vec::new();
    let mut bands = Vec::new();
    for (li, lane) in cfg.lanes.iter().enumerate() {
        let x = first_x + spacing * li as f64;
        // Some lanes retain a bright residual line of un-migrated material in
        // the well; others show little.
        let well_residual = if r.unit() < 0.7 { r.range(0.3, 0.7) } else { r.range(0.0, 0.15) };
        let (is_ladder, ladder_name) = match &lane.content {
            LaneContent::Ladder(name) => (true, Some(name.clone())),
            LaneContent::Sample(_) => (false, None),
        };
        lanes.push(SceneLane { is_ladder, ladder_name, x, half_x, well_residual });
        let band_gain = cfg.gel.band_gain;
        let push_band = |size: f64, mass: f64, bands: &mut Vec<SceneBand>| {
            let y = size_to_y(size);
            let sigma_y = sigma_at(y);
            // Densitometry: peak brightness = mass / band area (∝ half_x·sigma_y),
            // times the overall band gain (low for a realistic low-contrast look).
            let peak = MASS_TO_PEAK * mass / (half_x * sigma_y) * band_gain;
            bands.push(SceneBand { x, y, size, peak, half_x, sigma_y, lane: li });
        };
        match &lane.content {
            LaneContent::Ladder(name) => {
                let ladder = ladders::by_name(name)
                    .or_else(|| ladders::for_gel_type(cfg.gel.gel_type).first().copied())
                    .expect("a ladder for the gel type");
                for band in &ladder.bands {
                    // Prefer the template's loaded mass; reference (extra-thick)
                    // rungs are heavier/brighter, otherwise fall back to a
                    // plausible mass so intensity tracks amount, not a random amp.
                    let mass = band.mass_ng.unwrap_or_else(|| {
                        if band.reference { r.range(80.0, 120.0) } else { r.range(30.0, 55.0) }
                    });
                    push_band(band.size, mass, &mut bands);
                }
            }
            LaneContent::Sample(sizes) => {
                for &size in sizes {
                    let mass = r.range(15.0, 130.0);
                    push_band(size, mass, &mut bands);
                }
            }
        }
    }
    (lanes, bands)
}

/// Render a gel from a configuration.
pub fn simulate(cfg: &SimConfig) -> RenderedGel {
    let (w, h) = (cfg.gel.width, cfg.gel.height);
    // Separate deterministic stream so scene layout is stable per seed.
    let mut scene_rng = Rng::new(cfg.gel.seed.wrapping_mul(0x9E37_79B9).wrapping_add(1));
    let (lanes, bands) = build_scene(cfg, &mut scene_rng);

    // Background parameters, deterministic per seed. The camera-background draws
    // come first so they are unchanged; the fluorescence draws are appended.
    let mut bg_rng = Rng::new(cfg.gel.seed.wrapping_mul(0x85EB_CA6B).wrapping_add(7));
    let bg_ax = bg_rng.range(1.0, 3.0) * std::f64::consts::TAU / w as f64;
    let bg_ay = bg_rng.range(1.0, 3.0) * std::f64::consts::TAU / h as f64;
    let bg_phx = bg_rng.range(0.0, std::f64::consts::TAU);
    let bg_phy = bg_rng.range(0.0, std::f64::consts::TAU);
    // Dim agarose fluorescence: a separate low-frequency pattern in *canonical*
    // space (so it belongs to the gel and rides along under the warp/rotation).
    let fl_ax = bg_rng.range(1.0, 2.5) * std::f64::consts::TAU / w as f64;
    let fl_ay = bg_rng.range(1.0, 2.5) * std::f64::consts::TAU / h as f64;
    let fl_phx = bg_rng.range(0.0, std::f64::consts::TAU);
    let fl_phy = bg_rng.range(0.0, std::f64::consts::TAU);

    // Draw the wobble shape parameters in the SAME order as before (from the
    // scene stream) so per-seed output is unchanged, then bake smile/wobble into
    // the B-spline warp's control points.
    let wobble_freq = scene_rng.range(1.5, 3.0);
    let wobble_phase = scene_rng.range(0.0, std::f64::consts::TAU);
    let warp = build_warp(
        w as f64,
        h as f64,
        cfg.gel.smile_px,
        cfg.gel.wobble_px,
        wobble_freq,
        wobble_phase,
        cfg.gel.warp_2d,
    );
    let xform = Xform {
        theta: cfg.gel.rotation_deg,
        tx: cfg.gel.shift_px.0,
        ty: cfg.gel.shift_px.1,
        cx: (w as f64 - 1.0) / 2.0,
        cy: (h as f64 - 1.0) / 2.0,
        mode: cfg.gel.warp_mode,
        warp,
        smile: cfg.gel.smile_px,
        wobble_amp: cfg.gel.wobble_px,
        wobble_freq,
        wobble_phase,
        width: w as f64,
        height: h as f64,
    };

    // Noise stream (per-pixel, raster order → deterministic).
    let mut noise_rng = Rng::new(cfg.gel.seed.wrapping_mul(0xC2B2_AE35).wrapping_add(13));

    let layout = gel_layout(cfg);
    // Gel rectangle in *canonical* space. Testing membership here (rather than in
    // final space) is clean given the inverse-mapping renderer: the same soft
    // rectangle is what the warp/rotation carries into final image space, so the
    // dark camera border and the gel fluorescence line up with the warped gel.
    let gel_left = cfg.gel.gel_margin_frac * w as f64;
    let gel_right = w as f64 - cfg.gel.gel_margin_frac * w as f64;
    let gel_top = cfg.gel.gel_margin_frac * h as f64;
    let gel_bot = h as f64 - cfg.gel.gel_margin_frac * h as f64;
    let edge_soft = 3.0; // px of soft falloff at the gel edge

    // Flat-topped super-Gaussian window in x: ≈1 across [-hw, hw], soft edges.
    let super_x = |d: f64, hw: f64| -> f64 {
        let u = d / hw.max(1e-6);
        (-(u * u).powi(SUPER_GAUSS_P)).exp()
    };
    // Smooth 0→1 ramp (edge0 → edge1 may run either direction).
    let ramp = |edge0: f64, edge1: f64, x: f64| -> f64 {
        let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    // Soft gel mask (1 inside the gel rectangle, 0 outside) in canonical space.
    let gel_mask = |gx: f64, gy: f64| -> f64 {
        ramp(gel_left, gel_left + edge_soft, gx)
            * ramp(gel_right, gel_right - edge_soft, gx)
            * ramp(gel_top, gel_top + edge_soft, gy)
            * ramp(gel_bot, gel_bot - edge_soft, gy)
    };

    let smear = cfg.gel.smear;
    let scene_at = |gx: f64, gy: f64| -> f64 {
        let mut acc = 0.0;
        for b in &bands {
            let dx = gx - b.x;
            if dx.abs() > b.half_x * 1.9 {
                continue;
            }
            let dy = gy - b.y;
            let smear_len = b.sigma_y * 20.0;
            if dy < -b.sigma_y * 5.0 || dy > smear_len {
                continue;
            }
            // Flat-top (super-Gaussian) across the lane.
            let sx = super_x(dx, b.half_x);
            // Main Gaussian migration band.
            if dy.abs() <= b.sigma_y * 5.0 {
                acc += b.peak * sx * (-0.5 * (dy / b.sigma_y).powi(2)).exp();
            }
            // Downward DNA smear: a faint one-sided exponential trailing toward
            // the dye front, giving the vertical streaking of a real lane.
            if smear > 0.0 && dy > 0.0 {
                acc += b.peak * smear * sx * (-(dy / (b.sigma_y * 7.0))).exp();
            }
        }
        acc
    };

    // Loading wells: a dark rectangular slot per lane with a thin darker outline
    // and an optional bright residual line of un-migrated material. Rendered in
    // canonical space so they warp with the gel; purely a visual feature (not in
    // ground truth).
    let wells_at = |gx: f64, gy: f64| -> f64 {
        if !cfg.gel.wells {
            return 0.0;
        }
        let well_hh = 3.0; // slot half-height (px)
        let mut acc = 0.0;
        for l in &lanes {
            let dx = gx - l.x;
            let dyw = gy - layout.well_y;
            if dx.abs() > l.half_x * 1.6 || dyw.abs() > well_hh * 2.5 {
                continue;
            }
            // Slot interior and a slightly larger frame → a darker outline ring.
            let inner = super_x(dx, l.half_x * 0.9) * super_x(dyw, well_hh * 0.7);
            let outer = super_x(dx, l.half_x * 1.02) * super_x(dyw, well_hh);
            let frame = (outer - inner).max(0.0);
            acc -= 0.05 * inner; // slot slightly darker than the gel
            acc -= 0.10 * frame; // darker outline
            // Bright residual un-migrated material stuck at the well.
            acc += l.well_residual * super_x(dx, l.half_x * 0.85) * (-0.5 * (dyw / 1.0).powi(2)).exp();
        }
        acc
    };

    // Sparse bright specks (dust / detector hot pixels), on their own RNG stream
    // so toggling them never perturbs the Poisson-noise stream.
    let mut speck_rng = Rng::new(cfg.gel.seed.wrapping_mul(0x27D4_EB2F).wrapping_add(17));

    let mut img = GrayImage::new(w, h);
    for fy in 0..h {
        for fx in 0..w {
            let (gx, gy) = xform.inverse(fx as f64, fy as f64);
            let m = gel_mask(gx, gy);
            // Agarose autofluorescence: the dominant look of a stained gel — the
            // substrate glows, brightest just below the wells and fading toward
            // the dye front, times a low-frequency unevenness (a cloudy glow).
            let gyn = ((gy - gel_top) / (gel_bot - gel_top).max(1.0)).clamp(0.0, 1.0);
            let grad = 1.0 - 0.4 * gyn;
            let uneven = 0.78 + 0.22 * (gx * fl_ax + fl_phx).sin() * (gy * fl_ay + fl_phy).cos();
            let fluor = cfg.gel.fluorescence * grad * uneven;
            let gel_signal = (scene_at(gx, gy) + wells_at(gx, gy) + fluor) * m;
            // Uneven (darker) camera background, present across the whole frame.
            let bg = cfg.gel.background
                * (0.5 + 0.5 * (fx as f64 * bg_ax + bg_phx).sin() * (fy as f64 * bg_ay + bg_phy).cos());
            // Exposure gain on gel emission, then saturation clip (overexposure).
            let mut val = (gel_signal * cfg.gel.exposure + bg).min(1.0).max(0.0);
            // Poisson shot noise.
            if cfg.gel.photons > 0.0 {
                val = (noise_rng.poisson(val * cfg.gel.photons) / cfg.gel.photons).min(1.0);
            }
            // Sparse bright specks scattered across the frame.
            if cfg.gel.speck_density > 0.0 && speck_rng.unit() < cfg.gel.speck_density {
                val = (val + speck_rng.range(0.4, 1.0)).min(1.0);
            }
            img.put_pixel(fx, fy, Luma([(val * 255.0) as u8]));
        }
    }

    let truth = build_truth(cfg, &lanes, &bands, &xform);
    RenderedGel {
        image: DynamicImage::ImageLuma8(img),
        truth,
        rotation_deg: cfg.gel.rotation_deg,
        // The full canonical→image transform (warp + rotation + translation),
        // baked as a dense degree-1 GelWarp — the exact geometry a detector's
        // warp fit should recover.
        true_warp: bake_true_warp(&xform, w as f64, h as f64),
        config: cfg.clone(),
    }
}

/// Sample the full canonical→image [`Xform`] on a dense grid and interpolate it
/// as a degree-1 `GelWarp` (exact at the sampled nodes).
fn bake_true_warp(xform: &Xform, w: f64, h: f64) -> GelWarp {
    GelWarp::from_grid_with_degree(16, 16, 1, 1, |u, v| {
        let (fx, fy) = xform.forward(u * w, v * h);
        [fx, fy]
    })
}

/// Forward-map band centers to build image-space ground truth. Bands mapped
/// outside the image (run out of frame) are dropped from the truth.
fn build_truth(
    cfg: &SimConfig,
    lanes: &[SceneLane],
    bands: &[SceneBand],
    xform: &Xform,
) -> GroundTruth {
    let (w, h) = (cfg.gel.width as f64, cfg.gel.height as f64);
    let mut gt_lanes: Vec<GtLane> = Vec::new();
    for (li, lane) in lanes.iter().enumerate() {
        let mut gt_bands = Vec::new();
        let mut xs: Vec<f64> = Vec::new();
        let mut half_x = 5.0;
        for b in bands.iter().filter(|b| b.lane == li) {
            let (fx, fy) = xform.forward(b.x, b.y);
            if fx < 0.0 || fy < 0.0 || fx >= w || fy >= h {
                continue; // ran out of frame
            }
            half_x = b.half_x;
            gt_bands.push(GtBand {
                y_center: fy,
                size: Some(b.size),
                // Canonical (smile-free) migration, normalized by gel height.
                v_true: Some(b.y / h),
            });
            xs.push(fx);
        }
        if gt_bands.is_empty() {
            continue;
        }
        let x_min = xs.iter().cloned().fold(f64::MAX, f64::min) - half_x;
        let x_max = xs.iter().cloned().fold(f64::MIN, f64::max) + half_x;
        gt_lanes.push(GtLane {
            x_min: x_min.max(0.0) as u32,
            x_max: (x_max.min(w)) as u32,
            is_ladder: lane.is_ladder,
            ladder_name: lane.ladder_name.clone(),
            bands: gt_bands,
        });
    }
    GroundTruth {
        image: format!("sim_{:08}.png", cfg.gel.seed),
        gel_type: cfg.gel.gel_type,
        lanes: gt_lanes,
    }
}
