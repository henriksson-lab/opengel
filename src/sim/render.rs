//! Procedural gel renderer.
//!
//! The scene is defined in an ideal ("canonical") coordinate frame with exactly
//! known band positions. A geometric transform (nonlinear vertical warp, then
//! rotation + translation) maps canonical → final image space. Rendering uses
//! *inverse* mapping (for each output pixel, map back to canonical and sample),
//! which handles rotation, warp and run-out-of-frame cropping without
//! re-rasterization artifacts. Ground truth is the *forward*-mapped band
//! centers, so it stays exact under every effect.

use crate::core::ladders;
use crate::core::warp::GelWarp;
use crate::detect::eval::{GroundTruth, GtBand, GtLane};
use image::{DynamicImage, GrayImage, Luma};

use crate::sim::config::SimConfig;
use crate::sim::rng::Rng;

/// A band in canonical space.
struct SceneBand {
    x: f64,
    y: f64,
    size: f64,
    amp: f64,
    lane: usize,
}

/// A lane in canonical space.
struct SceneLane {
    is_ladder: bool,
    ladder_name: Option<String>,
}

/// Geometric transform canonical → final.
struct Xform {
    theta: f64,
    tx: f64,
    ty: f64,
    cx: f64,
    cy: f64,
    smile: f64,
    wobble_amp: f64,
    wobble_freq: f64,
    wobble_phase: f64,
    width: f64,
}

impl Xform {
    /// Vertical warp offset as a function of canonical x (depends on x only, so
    /// it is trivially invertible).
    fn warp(&self, x: f64) -> f64 {
        let t = (x - self.cx) / self.cx.max(1.0);
        self.smile * t * t
            + self.wobble_amp * (x / self.width * std::f64::consts::TAU * self.wobble_freq + self.wobble_phase).sin()
    }

    fn forward(&self, gx: f64, gy: f64) -> (f64, f64) {
        let wx = gx;
        let wy = gy + self.warp(gx);
        let (c, s) = (self.theta.to_radians().cos(), self.theta.to_radians().sin());
        let rx = self.cx + (wx - self.cx) * c - (wy - self.cy) * s;
        let ry = self.cy + (wx - self.cx) * s + (wy - self.cy) * c;
        (rx + self.tx, ry + self.ty)
    }

    fn inverse(&self, fx: f64, fy: f64) -> (f64, f64) {
        let rx = fx - self.tx;
        let ry = fy - self.ty;
        let (c, s) = (self.theta.to_radians().cos(), self.theta.to_radians().sin());
        // R(-theta)
        let wx = self.cx + (rx - self.cx) * c + (ry - self.cy) * s;
        let wy = self.cy - (rx - self.cx) * s + (ry - self.cy) * c;
        let gx = wx;
        let gy = wy - self.warp(wx);
        (gx, gy)
    }
}

/// A rendered gel plus its exact ground truth.
pub struct RenderedGel {
    pub image: DynamicImage,
    pub truth: GroundTruth,
    /// The rotation applied (degrees); auto-straighten should recover ~this.
    pub rotation_deg: f64,
    /// The true canonical→image distortion, baked as a `GelWarp` (the exact
    /// geometry a detector's warp fit should recover).
    pub true_warp: GelWarp,
    pub config: SimConfig,
}

/// Build the canonical scene (lanes + bands) for a config.
fn build_scene(cfg: &SimConfig, r: &mut Rng) -> (Vec<SceneLane>, Vec<SceneBand>) {
    let (w, h) = (cfg.width as f64, cfg.height as f64);
    let ladder = ladders::by_name(&cfg.ladder_name)
        .or_else(|| ladders::for_gel_type(cfg.gel_type).first().copied())
        .expect("a ladder for the gel type");
    let ladder_sizes: Vec<f64> = ladder.bands.iter().map(|b| b.size).collect();
    let ln_max = ladder_sizes.iter().cloned().fold(f64::MIN, f64::max).ln();
    let ln_min = ladder_sizes.iter().cloned().fold(f64::MAX, f64::min).ln();
    let top = 0.08 * h;
    let bot = 0.90 * h;
    let size_to_y = |size: f64| top + (bot - top) * (ln_max - size.ln()) / (ln_max - ln_min).max(1e-6);

    let n_lanes = 1 + cfg.n_sample_lanes;
    let left = 0.12 * w;
    let right = 0.88 * w;
    let spacing = if n_lanes > 1 {
        (right - left) / (n_lanes as f64 - 1.0)
    } else {
        0.0
    };

    let mut lanes = Vec::new();
    let mut bands = Vec::new();
    for li in 0..n_lanes {
        let x = left + spacing * li as f64;
        // Second ladder mid-gel (not the opposite edge): a symmetric edge would
        // have the *same* smile offset as lane 0, leaving no cross-lane signal.
        let is_ladder = li == 0 || (cfg.two_ladders && li == n_lanes / 2);
        lanes.push(SceneLane {
            is_ladder,
            ladder_name: is_ladder.then(|| cfg.ladder_name.clone()),
        });
        if is_ladder {
            for band in &ladder.bands {
                let size = band.size;
                // Reference (extra-thick) bands are rendered brighter.
                let amp = if band.reference {
                    r.range(0.95, 1.2)
                } else {
                    r.range(0.6, 0.85)
                };
                bands.push(SceneBand {
                    x,
                    y: size_to_y(size),
                    size,
                    amp,
                    lane: li,
                });
            }
        } else {
            let n = 2 + (r.range(0.0, 4.0) as usize);
            for _ in 0..n {
                // Random size within the ladder's range.
                let ln = r.range(ln_min, ln_max);
                let size = ln.exp();
                bands.push(SceneBand {
                    x,
                    y: size_to_y(size),
                    size,
                    amp: r.range(0.2, 0.9),
                    lane: li,
                });
            }
        }
    }
    (lanes, bands)
}

const SX: f64 = 5.0;
const SY: f64 = 3.5;

/// Render a gel from a configuration.
pub fn simulate(cfg: &SimConfig) -> RenderedGel {
    let (w, h) = (cfg.width, cfg.height);
    // Separate deterministic stream so scene layout is stable per seed.
    let mut scene_rng = Rng::new(cfg.seed.wrapping_mul(0x9E37_79B9).wrapping_add(1));
    let (lanes, bands) = build_scene(cfg, &mut scene_rng);

    // Background parameters, deterministic per seed.
    let mut bg_rng = Rng::new(cfg.seed.wrapping_mul(0x85EB_CA6B).wrapping_add(7));
    let bg_ax = bg_rng.range(1.0, 3.0) * std::f64::consts::TAU / w as f64;
    let bg_ay = bg_rng.range(1.0, 3.0) * std::f64::consts::TAU / h as f64;
    let bg_phx = bg_rng.range(0.0, std::f64::consts::TAU);
    let bg_phy = bg_rng.range(0.0, std::f64::consts::TAU);

    let xform = Xform {
        theta: cfg.rotation_deg,
        tx: cfg.shift_px.0,
        ty: cfg.shift_px.1,
        cx: (w as f64 - 1.0) / 2.0,
        cy: (h as f64 - 1.0) / 2.0,
        smile: cfg.smile_px,
        wobble_amp: cfg.wobble_px,
        wobble_freq: scene_rng.range(1.5, 3.0),
        wobble_phase: scene_rng.range(0.0, std::f64::consts::TAU),
        width: w as f64,
    };

    // Noise stream (per-pixel, raster order → deterministic).
    let mut noise_rng = Rng::new(cfg.seed.wrapping_mul(0xC2B2_AE35).wrapping_add(13));

    let scene_at = |gx: f64, gy: f64| -> f64 {
        let mut acc = 0.0;
        for b in &bands {
            let dx = (gx - b.x) / SX;
            let dy = (gy - b.y) / SY;
            let d2 = dx * dx + dy * dy;
            if d2 < 25.0 {
                acc += b.amp * (-0.5 * d2).exp();
            }
        }
        acc
    };

    let mut img = GrayImage::new(w, h);
    for fy in 0..h {
        for fx in 0..w {
            let (gx, gy) = xform.inverse(fx as f64, fy as f64);
            let signal = scene_at(gx, gy);
            // Uneven background in final space.
            let bg = cfg.background
                * (0.5 + 0.5 * (fx as f64 * bg_ax + bg_phx).sin() * (fy as f64 * bg_ay + bg_phy).cos());
            // Exposure gain then saturation clip (overexposure).
            let mut val = (signal * cfg.exposure + bg).min(1.0).max(0.0);
            // Poisson shot noise.
            if cfg.photons > 0.0 {
                val = (noise_rng.poisson(val * cfg.photons) / cfg.photons).min(1.0);
            }
            img.put_pixel(fx, fy, Luma([(val * 255.0) as u8]));
        }
    }

    let truth = build_truth(cfg, &lanes, &bands, &xform);
    let true_warp = bake_warp(&xform, w, h);
    RenderedGel {
        image: DynamicImage::ImageLuma8(img),
        truth,
        rotation_deg: cfg.rotation_deg,
        true_warp,
        config: cfg.clone(),
    }
}

/// Bake the canonical→image [`Xform`] into a `GelWarp`: sample a dense grid over
/// the canonical gel `(u, v) → (u·w, v·h)` and forward-map each node. A degree-1
/// grid interpolates its nodes exactly, so the smile/wobble/rotation is captured
/// faithfully (piecewise-linear between the dense samples).
fn bake_warp(xform: &Xform, width: u32, height: u32) -> GelWarp {
    let (w, h) = (width as f64, height as f64);
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
    let (w, h) = (cfg.width as f64, cfg.height as f64);
    let mut gt_lanes: Vec<GtLane> = Vec::new();
    for (li, lane) in lanes.iter().enumerate() {
        let mut gt_bands = Vec::new();
        let mut xs: Vec<f64> = Vec::new();
        for b in bands.iter().filter(|b| b.lane == li) {
            let (fx, fy) = xform.forward(b.x, b.y);
            if fx < 0.0 || fy < 0.0 || fx >= w || fy >= h {
                continue; // ran out of frame
            }
            gt_bands.push(GtBand {
                y_center: fy,
                size: Some(b.size),
                // Canonical (smile-free) migration: the band's y before warping,
                // normalized by the gel height.
                v_true: Some(b.y / h),
            });
            xs.push(fx);
        }
        if gt_bands.is_empty() {
            continue;
        }
        let x_min = xs.iter().cloned().fold(f64::MAX, f64::min) - 2.0 * SX;
        let x_max = xs.iter().cloned().fold(f64::MIN, f64::max) + 2.0 * SX;
        gt_lanes.push(GtLane {
            x_min: x_min.max(0.0) as u32,
            x_max: (x_max.min(w)) as u32,
            is_ladder: lane.is_ladder,
            ladder_name: lane.ladder_name.clone(),
            bands: gt_bands,
        });
    }
    GroundTruth {
        image: format!("sim_{:08}.png", cfg.seed),
        gel_type: cfg.gel_type,
        lanes: gt_lanes,
    }
}
