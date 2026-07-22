//! End-to-end analysis: detect lanes/bands, identify a ladder lane, and size
//! the remaining bands from the ladder's semi-log fit. Produces a `gel-core`
//! [`Analysis`] ready for the UI. Absolute quantification is deferred until the
//! user supplies ladder concentrations.

use std::collections::BTreeMap;

use crate::core::model::{Analysis, GelType, LadderAssignment, LadderTemplate};
use crate::core::{ladders, GrayF32};

use crate::detect::classical::ClassicalDetector;
use crate::detect::detector::{DetectParams, GelDetector};
use crate::detect::ladder_match::{best_template, LadderMatch};

/// Run classical detection + ladder identification + sizing.
///
/// `candidates` are the ladder templates to consider (defaults to the built-ins
/// for `gel_type` when empty). `min_r2` gates ladder identification.
pub fn analyze(
    img: &GrayF32,
    gel_type: GelType,
    params: &DetectParams,
    candidates: &[&LadderTemplate],
    min_r2: f64,
) -> Analysis {
    let det = ClassicalDetector::new().detect(img, params);
    analyze_detection(det, gel_type, candidates, min_r2)
}

/// Ladder ID + sizing from an already-computed [`Detection`]. Lets any detector
/// (classical, Cellpose, or a mask-driven / GelGenie segmenter — see
/// [`crate::detect::mask_segment`]) feed the full analysis pipeline.
pub fn analyze_detection(
    det: crate::detect::detector::Detection,
    gel_type: GelType,
    candidates: &[&LadderTemplate],
    min_r2: f64,
) -> Analysis {
    let mut analysis = Analysis {
        lanes: det.lanes,
        bands: det.bands,
        ..Default::default()
    };

    // Band indices per lane, kept in detection order (already y-ascending).
    let mut per_lane: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (i, b) in analysis.bands.iter().enumerate() {
        per_lane.entry(b.lane_id).or_default().push(i);
    }

    // Candidate templates: caller-provided or built-ins for this gel type.
    let owned_builtins;
    let cand_refs: Vec<&LadderTemplate> = if candidates.is_empty() {
        owned_builtins = ladders::for_gel_type(gel_type);
        owned_builtins.clone()
    } else {
        candidates.to_vec()
    };

    // Find the lane best explained by a template.
    let mut best: Option<(u32, LadderMatch)> = None;
    for (&lane_id, idxs) in &per_lane {
        let positions: Vec<f64> = idxs.iter().map(|&i| analysis.bands[i].y_center).collect();
        if let Some(m) = best_template(&positions, cand_refs.iter().copied(), min_r2) {
            if best.as_ref().map_or(true, |(_, bm)| m.r2 > bm.r2) {
                best = Some((lane_id, m));
            }
        }
    }

    if let Some((lane_id, m)) = best {
        // Mark the ladder lane and assign known sizes to its matched bands.
        if let Some(lane) = analysis.lanes.iter_mut().find(|l| l.id == lane_id) {
            lane.is_ladder = true;
            lane.label.get_or_insert_with(|| m.template_name.clone());
        }
        let idxs = &per_lane[&lane_id];
        let mut rung_to_band = Vec::with_capacity(m.pairs.len());
        for pair in &m.pairs {
            if let Some(&bi) = idxs.get(pair.band_index) {
                analysis.bands[bi].known_size = Some(pair.size);
                analysis.bands[bi].size = Some(pair.size);
                rung_to_band.push(Some(analysis.bands[bi].id));
            } else {
                rung_to_band.push(None);
            }
        }
        analysis.ladder_assignments.push(LadderAssignment {
            lane_id,
            template_name: m.template_name.clone(),
            rung_to_band,
        });

        // Size every non-ladder band from the ladder's semi-log fit.
        for (&lid, idxs) in &per_lane {
            if lid == lane_id {
                continue;
            }
            for &bi in idxs {
                let y = analysis.bands[bi].y_center;
                analysis.bands[bi].size = Some(m.fit.size_at(y));
            }
        }
    }

    analysis
}
