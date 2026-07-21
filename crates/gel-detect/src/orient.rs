//! Orientation estimation ("auto-straighten").
//!
//! Lanes are vertical when the column-sum intensity profile is most "peaky":
//! at the correct deskew angle the projection concentrates into lane peaks and
//! its coefficient of variation is maximized. We search rotation angles and
//! pick the maximizer, refining coarse-to-fine.

use gel_core::GrayF32;

use crate::classical::column_profile;

/// Coefficient of variation (var / mean²) of the column projection — scale
/// invariant, higher when lanes are vertical and well-separated.
fn projection_sharpness(img: &GrayF32) -> f64 {
    let profile = column_profile(img);
    if profile.is_empty() {
        return 0.0;
    }
    let n = profile.len() as f64;
    let mean = profile.iter().sum::<f64>() / n;
    if mean.abs() < 1e-9 {
        return 0.0;
    }
    let var = profile.iter().map(|&p| (p - mean).powi(2)).sum::<f64>() / n;
    var / (mean * mean)
}

/// Estimate the rotation (degrees) that would straighten `img`. A positive
/// result means the image is currently rotated counter-clockwise by that much,
/// so straightening applies `-angle`.
///
/// `max_deg` bounds the search (e.g. 50). `signal_is_bright` inverts dark-band
/// images so peaks are bright before projecting.
pub fn estimate_rotation(img: &GrayF32, max_deg: f64, signal_is_bright: bool) -> f64 {
    let work = if signal_is_bright {
        img.clone()
    } else {
        img.inverted()
    };

    // Coarse pass over the whole search range.
    let coarse = 2.0;
    let (best_angle, _) = search(&work, -max_deg, max_deg, coarse, f64::NEG_INFINITY);
    // Fine pass around the coarse best.
    let (fine_best, _) = search(&work, best_angle - coarse, best_angle + coarse, 0.25, f64::NEG_INFINITY);
    fine_best
}

/// Scan angles in `[lo, hi]` with `step`, returning the `(angle, score)` that
/// maximizes projection sharpness (starting from `seed_score`).
fn search(work: &GrayF32, lo: f64, hi: f64, step: f64, seed_score: f64) -> (f64, f64) {
    let mut best_angle = lo;
    let mut best_score = seed_score;
    let mut a = lo;
    while a <= hi + 1e-9 {
        let score = projection_sharpness(&work.rotated(a));
        if score > best_score {
            best_score = score;
            best_angle = a;
        }
        a += step;
    }
    (best_angle, best_score)
}

/// Straighten an image: estimate its rotation and return `(canonical, angle)`
/// where `canonical = img.rotated(-angle)`.
pub fn auto_straighten(img: &GrayF32, max_deg: f64, signal_is_bright: bool) -> (GrayF32, f64) {
    let angle = estimate_rotation(img, max_deg, signal_is_bright);
    (img.rotated(-angle), angle)
}
