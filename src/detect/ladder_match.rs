//! Ladder identification: match a lane's band pattern to a commercial ladder
//! template and decide which lane(s) are ladders.
//!
//! The physical model is semi-log: `ln(size)` is ~linear in migration position.
//! A lane that *is* a given ladder therefore yields a high R² when its matched
//! band positions are regressed against the template's `ln(size)` values.
//!
//! Two matchers:
//! * [`match_template`] — exact ordered mapping when the band count equals the
//!   rung count (the clean case).
//! * [`align_template`] — a RANSAC-style monotonic alignment that tolerates
//!   missing faint rungs and extra (sample/noise) bands by fitting a line
//!   through seed pairs and counting inliers.

use crate::core::model::LadderTemplate;
use crate::core::quant::SizingFit;

/// One matched correspondence: a detected band (by index into the lane's
/// top→bottom band list) and the ladder size assigned to it.
#[derive(Debug, Clone, Copy)]
pub struct MatchPair {
    pub band_index: usize,
    pub size: f64,
}

/// Result of matching one lane to one template.
#[derive(Debug, Clone)]
pub struct LadderMatch {
    pub template_name: String,
    /// Goodness of the semi-log fit in `[0, 1]` over matched pairs.
    pub r2: f64,
    /// Matched band↔size correspondences.
    pub pairs: Vec<MatchPair>,
    /// The fitted sizing model (for sizing other lanes).
    pub fit: SizingFit,
}

/// R² of `ln(size) = a*position + b` over equal-length position/size vectors.
fn semilog_r2(positions: &[f64], sizes: &[f64]) -> Option<(f64, SizingFit)> {
    if positions.len() != sizes.len() || positions.len() < 2 {
        return None;
    }
    let pts: Vec<(f64, f64)> = positions
        .iter()
        .cloned()
        .zip(sizes.iter().cloned())
        .collect();
    let fit = SizingFit::fit(&pts)?;
    let ys: Vec<f64> = sizes.iter().map(|s| s.ln()).collect();
    let mean = ys.iter().sum::<f64>() / ys.len() as f64;
    let mut ss_res = 0.0;
    let mut ss_tot = 0.0;
    for (&pos, &y) in positions.iter().zip(&ys) {
        let pred = fit.a * pos + fit.b;
        ss_res += (y - pred).powi(2);
        ss_tot += (y - mean).powi(2);
    }
    let r2 = if ss_tot < 1e-12 {
        1.0
    } else {
        (1.0 - ss_res / ss_tot).clamp(0.0, 1.0)
    };
    Some((r2, fit))
}

fn finalize(
    template: &LadderTemplate,
    pairs: Vec<MatchPair>,
    positions: &[f64],
) -> Option<LadderMatch> {
    if pairs.len() < 2 {
        return None;
    }
    let ps: Vec<f64> = pairs.iter().map(|p| positions[p.band_index]).collect();
    let ss: Vec<f64> = pairs.iter().map(|p| p.size).collect();
    let (r2, fit) = semilog_r2(&ps, &ss)?;
    Some(LadderMatch {
        template_name: template.name.clone(),
        r2,
        pairs,
        fit,
    })
}

/// Exact ordered match when the rung count equals the band count.
pub fn match_template(positions: &[f64], template: &LadderTemplate) -> Option<LadderMatch> {
    if positions.len() != template.bands.len() {
        return None;
    }
    let pairs = template
        .bands
        .iter()
        .enumerate()
        .map(|(i, b)| MatchPair {
            band_index: i,
            size: b.size,
        })
        .collect();
    finalize(template, pairs, positions)
}

/// RANSAC-style monotonic alignment tolerating unequal counts.
///
/// `positions` are the lane's band y-centers, ascending (top→bottom).
/// `tol_ln` is the residual tolerance in ln(size). Returns the alignment with
/// the most inliers (>= `min_matches`).
pub fn align_template(
    positions: &[f64],
    template: &LadderTemplate,
    tol_ln: f64,
    min_matches: usize,
) -> Option<LadderMatch> {
    let sizes: Vec<f64> = template.bands.iter().map(|b| b.size).collect();
    let ln_sizes: Vec<f64> = sizes.iter().map(|s| s.ln()).collect();
    let (s, p) = (sizes.len(), positions.len());
    if s < 2 || p < 2 {
        // Fall back to exact when tiny.
        return match_template(positions, template);
    }

    let mut best: Option<Vec<MatchPair>> = None;
    let mut best_count = 0usize;
    let mut best_resid = f64::INFINITY;

    // Seeds: pairs of rungs (ia<ib) mapped to pairs of bands (ja<jb).
    for ia in 0..s {
        for ib in (ia + 1)..s {
            for ja in 0..p {
                for jb in (ja + 1)..p {
                    let dp = positions[jb] - positions[ja];
                    if dp.abs() < 1e-6 {
                        continue;
                    }
                    let a = (ln_sizes[ib] - ln_sizes[ia]) / dp;
                    // Larger size migrates less (smaller y): slope must be < 0.
                    if a >= 0.0 {
                        continue;
                    }
                    let b = ln_sizes[ia] - a * positions[ja];

                    // Greedy monotonic inlier assignment.
                    let mut pairs = Vec::new();
                    let mut last_j: isize = -1;
                    let mut resid_sum = 0.0;
                    for i in 0..s {
                        let mut chosen: Option<(usize, f64)> = None;
                        for (j, &position) in positions
                            .iter()
                            .enumerate()
                            .take(p)
                            .skip((last_j + 1) as usize)
                        {
                            let r = (ln_sizes[i] - (a * position + b)).abs();
                            if r < tol_ln {
                                match chosen {
                                    Some((_, br)) if br <= r => {}
                                    _ => chosen = Some((j, r)),
                                }
                            }
                        }
                        if let Some((j, r)) = chosen {
                            pairs.push(MatchPair {
                                band_index: j,
                                size: sizes[i],
                            });
                            resid_sum += r;
                            last_j = j as isize;
                        }
                    }
                    let count = pairs.len();
                    if count > best_count || (count == best_count && resid_sum < best_resid) {
                        best_count = count;
                        best_resid = resid_sum;
                        best = Some(pairs);
                    }
                }
            }
        }
    }

    let pairs = best?;
    if pairs.len() < min_matches {
        return None;
    }
    finalize(template, pairs, positions)
}

/// Best matching template for a lane among candidates at or above `min_r2`.
/// Uses exact matching when counts align, else robust alignment.
pub fn best_template<'a>(
    positions: &[f64],
    candidates: impl IntoIterator<Item = &'a LadderTemplate>,
    min_r2: f64,
) -> Option<LadderMatch> {
    candidates
        .into_iter()
        .filter_map(|t| {
            if positions.len() == t.bands.len() {
                match_template(positions, t)
            } else {
                align_template(positions, t, 0.35, 4)
            }
        })
        .filter(|m| m.r2 >= min_r2)
        .max_by(|a, b| {
            // Prefer more matched rungs, then higher R².
            a.pairs
                .len()
                .cmp(&b.pairs.len())
                .then(a.r2.partial_cmp(&b.r2).unwrap())
        })
}

/// Among several lanes, pick the one best explained by some candidate template.
pub fn identify_ladder_lane<'a>(
    lanes_positions: &[Vec<f64>],
    candidates: &'a [&'a LadderTemplate],
    min_r2: f64,
) -> Option<(usize, LadderMatch)> {
    let cands = || candidates.iter().copied();
    lanes_positions
        .iter()
        .enumerate()
        .filter_map(|(i, pos)| best_template(pos, cands(), min_r2).map(|m| (i, m)))
        .max_by(|a, b| a.1.r2.partial_cmp(&b.1.r2).unwrap())
}
