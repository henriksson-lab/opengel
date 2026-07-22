//! Optical-flow dewarp: recover the gel's low-frequency vertical distortion —
//! the gentle *twisting* of the band fronts across the gel — directly from the
//! image, for the regions *between* ladder lanes where no rung constrains the
//! warp.
//!
//! The "flow" is estimated coarsely on purpose. The image is split into a modest
//! number of wide vertical strips; each strip's (low-pass) densitometry profile
//! is registered against a shared reference by 1-D cross-correlation, giving one
//! vertical shift per strip — how far that part of the gel's bands have twisted.
//! Per-pixel flow would just track speckle; the band deformation lives at low
//! spatial frequency, so a strip is the right granularity.
//!
//! The raw shifts are then fused with a **smoothness energy** (a second-
//! difference penalty) weighted against each strip's registration confidence:
//! confident strips (strong band structure) pull the curve to their measured
//! shift; empty inter-lane strips are interpolated by the energy term. The
//! resulting displacement `dy(x)` becomes the vertical offset of a fine grid of
//! warp control columns.

use crate::core::warp::{solve_linear, GelWarp};
use crate::core::GrayF32;
use crate::detect::signal::smooth;

/// Number of vertical strips the flow is estimated on. Coarse by design.
const STRIPS: usize = 24;
/// Max vertical shift searched, as a fraction of image height.
const MAX_SHIFT_FRAC: f64 = 0.12;

/// Mean vertical intensity profile of the strip `[x0, x1)`, low-pass smoothed.
fn strip_profile(img: &GrayF32, x0: usize, x1: usize) -> Vec<f64> {
    let h = img.height();
    let x1 = x1.min(img.width()).max(x0 + 1);
    let mut p = vec![0.0; h];
    for (y, pv) in p.iter_mut().enumerate() {
        let mut acc = 0.0;
        for x in x0..x1 {
            acc += img.get(x, y) as f64;
        }
        *pv = acc / (x1 - x0) as f64;
    }
    smooth(&p, 2)
}

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

/// Best vertical shift (subpixel) aligning `p` to `reference` within
/// `±max_shift`, and a confidence weight (correlation coefficient × signal
/// energy). A positive shift means `p`'s features sit *below* the reference's.
fn best_shift(p: &[f64], reference: &[f64], max_shift: i64) -> (f64, f64) {
    let n = p.len() as i64;
    let (pm, rm) = (mean(p), mean(reference));
    let var_p: f64 = p.iter().map(|v| (v - pm).powi(2)).sum();
    let var_r: f64 = reference.iter().map(|v| (v - rm).powi(2)).sum();
    let norm = (var_p * var_r).sqrt().max(1e-12);

    // Correlation over the integer shift range.
    let mut corr = vec![0.0; (2 * max_shift + 1) as usize];
    for (k, c) in corr.iter_mut().enumerate() {
        let s = k as i64 - max_shift;
        let mut num = 0.0;
        for y in 0..n {
            let yr = y + s;
            if yr < 0 || yr >= n {
                continue;
            }
            num += (p[y as usize] - pm) * (reference[yr as usize] - rm);
        }
        *c = num / norm;
    }
    // Peak + parabolic subpixel refinement.
    let mut ki = 0usize;
    for k in 1..corr.len() {
        if corr[k] > corr[ki] {
            ki = k;
        }
    }
    let peak = corr[ki];
    let mut shift = (ki as i64 - max_shift) as f64;
    if ki > 0 && ki + 1 < corr.len() {
        let (a, b, c) = (corr[ki - 1], corr[ki], corr[ki + 1]);
        let denom = a - 2.0 * b + c;
        if denom.abs() > 1e-12 {
            shift += 0.5 * (a - c) / denom;
        }
    }
    // Positive shift = p below reference (features at larger y).
    (shift, peak.max(0.0) * var_p)
}

/// Average the strip profiles after shifting each into a common frame, to sharpen
/// the reference for the next iteration.
fn aligned_reference(profiles: &[Vec<f64>], shifts: &[f64]) -> Vec<f64> {
    let n = profiles[0].len();
    let mut acc = vec![0.0; n];
    for (p, &s) in profiles.iter().zip(shifts) {
        let si = s.round() as i64;
        for (y, a) in acc.iter_mut().enumerate() {
            let ys = y as i64 + si; // undo the shift: p[y] matched ref[y+s]
            if ys >= 0 && (ys as usize) < n {
                *a += p[ys as usize];
            }
        }
    }
    for a in acc.iter_mut() {
        *a /= profiles.len() as f64;
    }
    acc
}

/// Weighted smoothing of `d` by minimizing
/// `Σ w_i (f_i − d_i)² + λ Σ (f_{i−1} − 2 f_i + f_{i+1})²`.
/// This is the energy-vs-flow trade-off: `w_i` is per-strip flow confidence,
/// `lambda` the smoothness energy that fills unconfident (inter-lane) strips.
fn smooth_weighted(d: &[f64], w: &[f64], lambda: f64) -> Vec<f64> {
    let n = d.len();
    let mut a = vec![vec![0.0; n]; n];
    let mut b = vec![0.0; n];
    for i in 0..n {
        a[i][i] += w[i];
        b[i] += w[i] * d[i];
    }
    // Second-difference (curvature) penalty.
    for i in 1..n.saturating_sub(1) {
        let idx = [i - 1, i, i + 1];
        let coef = [1.0, -2.0, 1.0];
        for r in 0..3 {
            for c in 0..3 {
                a[idx[r]][idx[c]] += lambda * coef[r] * coef[c];
            }
        }
    }
    solve_linear(a, b)
}

/// Fit an optical-flow warp: estimate the vertical band-twist `dy(x)` and build a
/// fine grid of control columns carrying that offset. `smoothness` is the energy
/// weight balancing flow evidence against a smooth displacement field (larger =
/// stiffer / more interpolation between confident strips).
pub fn fit_flow_warp(work: &GrayF32, width: u32, height: u32, smoothness: f64) -> GelWarp {
    let (w, h) = (width as f64, height as f64);
    let n = STRIPS.min(width as usize / 2).max(4);

    // Strip centers + profiles.
    let mut xs = Vec::with_capacity(n);
    let mut profiles = Vec::with_capacity(n);
    for i in 0..n {
        let x0 = (i as f64 * w / n as f64) as usize;
        let x1 = ((i + 1) as f64 * w / n as f64).ceil() as usize;
        xs.push((x0 as f64 + x1 as f64) / 2.0);
        profiles.push(strip_profile(work, x0, x1));
    }

    // Register each strip against an iteratively-sharpened reference.
    let max_shift = (MAX_SHIFT_FRAC * h) as i64;
    let mut reference = {
        let mut acc = vec![0.0; profiles[0].len()];
        for p in &profiles {
            for (y, a) in acc.iter_mut().enumerate() {
                *a += p[y];
            }
        }
        for a in acc.iter_mut() {
            *a /= n as f64;
        }
        acc
    };
    let mut shifts = vec![0.0; n];
    let mut weights = vec![0.0; n];
    for _ in 0..3 {
        for i in 0..n {
            let (s, wt) = best_shift(&profiles[i], &reference, max_shift);
            shifts[i] = s;
            weights[i] = wt;
        }
        reference = aligned_reference(&profiles, &shifts);
    }

    // Displacement to straighten the gel is the negative of the alignment shift;
    // remove the (weighted) mean so the warp introduces no global drift.
    let wsum: f64 = weights.iter().sum::<f64>().max(1e-12);
    let wmean_shift: f64 = shifts.iter().zip(&weights).map(|(s, w)| s * w).sum::<f64>() / wsum;
    let d: Vec<f64> = shifts.iter().map(|s| -(s - wmean_shift)).collect();

    // Normalize weights to mean 1 so `smoothness` is scale-stable.
    let wmean = mean(&weights).max(1e-12);
    let wn: Vec<f64> = weights.iter().map(|w| w / wmean).collect();
    let dy = smooth_weighted(&d, &wn, smoothness);

    // Build a 2×n grid (offset is uniform in v, so degree-1 in v suffices):
    // column i sits at x = xs[i] and is shifted vertically by dy[i].
    let du = 3.min(n - 1);
    GelWarp::from_grid_with_degree(n, 2, du, 1, |u, v| {
        let f = u * (n - 1) as f64;
        let i0 = (f.floor() as usize).min(n - 1);
        let i1 = (i0 + 1).min(n - 1);
        let frac = f - i0 as f64;
        let x = xs[i0] + (xs[i1] - xs[i0]) * frac;
        let dd = dy[i0] + (dy[i1] - dy[i0]) * frac;
        [x, v * h + dd]
    })
}
