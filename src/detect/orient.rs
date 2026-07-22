//! Orientation estimation ("auto-straighten").
//!
//! Lanes are vertical when the column-sum intensity profile is most "peaky":
//! at the correct deskew angle the projection concentrates into lane peaks and
//! its coefficient of variation is maximized. We search rotation angles and
//! pick the maximizer, refining coarse-to-fine.

use crate::core::GrayF32;

/// Coefficient of variation (var / mean²) of the column projection of `img`
/// after rotating it by `angle_deg` — scale invariant, higher when lanes are
/// vertical and well-separated.
///
/// Rotation fills the corners with black; over the full frame those black
/// triangles turn edge columns near-zero and inflate the column variance,
/// spuriously rewarding large angles (the gel would "snap" to ~45°). We compute
/// the rotation in a single pass so we can (a) count only pixels whose source
/// lies *inside* the frame and (b) build each column from the MEAN of its
/// in-frame pixels, dropping columns that are mostly out of frame. The score
/// then reflects only real content, at any angle — no fixed crop needed.
fn projection_sharpness_at(img: &GrayF32, angle_deg: f64) -> f64 {
    let (w, h) = (img.width(), img.height());
    if w < 4 || h < 4 {
        return 0.0;
    }
    let cx = (w as f64 - 1.0) / 2.0;
    let cy = (h as f64 - 1.0) / 2.0;
    let rad = angle_deg.to_radians();
    let (c, s) = (rad.cos(), rad.sin());
    let (xmax, ymax) = (w as f64 - 1.0, h as f64 - 1.0);
    let mut col_sum = vec![0.0f64; w];
    let mut col_cnt = vec![0u32; w];
    for y in 0..h {
        let dy = y as f64 - cy;
        for x in 0..w {
            let dx = x as f64 - cx;
            // Inverse rotation: output pixel -> source pixel.
            let sx = cx + dx * c + dy * s;
            let sy = cy - dx * s + dy * c;
            if sx < 0.0 || sy < 0.0 || sx > xmax || sy > ymax {
                continue; // outside the source frame (would be black)
            }
            col_sum[x] += img.sample_bilinear(sx as f32, sy as f32) as f64;
            col_cnt[x] += 1;
        }
    }
    // Per-column mean over in-frame pixels; keep only mostly-covered columns so
    // partial edge columns can't dominate the variance.
    let min_cnt = (h as u32) / 2;
    let profile: Vec<f64> = col_sum
        .iter()
        .zip(&col_cnt)
        .filter(|(_, &n)| n >= min_cnt)
        .map(|(&sum, &n)| sum / n as f64)
        .collect();
    let n = profile.len() as f64;
    if n < 4.0 {
        return 0.0;
    }
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
    let (fine_best, _) = search(
        &work,
        best_angle - coarse,
        best_angle + coarse,
        0.25,
        f64::NEG_INFINITY,
    );
    fine_best
}

/// Scan angles in `[lo, hi]` with `step`, returning the `(angle, score)` that
/// maximizes projection sharpness (starting from `seed_score`).
fn search(work: &GrayF32, lo: f64, hi: f64, step: f64, seed_score: f64) -> (f64, f64) {
    let mut best_angle = lo;
    let mut best_score = seed_score;
    let mut a = lo;
    while a <= hi + 1e-9 {
        let score = projection_sharpness_at(work, a);
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
