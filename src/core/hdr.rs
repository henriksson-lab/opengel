//! High-dynamic-range merge of an exposure bracket.
//!
//! Uses a linear sensor model: for pixel value `v` (normalized `[0,1]`) at
//! exposure time `t`, the scene radiance estimate is `(v - bias) / t`. We combine
//! exposures with a hat weighting that trusts mid-range pixels and distrusts
//! near-saturated (`v→1`) and near-noise-floor (`v→0`) pixels.
//!
//! Optional pre/merge stages ([`HdrOptions`]):
//! * **bias subtraction** — estimate each frame's black level (a low percentile)
//!   and subtract it, so faint bands aren't lifted by the sensor's dark offset;
//! * **alignment** — coarse integer translational registration of each frame to
//!   the best-exposed reference (a fixed rig only shifts, it doesn't rotate);
//! * **de-ghosting** — reject, per pixel, exposure samples that disagree with the
//!   across-exposure median (a speck that moved between frames).

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

/// Optional stages applied during [`merge_hdr_with`]. All default `false`, which
/// reproduces the plain linear hat-weighted merge ([`merge_hdr`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HdrOptions {
    /// Estimate and subtract each frame's black level (bias) before `v/t`.
    pub bias_subtraction: bool,
    /// Coarsely register each frame to the best-exposed reference (translation).
    pub align: bool,
    /// Reject per-pixel exposure samples that disagree with the median (motion).
    pub deghost: bool,
}

/// Triangular hat weight over normalized pixel value, peaking at 0.5.
/// Small epsilon floor keeps every pixel usable as a last resort.
#[inline]
fn hat_weight(v: f32) -> f32 {
    let w = 1.0 - (2.0 * v - 1.0).abs();
    w.max(1e-3)
}

/// Merge an exposure bracket into a linear radiance image (plain hat-weighted
/// merge, no optional stages). See [`merge_hdr_with`].
pub fn merge_hdr(frames: &[GrayF32], exposures: &[f64]) -> Result<GrayF32, HdrError> {
    merge_hdr_with(frames, exposures, &HdrOptions::default())
}

/// Merge an exposure bracket into a linear radiance image, applying the optional
/// stages in `opts`.
///
/// `frames` and `exposures` (seconds) must be parallel and non-empty; all frames
/// must share dimensions. The result is radiance in units of
/// "normalized-intensity per second" and is not clamped to `[0,1]`.
pub fn merge_hdr_with(
    frames: &[GrayF32],
    exposures: &[f64],
    opts: &HdrOptions,
) -> Result<GrayF32, HdrError> {
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

    // --- optional alignment: register every frame to the best-exposed one ---
    let aligned: Vec<GrayF32> = if opts.align && frames.len() > 1 {
        let reference = best_exposed_index(frames);
        frames
            .iter()
            .enumerate()
            .map(|(i, f)| {
                if i == reference {
                    f.clone()
                } else {
                    let (dx, dy) = estimate_shift(&frames[reference], f);
                    translate(f, dx, dy)
                }
            })
            .collect()
    } else {
        frames.to_vec()
    };

    // --- optional bias subtraction: per-frame black level ---
    let bias: Vec<f32> = if opts.bias_subtraction {
        aligned.iter().map(black_level).collect()
    } else {
        vec![0.0; aligned.len()]
    };

    let mut out = Array2::<f32>::zeros((h, w));
    // Reused per-pixel scratch for de-ghosting.
    let mut radiances = vec![0.0f32; aligned.len()];
    for y in 0..h {
        for x in 0..w {
            for (i, (frame, &t)) in aligned.iter().zip(exposures).enumerate() {
                let v = frame.get(x, y);
                radiances[i] = ((v - bias[i]).max(0.0)) / t as f32;
            }
            // De-ghost: mark samples deviating far from the median as rejected.
            let keep = if opts.deghost && aligned.len() >= 3 {
                deghost_mask(&radiances)
            } else {
                None
            };

            let mut wsum = 0.0f32;
            let mut rsum = 0.0f32;
            let mut best_dist = f32::INFINITY;
            let mut best_radiance = 0.0f32;
            for (i, (frame, _t)) in aligned.iter().zip(exposures).enumerate() {
                let v = frame.get(x, y);
                if let Some(mask) = &keep {
                    if !mask[i] {
                        continue;
                    }
                }
                let wt = hat_weight(v);
                wsum += wt;
                rsum += wt * radiances[i];
                let dist = (v - 0.5).abs();
                if dist < best_dist {
                    best_dist = dist;
                    best_radiance = radiances[i];
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

/// Index of the frame with the most well-exposed pixels (max total hat weight) —
/// the sharpest reference for alignment.
fn best_exposed_index(frames: &[GrayF32]) -> usize {
    frames
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| {
            let wa: f32 = a.data.iter().map(|&v| hat_weight(v)).sum();
            let wb: f32 = b.data.iter().map(|&v| hat_weight(v)).sum();
            wa.partial_cmp(&wb).unwrap()
        })
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// Estimate a frame's black level: a low percentile of its pixel values, robust
/// to a few dead-black pixels.
fn black_level(frame: &GrayF32) -> f32 {
    let mut vals: Vec<f32> = frame.data.iter().copied().collect();
    if vals.is_empty() {
        return 0.0;
    }
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    // 0.5th percentile.
    let idx = ((vals.len() as f64) * 0.005) as usize;
    vals[idx.min(vals.len() - 1)]
}

/// Per-pixel de-ghost mask: keep only samples within a relative tolerance of the
/// median radiance. `None`-equivalent (all kept) is handled by the caller.
fn deghost_mask(radiances: &[f32]) -> Option<Vec<bool>> {
    let mut sorted: Vec<f32> = radiances.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sorted[sorted.len() / 2];
    // Tolerance scales with the median (radiances span orders of magnitude).
    let tol = 0.5 * median + 1e-6;
    let mask: Vec<bool> = radiances
        .iter()
        .map(|&r| (r - median).abs() <= tol)
        .collect();
    // Never reject everything.
    if mask.iter().all(|&k| !k) {
        None
    } else {
        Some(mask)
    }
}

/// Shift `frame` by integer `(dx, dy)` pixels, replicating the border. Positive
/// `dx`/`dy` move content right/down.
fn translate(frame: &GrayF32, dx: i32, dy: i32) -> GrayF32 {
    let (w, h) = (frame.width(), frame.height());
    let mut out = Array2::<f32>::zeros((h, w));
    for y in 0..h {
        let sy = (y as i32 - dy).clamp(0, h as i32 - 1) as usize;
        for x in 0..w {
            let sx = (x as i32 - dx).clamp(0, w as i32 - 1) as usize;
            out[[y, x]] = frame.get(sx, sy);
        }
    }
    GrayF32 { data: out }
}

/// Estimate the integer translation that aligns `moving` onto `reference` via
/// **FFT phase correlation**. Both frames are zero-padded to a common power-of-
/// two size; the normalized cross-power spectrum's inverse transform has a sharp
/// peak at the translation between them. O(N log N), robust to brightness
/// differences (the whitening normalizes magnitude).
fn estimate_shift(reference: &GrayF32, moving: &GrayF32) -> (i32, i32) {
    use rustfft::num_complex::Complex;

    let (w, h) = (reference.width(), reference.height());
    let (pw, ph) = (next_pow2(w), next_pow2(h));
    if pw == 0 || ph == 0 {
        return (0, 0);
    }
    let mut r = padded_complex(reference, pw, ph);
    let mut m = padded_complex(moving, pw, ph);
    fft2(&mut r, pw, ph, false);
    fft2(&mut m, pw, ph, false);

    // Cross-power spectrum, magnitude-whitened: C = R · conj(M) / |R · conj(M)|.
    let mut c: Vec<Complex<f32>> = r
        .iter()
        .zip(&m)
        .map(|(&rk, &mk)| {
            let cross = rk * mk.conj();
            let mag = cross.norm();
            if mag > 1e-12 {
                cross / mag
            } else {
                Complex::new(0.0, 0.0)
            }
        })
        .collect();
    fft2(&mut c, pw, ph, true); // inverse → correlation surface

    // Peak of the real correlation surface gives the shift (with wraparound).
    let mut best = f32::NEG_INFINITY;
    let mut peak = 0usize;
    for (i, v) in c.iter().enumerate() {
        if v.re > best {
            best = v.re;
            peak = i;
        }
    }
    let (mut dx, mut dy) = ((peak % pw) as i32, (peak / pw) as i32);
    if dx > pw as i32 / 2 {
        dx -= pw as i32;
    }
    if dy > ph as i32 / 2 {
        dy -= ph as i32;
    }
    // The peak of IFFT(R·conj(M)) lands at the translation that carries `moving`
    // back onto `reference`, so it is the correction to apply directly.
    (dx, dy)
}

fn next_pow2(n: usize) -> usize {
    let mut p = 1;
    while p < n {
        p <<= 1;
    }
    p
}

/// Zero-pad `frame` into a `pw × ph` complex buffer (row-major), real part only.
fn padded_complex(
    frame: &GrayF32,
    pw: usize,
    ph: usize,
) -> Vec<rustfft::num_complex::Complex<f32>> {
    use rustfft::num_complex::Complex;
    let (w, h) = (frame.width(), frame.height());
    let mut buf = vec![Complex::new(0.0f32, 0.0); pw * ph];
    for y in 0..h.min(ph) {
        for x in 0..w.min(pw) {
            buf[y * pw + x].re = frame.get(x, y);
        }
    }
    buf
}

/// In-place 2D FFT (or inverse) of a `pw × ph` row-major complex buffer, done as
/// row transforms then column transforms. `pw`/`ph` should be powers of two.
fn fft2(buf: &mut [rustfft::num_complex::Complex<f32>], pw: usize, ph: usize, inverse: bool) {
    use rustfft::num_complex::Complex;
    let mut planner = rustfft::FftPlanner::<f32>::new();
    let row_fft = if inverse {
        planner.plan_fft_inverse(pw)
    } else {
        planner.plan_fft_forward(pw)
    };
    // Rows are contiguous.
    for row in buf.chunks_mut(pw) {
        row_fft.process(row);
    }
    // Columns: gather stride-`pw` into a scratch buffer, transform, scatter back.
    let col_fft = if inverse {
        planner.plan_fft_inverse(ph)
    } else {
        planner.plan_fft_forward(ph)
    };
    let mut col = vec![Complex::new(0.0f32, 0.0); ph];
    for x in 0..pw {
        for (y, c) in col.iter_mut().enumerate() {
            *c = buf[y * pw + x];
        }
        col_fft.process(&mut col);
        for (y, c) in col.iter().enumerate() {
            buf[y * pw + x] = *c;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn constant(w: usize, h: usize, v: f32) -> GrayF32 {
        GrayF32 {
            data: Array2::from_elem((h, w), v),
        }
    }

    #[test]
    fn plain_merge_recovers_radiance() {
        // Same scene at two exposures: v = radiance * t (below saturation).
        // radiance = 0.3. t = 1 → v=0.3; t=2 → v=0.6.
        let f1 = constant(4, 4, 0.3);
        let f2 = constant(4, 4, 0.6);
        let m = merge_hdr(&[f1, f2], &[1.0, 2.0]).unwrap();
        for &v in m.data.iter() {
            assert!((v - 0.3).abs() < 1e-4, "got {v}");
        }
    }

    #[test]
    fn bias_subtraction_removes_offset() {
        // Frames with a dark background = sensor bias 0.1 (most of the frame) and
        // a small signal block. Signal radiance is 0.3, so at t=1 the signal
        // pixels read 0.1+0.3=0.4 and at t=2 read 0.1+0.6=0.7.
        let (w, h) = (16, 16);
        let mut a = Array2::<f32>::from_elem((h, w), 0.1);
        let mut b = Array2::<f32>::from_elem((h, w), 0.1);
        for y in 6..10 {
            for x in 6..10 {
                a[[y, x]] = 0.4;
                b[[y, x]] = 0.7;
            }
        }
        let (f1, f2) = (GrayF32 { data: a }, GrayF32 { data: b });
        let opts = HdrOptions {
            bias_subtraction: true,
            ..Default::default()
        };
        let m = merge_hdr_with(&[f1, f2], &[1.0, 2.0], &opts).unwrap();
        // The dark background (0.5th percentile) is subtracted as bias, so the
        // signal block recovers radiance 0.3 rather than a bias-inflated value.
        assert!((m.get(7, 7) - 0.3).abs() < 1e-3, "signal = {}", m.get(7, 7));
    }

    #[test]
    fn alignment_recovers_a_known_shift() {
        // Build a frame with a bright block, and a copy shifted right by 3 px.
        let (w, h) = (64, 48);
        let mut a = Array2::<f32>::from_elem((h, w), 0.2);
        for y in 15..30 {
            for x in 20..35 {
                a[[y, x]] = 0.8;
            }
        }
        let reference = GrayF32 { data: a };
        let moving = translate(&reference, 3, 0); // content shifted right by 3
        let (dx, dy) = estimate_shift(&reference, &moving);
        // To align `moving` back onto `reference` we translate it left by 3.
        assert_eq!((dx, dy), (-3, 0));
    }

    #[test]
    fn deghost_rejects_an_outlier_frame() {
        // Three exposures agree on radiance 0.3 except a speck in frame 2.
        let f1 = constant(2, 2, 0.30); // t=1 → r=0.30
        let f2 = constant(2, 2, 0.60); // t=2 → r=0.30
        let mut d = Array2::from_elem((2, 2), 0.90f32); // t=3 → r=0.30 normally...
        d[[0, 0]] = 0.10; // ...but one pixel is a ghost (r≈0.033)
        let f3 = GrayF32 { data: d };
        let opts = HdrOptions {
            deghost: true,
            ..Default::default()
        };
        let m = merge_hdr_with(&[f1, f2, f3], &[1.0, 2.0, 3.0], &opts).unwrap();
        // The ghost pixel should be pulled back toward the 0.30 consensus rather
        // than dragged down by the outlier sample.
        assert!(
            (m.get(0, 0) - 0.30).abs() < 0.05,
            "ghost pixel = {}",
            m.get(0, 0)
        );
    }
}
