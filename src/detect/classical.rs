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
            for b in detect_bands_in_lane(&work, lane, params) {
                let (y_center, y_half, area) = b;
                bands.push(DetBand {
                    id: band_id,
                    lane_id: lane.id,
                    x_center: lane.x_center(),
                    y_center,
                    y_half_width: y_half,
                    integrated_density: area,
                });
                band_id += 1;
            }
        }
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
        .map(|p| {
            let half = (p.right as f64 - p.left as f64) / 2.0;
            (p.center, half.max(0.5), p.area)
        })
        .collect()
}
