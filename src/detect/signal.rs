//! 1-D signal processing for densitometry traces: smoothing, morphological
//! baseline (rolling-ball style), and peak detection with integration.

/// Moving-average smoothing with a window of `2*radius+1`. `radius == 0` is a
/// no-op. Edges use available samples (shrinking window).
pub fn smooth(trace: &[f64], radius: usize) -> Vec<f64> {
    if radius == 0 || trace.len() < 2 {
        return trace.to_vec();
    }
    let n = trace.len();
    let mut out = vec![0.0; n];
    for (i, out_i) in out.iter_mut().enumerate().take(n) {
        let lo = i.saturating_sub(radius);
        let hi = (i + radius + 1).min(n);
        let slice = &trace[lo..hi];
        *out_i = slice.iter().sum::<f64>() / slice.len() as f64;
    }
    out
}

/// 1-D grayscale morphological opening (erosion then dilation) with a flat
/// structuring element of the given `radius`. This estimates the slowly-varying
/// background under the peaks — the "rolling ball / rolling disk" baseline.
pub fn morphological_baseline(trace: &[f64], radius: usize) -> Vec<f64> {
    let eroded = min_filter(trace, radius);
    max_filter(&eroded, radius)
}

fn min_filter(trace: &[f64], radius: usize) -> Vec<f64> {
    window_reduce(trace, radius, f64::min, f64::INFINITY)
}

fn max_filter(trace: &[f64], radius: usize) -> Vec<f64> {
    window_reduce(trace, radius, f64::max, f64::NEG_INFINITY)
}

fn window_reduce(trace: &[f64], radius: usize, f: fn(f64, f64) -> f64, init: f64) -> Vec<f64> {
    let n = trace.len();
    let mut out = vec![0.0; n];
    for (i, out_i) in out.iter_mut().enumerate().take(n) {
        let lo = i.saturating_sub(radius);
        let hi = (i + radius + 1).min(n);
        let mut acc = init;
        for &v in &trace[lo..hi] {
            acc = f(acc, v);
        }
        *out_i = acc;
    }
    out
}

/// Subtract a morphological baseline, clamping negatives to zero.
pub fn subtract_baseline(trace: &[f64], radius: usize) -> Vec<f64> {
    let base = morphological_baseline(trace, radius);
    trace
        .iter()
        .zip(&base)
        .map(|(&v, &b)| (v - b).max(0.0))
        .collect()
}

/// A detected peak in a background-subtracted trace.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Peak {
    /// Sub-sample center (index units) from a parabolic vertex refinement.
    pub center: f64,
    /// Peak height at the apex.
    pub height: f64,
    /// Inclusive integration bounds (indices into the trace).
    pub left: usize,
    pub right: usize,
    /// Integrated area (sum of values) over `[left, right]`.
    pub area: f64,
    /// Prominence above the higher of the two bounding valleys.
    pub prominence: f64,
}

/// Detect and integrate peaks in a (baseline-subtracted, non-negative) trace.
///
/// * `min_prominence` — reject peaks whose prominence is below this (absolute).
/// * `min_distance` — minimum index separation between accepted peak apices.
///
/// Integration bounds run to the local minima between adjacent peaks (or the
/// point where the trace returns near-zero).
pub fn find_peaks(trace: &[f64], min_prominence: f64, min_distance: usize) -> Vec<Peak> {
    let n = trace.len();
    if n < 3 {
        return Vec::new();
    }

    // Candidate apices: strict-ish local maxima.
    let mut apices: Vec<usize> = Vec::new();
    for i in 1..n - 1 {
        if trace[i] >= trace[i - 1] && trace[i] > trace[i + 1] {
            apices.push(i);
        }
    }

    // Prominence via bounding minima (descend both sides until we rise again).
    let mut peaks: Vec<Peak> = Vec::new();
    for &i in &apices {
        let (left, left_min) = descend(trace, i, -1);
        let (right, right_min) = descend(trace, i, 1);
        let base = left_min.max(right_min);
        let prominence = trace[i] - base;
        if prominence < min_prominence {
            continue;
        }
        let area: f64 = trace[left..=right].iter().sum();
        peaks.push(Peak {
            center: refine_center(trace, i),
            height: trace[i],
            left,
            right,
            area,
            prominence,
        });
    }

    // Enforce min_distance by keeping the more prominent of close peaks.
    peaks.sort_by(|a, b| b.prominence.partial_cmp(&a.prominence).unwrap());
    let mut kept: Vec<Peak> = Vec::new();
    for p in peaks {
        if kept
            .iter()
            .all(|k| (k.center - p.center).abs() as usize >= min_distance)
        {
            kept.push(p);
        }
    }
    kept.sort_by(|a, b| a.center.partial_cmp(&b.center).unwrap());
    kept
}

/// Walk downhill from `start` in `dir` (+1/-1) to the local minimum; return its
/// index and value.
fn descend(trace: &[f64], start: usize, dir: isize) -> (usize, f64) {
    let n = trace.len() as isize;
    let mut i = start as isize;
    let mut min_idx = start;
    let mut min_val = trace[start];
    loop {
        let next = i + dir;
        if next < 0 || next >= n {
            break;
        }
        let nv = trace[next as usize];
        if nv <= min_val {
            min_val = nv;
            min_idx = next as usize;
            i = next;
        } else {
            // Rising again — the valley is behind us.
            break;
        }
    }
    (min_idx, min_val)
}

/// Parabolic vertex refinement of a peak apex for sub-sample center.
fn refine_center(trace: &[f64], i: usize) -> f64 {
    if i == 0 || i + 1 >= trace.len() {
        return i as f64;
    }
    let (l, c, r) = (trace[i - 1], trace[i], trace[i + 1]);
    let denom = l - 2.0 * c + r;
    if denom.abs() < 1e-12 {
        return i as f64;
    }
    let delta = 0.5 * (l - r) / denom;
    i as f64 + delta.clamp(-1.0, 1.0)
}
