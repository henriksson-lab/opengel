//! Evaluation harness: score a [`GelDetector`] against annotated ground truth.
//!
//! This is the numeric basis for choosing a default detector and tuning its
//! parameters. Ground truth is authored per image (Claude-assisted) as JSON and
//! deserializes into [`GroundTruth`].

use crate::core::model::GelType;
use crate::core::GrayF32;
use serde::{Deserialize, Serialize};

use crate::detect::detector::{DetectParams, GelDetector};

/// Annotated ground truth for one gel image.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroundTruth {
    /// Image filename this annotation refers to (for the CLI to locate pixels).
    pub image: String,
    pub gel_type: GelType,
    #[serde(default)]
    pub lanes: Vec<GtLane>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GtLane {
    pub x_min: u32,
    pub x_max: u32,
    #[serde(default)]
    pub is_ladder: bool,
    #[serde(default)]
    pub ladder_name: Option<String>,
    #[serde(default)]
    pub bands: Vec<GtBand>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GtBand {
    pub y_center: f64,
    #[serde(default)]
    pub size: Option<f64>,
    /// True rectified migration `v ∈ [0,1]` (canonical, smile-free). Filled by
    /// the simulator, which knows each band's undistorted position; used to
    /// score how well a fitted warp recovers migration. `None` for real gels.
    #[serde(default)]
    pub v_true: Option<f64>,
}

/// Metrics for a single evaluated image.
#[derive(Debug, Clone, Default)]
pub struct EvalMetrics {
    pub lane_count_pred: usize,
    pub lane_count_true: usize,
    /// Mean IoU over greedily matched lane pairs (0 if none).
    pub lane_iou_mean: f64,
    pub band_true_positives: usize,
    pub band_false_positives: usize,
    pub band_false_negatives: usize,
    /// Mean absolute y error (px) over matched bands.
    pub band_pos_mae: f64,
}

impl EvalMetrics {
    pub fn band_precision(&self) -> f64 {
        let d = self.band_true_positives + self.band_false_positives;
        if d == 0 {
            0.0
        } else {
            self.band_true_positives as f64 / d as f64
        }
    }
    pub fn band_recall(&self) -> f64 {
        let d = self.band_true_positives + self.band_false_negatives;
        if d == 0 {
            0.0
        } else {
            self.band_true_positives as f64 / d as f64
        }
    }
    pub fn band_f1(&self) -> f64 {
        let (p, r) = (self.band_precision(), self.band_recall());
        if p + r < 1e-12 {
            0.0
        } else {
            2.0 * p * r / (p + r)
        }
    }
}

/// Warp-recovery error: mean absolute difference between the migration a `warp`
/// assigns to each ground-truth band and the band's true (smile-free) migration.
///
/// For each GT band the pixel position is `(lane center x, y_center)`; the warp
/// inverts it to `v_pred`, compared against `v_true`. The identity warp scores
/// the raw smile/rotation distortion; a well-fitted warp drives this toward 0.
/// Returns `None` if no band carries `v_true`.
pub fn warp_migration_error(warp: &crate::core::warp::GelWarp, gt: &GroundTruth) -> Option<f64> {
    let mut sum = 0.0;
    let mut n = 0usize;
    for lane in &gt.lanes {
        let cx = (lane.x_min as f64 + lane.x_max as f64) / 2.0;
        for band in &lane.bands {
            if let Some(v_true) = band.v_true {
                let (_, v_pred) = warp.invert(cx, band.y_center);
                sum += (v_pred - v_true).abs();
                n += 1;
            }
        }
    }
    if n == 0 {
        None
    } else {
        Some(sum / n as f64)
    }
}

/// Cross-lane smile consistency: for each ladder size appearing in ≥2 lanes,
/// the spread (max − min) of the migration `v` the `warp` assigns to those
/// same-size bands. Averaged over sizes. A warp that removes smile maps every
/// occurrence of a rung to the same `v`, driving this toward 0. Unlike
/// [`warp_migration_error`] this needs no ground-truth `v_true` — only that the
/// same size is loaded in multiple lanes — so it scores a *fitted* warp.
pub fn iso_migration_spread(warp: &crate::core::warp::GelWarp, gt: &GroundTruth) -> Option<f64> {
    let mut groups: Vec<(f64, Vec<f64>)> = Vec::new();
    for lane in &gt.lanes {
        let cx = (lane.x_min as f64 + lane.x_max as f64) / 2.0;
        for b in &lane.bands {
            if let Some(s) = b.size {
                let (_, v) = warp.invert(cx, b.y_center);
                match groups.iter_mut().find(|(gs, _)| (*gs - s).abs() < 1e-6) {
                    Some((_, vs)) => vs.push(v),
                    None => groups.push((s, vec![v])),
                }
            }
        }
    }
    let spreads: Vec<f64> = groups
        .iter()
        .filter(|(_, vs)| vs.len() >= 2)
        .map(|(_, vs)| {
            let hi = vs.iter().cloned().fold(f64::MIN, f64::max);
            let lo = vs.iter().cloned().fold(f64::MAX, f64::min);
            hi - lo
        })
        .collect();
    if spreads.is_empty() {
        None
    } else {
        Some(spreads.iter().sum::<f64>() / spreads.len() as f64)
    }
}

/// 1-D interval intersection-over-union.
fn iou(a: (u32, u32), b: (u32, u32)) -> f64 {
    let lo = a.0.max(b.0);
    let hi = a.1.min(b.1);
    let inter = hi.saturating_sub(lo) as f64;
    let union = (a.1.max(b.1) - a.0.min(b.0)) as f64;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

/// Evaluate a detector on one image against its ground truth.
///
/// `band_tol_px` is the y tolerance for calling a detected band a true positive.
pub fn evaluate(
    detector: &dyn GelDetector,
    img: &GrayF32,
    params: &DetectParams,
    gt: &GroundTruth,
    band_tol_px: f64,
) -> EvalMetrics {
    let det = detector.detect(img, params);

    // --- Lane matching (greedy by IoU) ---
    let mut used_pred = vec![false; det.lanes.len()];
    let mut lane_pairs: Vec<(usize, usize)> = Vec::new(); // (gt_idx, pred_idx)
    let mut iou_sum = 0.0;
    for (gi, gl) in gt.lanes.iter().enumerate() {
        let mut best = None;
        let mut best_iou = 0.0;
        for (pi, pl) in det.lanes.iter().enumerate() {
            if used_pred[pi] {
                continue;
            }
            let s = iou((gl.x_min, gl.x_max), (pl.x_min, pl.x_max));
            if s > best_iou {
                best_iou = s;
                best = Some(pi);
            }
        }
        if let Some(pi) = best {
            if best_iou > 0.1 {
                used_pred[pi] = true;
                lane_pairs.push((gi, pi));
                iou_sum += best_iou;
            }
        }
    }
    let lane_iou_mean = if lane_pairs.is_empty() {
        0.0
    } else {
        iou_sum / lane_pairs.len() as f64
    };

    // --- Band matching within matched lanes ---
    let mut tp = 0usize;
    let mut fp = 0usize;
    let mut fn_ = 0usize;
    let mut abs_err_sum = 0.0;
    for (gi, pi) in &lane_pairs {
        let gt_bands = &gt.lanes[*gi].bands;
        let pred_bands: Vec<&crate::detect::detector::DetBand> = det
            .bands
            .iter()
            .filter(|b| b.lane_id == det.lanes[*pi].id)
            .collect();
        let mut matched_pred = vec![false; pred_bands.len()];
        for gb in gt_bands {
            let mut best = None;
            let mut best_d = band_tol_px;
            for (k, pb) in pred_bands.iter().enumerate() {
                if matched_pred[k] {
                    continue;
                }
                let d = (pb.y_center - gb.y_center).abs();
                if d <= best_d {
                    best_d = d;
                    best = Some(k);
                }
            }
            if let Some(k) = best {
                matched_pred[k] = true;
                tp += 1;
                abs_err_sum += best_d;
            } else {
                fn_ += 1;
            }
        }
        fp += matched_pred.iter().filter(|m| !**m).count();
    }
    // Bands in unmatched predicted lanes count as false positives.
    for (pi, pl) in det.lanes.iter().enumerate() {
        if !used_pred[pi] {
            fp += det.bands.iter().filter(|b| b.lane_id == pl.id).count();
        }
    }

    let band_pos_mae = if tp > 0 { abs_err_sum / tp as f64 } else { 0.0 };

    EvalMetrics {
        lane_count_pred: det.lanes.len(),
        lane_count_true: gt.lanes.len(),
        lane_iou_mean,
        band_true_positives: tp,
        band_false_positives: fp,
        band_false_negatives: fn_,
        band_pos_mae,
    }
}

/// Aggregate metrics across a dataset (simple per-image mean of rates plus
/// summed counts).
#[derive(Debug, Clone, Default)]
pub struct AggregateMetrics {
    pub images: usize,
    pub mean_lane_iou: f64,
    pub mean_band_precision: f64,
    pub mean_band_recall: f64,
    pub mean_band_f1: f64,
    pub mean_band_pos_mae: f64,
}

/// Average a set of per-image metrics.
pub fn aggregate(all: &[EvalMetrics]) -> AggregateMetrics {
    if all.is_empty() {
        return AggregateMetrics::default();
    }
    let n = all.len() as f64;
    AggregateMetrics {
        images: all.len(),
        mean_lane_iou: all.iter().map(|m| m.lane_iou_mean).sum::<f64>() / n,
        mean_band_precision: all.iter().map(|m| m.band_precision()).sum::<f64>() / n,
        mean_band_recall: all.iter().map(|m| m.band_recall()).sum::<f64>() / n,
        mean_band_f1: all.iter().map(|m| m.band_f1()).sum::<f64>() / n,
        mean_band_pos_mae: all.iter().map(|m| m.band_pos_mae).sum::<f64>() / n,
    }
}
