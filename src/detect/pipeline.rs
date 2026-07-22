//! End-to-end analysis: detect lanes/bands, identify a ladder lane, and size
//! the remaining bands from the ladder's semi-log fit. Produces a `gel-core`
//! [`Analysis`] ready for the UI. Absolute quantification is deferred until the
//! user supplies ladder concentrations.

use std::collections::BTreeMap;

use crate::core::model::{Analysis, Band, GelType, LadderAssignment, LadderTemplate, Lane};
use crate::core::warp::GelWarp;
use crate::core::{ladders, GrayF32};

use crate::detect::classical::{lane_row_profile, ClassicalDetector};
use crate::detect::detector::{DetectParams, GelDetector};
use crate::detect::ladder_match::{best_template, LadderMatch};

type WarpRungGroups = Vec<(f64, Vec<(f64, f64, f64)>)>;
use crate::detect::signal::subtract_baseline;

/// Re-measure each band's integrated density on the **rectified** image, so a
/// smiling / bent gel does not skew densitometry. Rectified through the warp,
/// lanes are vertical and bands horizontal, so the standard 1-D lane trace
/// applies directly (the "rectify then reuse" path). With the current
/// lane-only warp fit this closely matches the raw-image estimate; it becomes
/// the correction once the warp carries smile (a taller `v` control grid).
fn measure_rectified(
    work: &GrayF32,
    warp: &GelWarp,
    lanes: &[Lane],
    bands: &mut [Band],
    baseline_radius: usize,
) {
    let (rw, rh) = (work.width(), work.height());
    if rw == 0 || rh == 0 {
        return;
    }
    let rect = warp.rectify(work, rw, rh);
    let mut prof: BTreeMap<u32, Vec<f64>> = BTreeMap::new();
    for lane in lanes {
        let x0 = (lane.u_min * rw as f64).clamp(0.0, rw as f64 - 1.0) as usize;
        let x1 = ((lane.u_max * rw as f64).ceil() as usize).clamp(x0 + 1, rw);
        let raw = lane_row_profile(&rect, x0, x1);
        prof.insert(lane.id, subtract_baseline(&raw, baseline_radius));
    }
    for b in bands.iter_mut() {
        if let Some(p) = prof.get(&b.lane_id) {
            let y0 = ((b.v_center - b.v_half_width) * rh as f64).max(0.0) as usize;
            let y1 = (((b.v_center + b.v_half_width) * rh as f64) as usize + 1).min(p.len());
            if y0 < y1 {
                b.integrated_density = p[y0..y1].iter().sum();
            }
        }
    }
}

/// Refine the coarse warp into a **smile** warp using ladder rungs matched
/// across lanes. Each ladder size that appears in ≥2 lanes is a "front" of
/// points known to share migration `v`; fitting a taller control grid so those
/// points map to a single `v` straightens the smile. Returns `None` when no
/// size is shared across ≥2 lanes (smile is then unobservable).
fn fit_smile_warp(
    det: &crate::detect::detector::Detection,
    b0: &[Band],
    per_lane0: &BTreeMap<u32, Vec<usize>>,
    template: &LadderTemplate,
    min_r2: f64,
    dims: (u32, u32),
    params: &DetectParams,
) -> Option<GelWarp> {
    let (width, height) = dims;
    let (w, h) = (width as f64, height as f64);
    let lane_centers: Vec<f64> = det.lanes.iter().map(|l| l.x_center()).collect();
    if lane_centers.len() < 2 {
        return None;
    }
    let lane_pos: std::collections::HashMap<u32, usize> = det
        .lanes
        .iter()
        .enumerate()
        .map(|(i, l)| (l.id, i))
        .collect();
    let nu = lane_centers.len() + 2; // gel edges + one column per lane

    // Match the template to every lane; group matched rungs by ladder size.
    // Each group entry is `(size, [(u, x_px, y_px)])`. Only lanes that are
    // *confidently* this ladder qualify as smile-front sources: a high fit and
    // most of the template's rungs explained. This keeps a lone ladder lane (the
    // common case) from pairing with a spuriously-matched sample lane — which
    // would fabricate a bogus smile.
    let strict_r2 = min_r2.max(0.9);
    let min_rungs = (0.6 * template.bands.len() as f64).ceil() as usize;
    let mut by_size: WarpRungGroups = Vec::new();
    for (&lane_id, idxs) in per_lane0 {
        let positions: Vec<f64> = idxs.iter().map(|&i| b0[i].v_center).collect();
        let Some(m) = best_template(&positions, std::iter::once(template), strict_r2) else {
            continue;
        };
        if m.pairs.len() < min_rungs.max(3) {
            continue;
        }
        let u = (lane_pos[&lane_id] + 1) as f64 / (nu - 1) as f64;
        for pair in &m.pairs {
            let Some(&bi) = idxs.get(pair.band_index) else {
                continue;
            };
            let db = &det.bands[bi];
            let pt = (u, db.x_center, db.y_center);
            match by_size
                .iter_mut()
                .find(|(s, _)| (*s - pair.size).abs() < 1e-6)
            {
                Some((_, pts)) => pts.push(pt),
                None => by_size.push((pair.size, vec![pt])),
            }
        }
    }
    // Fronts: sizes observed in ≥2 lanes.
    let fronts: Vec<Vec<(f64, f64, f64)>> = by_size
        .into_iter()
        .map(|(_, p)| p)
        .filter(|p| p.len() >= 2)
        .collect();
    if fronts.is_empty() {
        return None;
    }

    // Prior grid: columns at the lane centers (+ gel edges), rows uniform in v
    // with y = v·h (flat, no smile yet).
    //
    // TODO(warp-resolution): control columns currently sit only where lanes
    // (and thus ladder rungs) are, so the surface can only bend at lane
    // positions and is merely interpolated in the gaps between them. Add the
    // option of extra knots/columns *between* ladders for finer distortion.
    // Those in-between control points have no ladder-rung constraint, so fit
    // them by combining an energy-minimization (smoothness) term with local
    // optical flow — estimate the gel's local displacement field from the image
    // itself and let flow drive the unconstrained knots while the smoothness
    // energy regularizes them.
    //
    // Compute the flow at a *coarse* scale (heavily downsampled / low-pass), not
    // per pixel: the signal we want is the gentle twisting/curvature of the band
    // fronts across the gel, which lives at low spatial frequency. Fine-scale
    // flow would just track speckle and noise. So the band deformation itself —
    // not pixel texture — is what should drive the flow field.
    let mut xs = Vec::with_capacity(nu);
    xs.push(0.0);
    xs.extend_from_slice(&lane_centers);
    xs.push(w);
    let extra_vertical_edges = params.extra_vertical_edges.max(2);
    let nv = (fronts.len() + extra_vertical_edges).max(3);
    let prior = GelWarp::from_grid(nu, nv, |u, v| {
        let f = u * (nu - 1) as f64;
        let i0 = (f.floor() as usize).min(nu - 1);
        let i1 = (i0 + 1).min(nu - 1);
        let frac = f - i0 as f64;
        [xs[i0] + (xs[i1] - xs[i0]) * frac, v * h]
    });

    // Correspondences: every point in a front shares that front's mean v.
    let mut corr = Vec::new();
    for pts in &fronts {
        let v_mean = pts.iter().map(|(_, _, y)| y / h).sum::<f64>() / pts.len() as f64;
        for &(u, x, y) in pts {
            corr.push((u, v_mean, x, y));
        }
    }
    Some(prior.refine_least_squares_with_spacing(
        &corr,
        params.warp_regularization,
        params.row_spacing_weight,
    ))
}

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
    analyze_detection(det, img, gel_type, params, candidates, min_r2)
}

/// Ladder ID + sizing from an already-computed pixel-space [`Detection`]. Fits
/// the gel warp from the detection, lifts lanes/bands into rectified `(u, v)`,
/// refines densities on the straightened gel, then identifies the ladder and
/// sizes bands. Lets any detector (classical, Cellpose, or a mask-driven /
/// GelGenie segmenter — see [`crate::detect::mask_segment`]) feed the full
/// pipeline.
pub fn analyze_detection(
    det: crate::detect::detector::Detection,
    img: &GrayF32,
    gel_type: GelType,
    params: &DetectParams,
    candidates: &[&LadderTemplate],
    min_r2: f64,
) -> Analysis {
    let (w, h) = (img.width() as u32, img.height() as u32);

    // Candidate templates: caller-provided or built-ins for this gel type.
    let owned_builtins;
    let cand_refs: Vec<&LadderTemplate> = if candidates.is_empty() {
        owned_builtins = ladders::for_gel_type(gel_type);
        owned_builtins.clone()
    } else {
        candidates.to_vec()
    };

    // Pass 1 — coarse (lane-only) warp, giving migration positions for matching.
    // Detection ran first (raw pixels), so there is no circular dependency.
    let coarse = det.fit_warp(w, h);
    let (_, b0) = det.to_model(&coarse);
    let mut per_lane0: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (i, b) in b0.iter().enumerate() {
        per_lane0.entry(b.lane_id).or_default().push(i);
    }
    // The best-matching template drives smile fitting (which rungs share v).
    let template0 = per_lane0
        .values()
        .filter_map(|idxs| {
            let pos: Vec<f64> = idxs.iter().map(|&i| b0[i].v_center).collect();
            best_template(&pos, cand_refs.iter().copied(), min_r2)
        })
        .max_by(best_ladder_match)
        .and_then(|m| {
            cand_refs
                .iter()
                .copied()
                .find(|t| t.name == m.template_name)
        });

    let work = if params.signal_is_bright {
        img.clone()
    } else {
        img.inverted()
    };

    // Pass 2 — refine the warp. With the optical-flow option, recover the band
    // twist directly from the image (works between lanes, no ladder needed);
    // otherwise fit smile from ladder rungs matched across multiple lanes,
    // falling back to the coarse warp when only one ladder lane is present.
    let warp = if params.optical_flow_warp {
        crate::detect::flow::fit_flow_warp(&work, w, h, params.flow_smoothness)
    } else {
        let smile = template0
            .and_then(|t| fit_smile_warp(&det, &b0, &per_lane0, t, min_r2, (w, h), params));
        smile.unwrap_or(coarse)
    };

    let (lanes, mut bands) = det.to_model(&warp);

    // Refine densities on the rectified (straightened) gel.
    measure_rectified(&work, &warp, &lanes, &mut bands, params.baseline_radius);

    let mut analysis = Analysis {
        warp: Some(warp),
        lanes,
        bands,
        ..Default::default()
    };

    // Band indices per lane, kept in detection order (already v-ascending).
    let mut per_lane: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (i, b) in analysis.bands.iter().enumerate() {
        per_lane.entry(b.lane_id).or_default().push(i);
    }

    // Find the lane best explained by a template.
    let mut best: Option<(u32, LadderMatch)> = None;
    for (&lane_id, idxs) in &per_lane {
        let positions: Vec<f64> = idxs.iter().map(|&i| analysis.bands[i].v_center).collect();
        if let Some(m) = best_template(&positions, cand_refs.iter().copied(), min_r2) {
            if best
                .as_ref()
                .is_none_or(|(_, bm)| best_ladder_match(&m, bm).is_gt())
            {
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
                let v = analysis.bands[bi].v_center;
                analysis.bands[bi].size = Some(m.fit.size_at(v));
            }
        }
    }

    analysis
}

fn best_ladder_match(a: &LadderMatch, b: &LadderMatch) -> std::cmp::Ordering {
    a.pairs
        .len()
        .cmp(&b.pairs.len())
        .then_with(|| a.r2.partial_cmp(&b.r2).unwrap())
}
