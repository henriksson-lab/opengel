//! Classical densitometry detector: the gel-analysis workhorse.
//!
//! 1. **Lane finding** — sum intensity down each column to get a horizontal
//!    profile; lanes are the peaks, bounded by the valleys between them.
//! 2. **Band finding** — within each lane, sum intensity across the lane width
//!    to get a vertical densitometry trace; subtract a rolling-ball baseline;
//!    detect and integrate peaks as bands.

use crate::core::GrayF32;

use crate::detect::detector::{DetBand, DetLane, DetectParams, Detection, GelDetector};
use crate::detect::signal::{find_peaks, smooth, subtract_baseline};

pub struct ClassicalDetector;

impl ClassicalDetector {
    pub fn new() -> Self {
        ClassicalDetector
    }
}

impl Default for ClassicalDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl GelDetector for ClassicalDetector {
    fn name(&self) -> &str {
        "classical"
    }

    fn detect(&self, img: &GrayF32, params: &DetectParams) -> Detection {
        let work = if params.signal_is_bright {
            img.clone()
        } else {
            img.inverted()
        };
        let lanes = detect_lanes(&work, params);

        let mut bands = Vec::new();
        let mut band_id = 0u32;
        for lane in &lanes {
            let lane_bands = detect_bands_in_lane(&work, lane, params);
            // Solve every band's tilt at once as a per-lane Gaussian mixture, and
            // get one shared box half-height for the lane (see
            // `fit_lane_gmm_angles`): adjacent bands share pixels by
            // responsibility, and all bands share width (the lane) and height (the
            // pooled Σyy) so a faint band can't get a spurious size or angle.
            let angles = fit_lane_gmm_angles(&work, lane, &lane_bands);
            // Shared box height = the lane's MEDIAN 1-D peak half-width. This is
            // robust for faint lanes (unlike the mixture's pooled Σyy, which
            // inflates when sparse bright pixels + diffuse glow spread the
            // Gaussians), so every box in a lane gets the same sensible thickness.
            let mut halves: Vec<f64> = lane_bands.iter().map(|b| b.1).collect();
            halves.sort_by(|p, q| p.partial_cmp(q).unwrap());
            let half_h = halves
                .get(halves.len() / 2)
                .copied()
                .unwrap_or(1.0)
                .max(0.75);
            for (&(y_center, _y_half, area), &angle) in lane_bands.iter().zip(&angles) {
                bands.push(DetBand {
                    id: band_id,
                    lane_id: lane.id,
                    x_center: lane.x_center(),
                    y_center,
                    y_half_width: half_h,
                    integrated_density: area,
                    angle,
                });
                band_id += 1;
            }
        }
        // Finish "handle 0°" locally: within each lane, a band the mixture
        // couldn't orient (0°) or a rare outlier borrows that lane's own robust
        // tilt; confident bands keep their measured angle, and a lane with no
        // confident band is left as-is (no cross-lane trend is imposed).
        fill_undetermined_angles(&mut bands);
        Detection { lanes, bands }
    }
}

/// Column-sum profile: `profile[x] = sum over y of intensity`.
pub fn column_profile(img: &GrayF32) -> Vec<f64> {
    let (w, h) = (img.width(), img.height());
    let mut profile = vec![0.0; w];
    for y in 0..h {
        for (x, px) in profile.iter_mut().enumerate().take(w) {
            *px += img.get(x, y) as f64;
        }
    }
    profile
}

/// Row-sum profile over `[x_min, x_max)`: `profile[y] = sum over x`.
pub fn lane_row_profile(img: &GrayF32, x_min: usize, x_max: usize) -> Vec<f64> {
    let h = img.height();
    let x_max = x_max.min(img.width());
    let mut profile = vec![0.0; h];
    for (y, py) in profile.iter_mut().enumerate().take(h) {
        let mut acc = 0.0;
        for x in x_min..x_max {
            acc += img.get(x, y) as f64;
        }
        *py = acc;
    }
    profile
}

fn detect_lanes(img: &GrayF32, params: &DetectParams) -> Vec<DetLane> {
    let raw = column_profile(img);
    let profile = smooth(&raw, params.column_smooth);
    let corrected = subtract_baseline(&profile, params.baseline_radius);
    let profile = if corrected.iter().any(|&v| v > 0.0) {
        corrected
    } else {
        profile
    };
    let max = profile.iter().cloned().fold(0.0f64, f64::max);
    if max <= 0.0 {
        return Vec::new();
    }
    let min_prom = params.lane_min_rel_prominence * max;
    let peaks = find_peaks(&profile, min_prom, params.min_lane_distance);
    let regions = lane_regions(&profile, min_prom);
    let mut lanes = if regions.len() > peaks.len() {
        regions
    } else {
        peaks
            .into_iter()
            .map(|p| LaneRegion {
                left: p.left,
                right: p.right,
                score: p.prominence,
            })
            .collect()
    };

    // Keep only the requested number of lanes, most prominent first.
    if let Some(n) = params.expected_lanes {
        lanes.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        lanes.truncate(n);
        lanes.sort_by(|a, b| a.left.cmp(&b.left));
    }

    let h = img.height() as u32;
    lanes
        .into_iter()
        .enumerate()
        .map(|(i, r)| DetLane {
            id: i as u32,
            x_min: r.left as u32,
            x_max: (r.right as u32 + 1).min(img.width() as u32),
            y_min: 0,
            y_max: h,
            is_ladder: false,
        })
        .collect()
}

#[derive(Debug, Clone)]
struct LaneRegion {
    left: usize,
    right: usize,
    score: f64,
}

fn lane_regions(profile: &[f64], threshold: f64) -> Vec<LaneRegion> {
    let mut regions = Vec::new();
    let mut start = None;
    for (i, &v) in profile.iter().enumerate() {
        if v >= threshold {
            start.get_or_insert(i);
        } else if let Some(left) = start.take() {
            push_lane_region(profile, left, i.saturating_sub(1), &mut regions);
        }
    }
    if let Some(left) = start {
        push_lane_region(profile, left, profile.len().saturating_sub(1), &mut regions);
    }
    regions
}

fn push_lane_region(profile: &[f64], left: usize, right: usize, regions: &mut Vec<LaneRegion>) {
    if right <= left {
        return;
    }
    let score = profile[left..=right].iter().cloned().fold(0.0f64, f64::max);
    regions.push(LaneRegion { left, right, score });
}

/// Handle undetermined/outlier tilts **within each lane** — never across lanes.
/// A band the mixture couldn't orient (≈0°) or one that disagrees sharply with
/// its lane-mates borrows that lane's own robust tilt (density-weighted mean with
/// MAD outlier rejection). Confident, in-family bands keep their measured angle,
/// and a lane with no confident band is left as measured — no global `tilt(x)`
/// line is imposed, since the smile is not monotonic and a cross-lane fit would
/// forcibly (and wrongly) rotate a lane toward its neighbours.
fn fill_undetermined_angles(bands: &mut [DetBand]) {
    use std::collections::BTreeMap;
    let zero_tol = 0.9f64.to_radians();
    let mut by_lane: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (i, b) in bands.iter().enumerate() {
        by_lane.entry(b.lane_id).or_default().push(i);
    }
    for idxs in by_lane.values() {
        // Confident (mixture-oriented) bands in this lane, with density weights.
        let mut conf: Vec<(f64, f64)> = idxs
            .iter()
            .filter_map(|&i| {
                let b = &bands[i];
                (b.angle.abs() > zero_tol).then_some((b.angle, b.integrated_density.max(1e-6)))
            })
            .collect();
        if conf.is_empty() {
            continue; // nothing reliable in this lane → leave bands as measured
        }
        // Robust lane tilt: density-weighted mean, two MAD-rejection rounds.
        let wmean = |v: &[(f64, f64)]| -> f64 {
            let sw: f64 = v.iter().map(|x| x.1).sum();
            if sw <= 0.0 {
                0.0
            } else {
                v.iter().map(|(a, w)| a * w).sum::<f64>() / sw
            }
        };
        let mut center = wmean(&conf);
        for _ in 0..2 {
            let mut dev: Vec<f64> = conf.iter().map(|(a, _)| (a - center).abs()).collect();
            dev.sort_by(|p, q| p.partial_cmp(q).unwrap());
            let mad = dev[dev.len() / 2].max(0.02);
            conf.retain(|(a, _)| (a - center).abs() <= 3.0 * mad);
            if conf.is_empty() {
                break;
            }
            center = wmean(&conf);
        }
        // MAD of the surviving bands, to flag in-lane outliers.
        let mut dev: Vec<f64> = idxs
            .iter()
            .filter_map(|&i| {
                let a = bands[i].angle;
                (a.abs() > zero_tol).then_some((a - center).abs())
            })
            .collect();
        dev.sort_by(|p, q| p.partial_cmp(q).unwrap());
        let mad = dev.get(dev.len() / 2).copied().unwrap_or(0.05).max(0.02);
        for &i in idxs {
            let a = bands[i].angle;
            if a.abs() < zero_tol || (a - center).abs() > 3.0 * mad {
                bands[i].angle = center.clamp(-0.44, 0.44);
            }
        }
    }
}

/// Per-lane band tilts (radians) as a **Gaussian mixture** over the lane's bright
/// pixels: one 2-D Gaussian per detected band, its principal axis giving the
/// band's tilt. Pixels are assigned to bands by soft responsibility (EM), so
/// closely-spaced bands share the boundary between them smoothly — no hard
/// per-band window that would bleed in a neighbour or clip a tilted band's ends
/// (which biases the angle toward horizontal).
///
/// The band **centers are known** from detection (lane-center x, peak y), so the
/// means are fixed and EM only solves the covariances. Σxx (lane width) and Σyy
/// (thickness) are **tied across the lane**; only the per-band cross-term Σxy —
/// the tilt — varies, so a faint band borrows the lane's shape instead of
/// collapsing. The tilt is `½·atan2(2·Σxy, Σxx − Σyy)` (Σxx ≫ Σyy → well-posed);
/// a near-isotropic result reports horizontal. Clamped to ±25°.
fn fit_lane_gmm_angles(work: &GrayF32, lane: &DetLane, bands: &[(f64, f64, f64)]) -> Vec<f64> {
    let k = bands.len();
    if k == 0 {
        return Vec::new();
    }
    let (w, h) = (work.width(), work.height());
    let x0 = (lane.x_min as usize).min(w);
    let x1 = (lane.x_max as usize).min(w);
    if x1 <= x0 + 2 {
        return vec![0.0; k];
    }
    // Fixed means: lane-center x, detected peak y.
    let cx = lane.x_center();
    let mu: Vec<[f64; 2]> = bands.iter().map(|b| [cx, b.0]).collect();
    // Bright pixels only (above the region mean) — bands dominate the mixture.
    let (mut sum, mut cnt) = (0.0f64, 0.0f64);
    for y in 0..h {
        for x in x0..x1 {
            sum += work.get(x, y) as f64;
            cnt += 1.0;
        }
    }
    let mean = sum / cnt.max(1.0);
    let mut pts: Vec<(f64, f64, f64)> = Vec::new();
    for y in 0..h {
        for x in x0..x1 {
            let g = (work.get(x, y) as f64 - mean).max(0.0);
            if g > 0.0 {
                pts.push((x as f64, y as f64, g));
            }
        }
    }
    if pts.len() < k * 3 {
        return vec![0.0; k];
    }
    // Tied shape: all bands in the lane share Σxx (lane width) and Σyy (band
    // thickness); only the cross-term Σxy — the tilt — is per band. This pools
    // every band's pixels to pin the shared shape, so a faint band with few
    // pixels borrows the lane's shape instead of collapsing to an ambiguous
    // near-round covariance (the old per-band 0° fallback). Σxx stays ≫ Σyy, so
    // Σxx−Σyy never vanishes and the axis is always well-conditioned.
    let eps = 0.5; // covariance floor (keeps Gaussians non-degenerate)
    let mut sxx = ((x1 - x0) as f64 / 4.0).powi(2).max(1.0);
    let mut syy = (bands.iter().map(|b| b.1.max(1.0).powi(2)).sum::<f64>() / k as f64).max(1.0);
    let mut sxy = vec![0.0f64; k];
    let mut pi: Vec<f64> = bands.iter().map(|b| b.2.max(1e-6)).collect();
    let psum: f64 = pi.iter().sum();
    for p in pi.iter_mut() {
        *p /= psum;
    }
    for _ in 0..25 {
        let mut nk = vec![0.0f64; k]; // responsibility mass per band
        let (mut axx, mut ayy) = (0.0f64, 0.0f64); // pooled → shared Σxx, Σyy
        let mut axy = vec![0.0f64; k]; // per-band Σxy
        for &(px, py, pw) in &pts {
            let mut r = vec![0.0f64; k];
            let mut rs = 0.0f64;
            for j in 0..k {
                let v = pi[j] * gauss2d(px, py, &mu[j], &[sxx, sxy[j], syy]);
                r[j] = v;
                rs += v;
            }
            if rs <= 1e-300 {
                continue;
            }
            for j in 0..k {
                let rj = pw * r[j] / rs;
                if rj <= 0.0 {
                    continue;
                }
                let dx = px - mu[j][0];
                let dy = py - mu[j][1];
                nk[j] += rj;
                axx += rj * dx * dx;
                ayy += rj * dy * dy;
                axy[j] += rj * dx * dy;
            }
        }
        let ntot: f64 = nk.iter().sum();
        if ntot < 1e-6 {
            break;
        }
        sxx = axx / ntot + eps;
        syy = ayy / ntot + eps;
        // Keep each per-band covariance positive-definite (|Σxy| < √(Σxx·Σyy)).
        let cap = 0.95 * (sxx * syy).sqrt();
        for j in 0..k {
            if nk[j] > 1e-6 {
                sxy[j] = (axy[j] / nk[j]).clamp(-cap, cap);
            }
            pi[j] = nk[j] / ntot;
        }
    }
    let denom = sxx - syy;
    sxy.iter()
        .map(|&s| {
            if denom <= 1e-6 {
                0.0
            } else {
                (0.5 * (2.0 * s).atan2(denom)).clamp(-0.44, 0.44)
            }
        })
        .collect()
}

/// Unnormalized-safe 2-D Gaussian density at `(x, y)` for mean `mu` and
/// covariance `cov = [Σxx, Σxy, Σyy]`.
fn gauss2d(x: f64, y: f64, mu: &[f64; 2], cov: &[f64; 3]) -> f64 {
    let [sxx, sxy, syy] = *cov;
    let det = sxx * syy - sxy * sxy;
    if det <= 1e-9 {
        return 0.0;
    }
    let (ixx, ixy, iyy) = (syy / det, -sxy / det, sxx / det);
    let (dx, dy) = (x - mu[0], y - mu[1]);
    let m = ixx * dx * dx + 2.0 * ixy * dx * dy + iyy * dy * dy;
    (-0.5 * m).exp() / (2.0 * std::f64::consts::PI * det.sqrt())
}

/// Returns `(y_center, y_half_width, integrated_density)` per band.
fn detect_bands_in_lane(
    img: &GrayF32,
    lane: &DetLane,
    params: &DetectParams,
) -> Vec<(f64, f64, f64)> {
    let raw = lane_row_profile(img, lane.x_min as usize, lane.x_max as usize);
    let smoothed = smooth(&raw, params.row_smooth);
    let corrected = subtract_baseline(&smoothed, params.baseline_radius);
    let max = corrected.iter().cloned().fold(0.0f64, f64::max);
    if max <= 0.0 {
        return Vec::new();
    }
    let min_prom = params.band_min_rel_prominence * max;
    find_peaks(&corrected, min_prom, params.min_band_distance)
        .into_iter()
        .map(|p| (p.center, half_width_half_max(&corrected, &p), p.area))
        .collect()
}

/// Band half-thickness (px) as the half-width at half-maximum of the peak: walk
/// out from the peak center until the profile falls below half the peak height,
/// bounded by the peak's own [left, right] support. This is the band's actual
/// thickness — unlike `(right - left)/2`, which spans the whole valley-to-valley
/// footprint and wildly overstates thin bands packed close together.
fn half_width_half_max(profile: &[f64], p: &crate::detect::signal::Peak) -> f64 {
    let n = profile.len();
    if n == 0 {
        return 0.75;
    }
    let c = (p.center.round() as usize).min(n - 1);
    let half = p.height * 0.5;
    let mut l = c;
    while l > p.left && profile[l] > half {
        l -= 1;
    }
    let mut r = c;
    while r + 1 < n && r < p.right && profile[r] > half {
        r += 1;
    }
    ((r - l) as f64 / 2.0).max(0.75)
}
