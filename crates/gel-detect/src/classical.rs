//! Classical densitometry detector: the gel-analysis workhorse.
//!
//! 1. **Lane finding** — sum intensity down each column to get a horizontal
//!    profile; lanes are the peaks, bounded by the valleys between them.
//! 2. **Band finding** — within each lane, sum intensity across the lane width
//!    to get a vertical densitometry trace; subtract a rolling-ball baseline;
//!    detect and integrate peaks as bands.

use gel_core::model::{Band, Lane};
use gel_core::GrayF32;

use crate::detector::{DetectParams, Detection, GelDetector};
use crate::signal::{find_peaks, smooth, subtract_baseline};

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
        let h = work.height();
        let lanes = detect_lanes(&work, params);

        let mut bands = Vec::new();
        let mut band_id = 0u32;
        for lane in &lanes {
            for b in detect_bands_in_lane(&work, lane, params) {
                let (y_center, y_half, area) = b;
                let rf = if h > 1 {
                    Some((y_center - lane.y_min as f64) / (lane.y_max - lane.y_min).max(1) as f64)
                } else {
                    None
                };
                bands.push(Band {
                    id: band_id,
                    lane_id: lane.id,
                    y_center,
                    y_half_width: y_half,
                    integrated_density: area,
                    rf,
                    size: None,
                    known_size: None,
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
        for x in 0..w {
            profile[x] += img.get(x, y) as f64;
        }
    }
    profile
}

/// Row-sum profile over `[x_min, x_max)`: `profile[y] = sum over x`.
pub fn lane_row_profile(img: &GrayF32, x_min: usize, x_max: usize) -> Vec<f64> {
    let h = img.height();
    let x_max = x_max.min(img.width());
    let mut profile = vec![0.0; h];
    for y in 0..h {
        let mut acc = 0.0;
        for x in x_min..x_max {
            acc += img.get(x, y) as f64;
        }
        profile[y] = acc;
    }
    profile
}

fn detect_lanes(img: &GrayF32, params: &DetectParams) -> Vec<Lane> {
    let raw = column_profile(img);
    let profile = smooth(&raw, params.column_smooth);
    let max = profile.iter().cloned().fold(0.0f64, f64::max);
    if max <= 0.0 {
        return Vec::new();
    }
    let min_prom = params.lane_min_rel_prominence * max;
    let mut peaks = find_peaks(&profile, min_prom, params.min_lane_distance);

    // Keep only the requested number of lanes, most prominent first.
    if let Some(n) = params.expected_lanes {
        peaks.sort_by(|a, b| b.prominence.partial_cmp(&a.prominence).unwrap());
        peaks.truncate(n);
        peaks.sort_by(|a, b| a.center.partial_cmp(&b.center).unwrap());
    }

    let h = img.height() as u32;
    peaks
        .into_iter()
        .enumerate()
        .map(|(i, p)| Lane {
            id: i as u32,
            x_min: p.left as u32,
            x_max: (p.right as u32 + 1).min(img.width() as u32),
            y_min: 0,
            y_max: h,
            label: None,
            is_ladder: false,
        })
        .collect()
}

/// Returns `(y_center, y_half_width, integrated_density)` per band.
fn detect_bands_in_lane(
    img: &GrayF32,
    lane: &Lane,
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
