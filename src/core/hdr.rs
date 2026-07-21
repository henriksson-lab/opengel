//! High-dynamic-range merge of an exposure bracket.
//!
//! Uses a linear sensor model: for pixel value `v` (normalized `[0,1]`) at
//! exposure time `t`, the scene radiance estimate is `v / t`. We combine
//! exposures with a hat weighting that trusts mid-range pixels and distrusts
//! near-saturated (`v→1`) and near-noise-floor (`v→0`) pixels.

use crate::core::imagef32::GrayF32;
use ndarray::Array2;

#[derive(Debug, thiserror::Error)]
pub enum HdrError {
    #[error("need at least one frame")]
    Empty,
    #[error("frame/exposure count mismatch: {frames} frames, {exposures} exposures")]
    CountMismatch { frames: usize, exposures: usize },
    #[error("all frames must share dimensions")]
    DimMismatch,
    #[error("exposure times must be positive")]
    NonPositiveExposure,
}

/// Triangular hat weight over normalized pixel value, peaking at 0.5.
/// Small epsilon floor keeps every pixel usable as a last resort.
#[inline]
fn hat_weight(v: f32) -> f32 {
    let w = 1.0 - (2.0 * v - 1.0).abs();
    w.max(1e-3)
}

/// Merge an exposure bracket into a linear radiance image.
///
/// `frames` and `exposures` (seconds) must be parallel and non-empty; all
/// frames must share dimensions. The result is radiance in units of
/// "normalized-intensity per second" and is not clamped to `[0,1]`.
pub fn merge_hdr(frames: &[GrayF32], exposures: &[f64]) -> Result<GrayF32, HdrError> {
    if frames.is_empty() {
        return Err(HdrError::Empty);
    }
    if frames.len() != exposures.len() {
        return Err(HdrError::CountMismatch {
            frames: frames.len(),
            exposures: exposures.len(),
        });
    }
    if exposures.iter().any(|&t| t <= 0.0) {
        return Err(HdrError::NonPositiveExposure);
    }
    let (w, h) = (frames[0].width(), frames[0].height());
    if frames.iter().any(|f| f.width() != w || f.height() != h) {
        return Err(HdrError::DimMismatch);
    }

    let mut out = Array2::<f32>::zeros((h, w));
    for y in 0..h {
        for x in 0..w {
            let mut wsum = 0.0f32;
            let mut rsum = 0.0f32;
            // Fallback: value closest to mid-grey if every weight collapses.
            let mut best_dist = f32::INFINITY;
            let mut best_radiance = 0.0f32;
            for (frame, &t) in frames.iter().zip(exposures) {
                let v = frame.get(x, y);
                let radiance = v / t as f32;
                let wt = hat_weight(v);
                wsum += wt;
                rsum += wt * radiance;
                let dist = (v - 0.5).abs();
                if dist < best_dist {
                    best_dist = dist;
                    best_radiance = radiance;
                }
            }
            out[[y, x]] = if wsum > 1e-6 {
                rsum / wsum
            } else {
                best_radiance
            };
        }
    }
    Ok(GrayF32 { data: out })
}
