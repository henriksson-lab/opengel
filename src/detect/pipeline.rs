//! End-to-end analysis: detect lanes/bands, identify a ladder lane, and size
//! the remaining bands from the ladder's semi-log fit. Produces a `gel-core`
//! [`Analysis`] ready for the UI. Absolute quantification is deferred until the
//! user supplies ladder concentrations.

use std::collections::BTreeMap;

use crate::core::model::{Analysis, Band, GelType, LadderAssignment, LadderTemplate, Lane};
use crate::core::quant::SizingFit;
use crate::core::warp::GelWarp;
use crate::core::{ladders, GrayF32};

use crate::detect::classical::{lane_row_profile, ClassicalDetector};
use crate::detect::detector::{DetectParams, GelDetector};
use crate::detect::ladder_match::{best_template, best_template_anchored, LadderMatch};

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
    // Control-row count is fixed and low: the ladder fronts are least-squares
    // *data* (see `corr` below), not control rows. Pinning one row per front
    // (fronts can be 10+) over-parameterizes the surface — it clutters the grid
    // with knots and overfits band-position noise into a wiggly warp. A smile is
    // a smooth, low-order distortion in v, so a few interior rows suffice; all
    // fronts still constrain them. `extra_vertical_edges` tunes the count.
    let nv = (params.extra_vertical_edges + 1).max(3);
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

/// Fit the gel warp so its iso-migration lines **follow the measured band
/// tilts**. Each band contributes two correspondences — its left and right ends,
/// offset from the band center by half the lane width along the band's angle — so
/// the surface is pinned to the band's position *and* local slope. Bands span the
/// full height and every lane, so this constrains the smile everywhere, unlike
/// the ladder-rung fit (only at matched rungs) — no ladder needed. Columns sit at
/// the lane centers (+ gel edges); a few `v` rows keep the surface smooth, with
/// regularization filling the gaps between bands. `None` if there are too few
/// bands to constrain a fit.
#[allow(clippy::too_many_arguments)]
fn fit_band_warp(
    det: &crate::detect::detector::Detection,
    b0: &[Band],
    per_lane0: &BTreeMap<u32, Vec<usize>>,
    template0: Option<&LadderTemplate>,
    min_r2: f64,
    dims: (u32, u32),
    params: &DetectParams,
) -> Option<GelWarp> {
    use std::collections::HashMap;
    let (width, height) = dims;
    let (w, h) = (width as f64, height as f64);
    let lane_centers: Vec<f64> = det.lanes.iter().map(|l| l.x_center()).collect();
    if lane_centers.is_empty() || det.bands.len() < 2 {
        return None;
    }
    let lane_pos: HashMap<u32, usize> =
        det.lanes.iter().enumerate().map(|(i, l)| (l.id, i)).collect();

    // Rung alignment: matched ladder rungs of the same size across ≥2 lanes must
    // share one migration `v`, so the warp straightens the smile for cross-lane
    // quantification (not just following per-band tilts). Build band_id → shared v.
    let mut shared_v: HashMap<u32, f64> = HashMap::new();
    if let Some(template) = template0 {
        let strict_r2 = min_r2.max(0.9);
        let min_rungs = (0.6 * template.bands.len() as f64).ceil() as usize;
        let mut by_size: Vec<(f64, Vec<(u32, f64)>)> = Vec::new();
        for idxs in per_lane0.values() {
            let positions: Vec<f64> = idxs.iter().map(|&i| b0[i].v_center).collect();
            let Some(m) = best_template(&positions, std::iter::once(template), strict_r2) else {
                continue;
            };
            if m.pairs.len() < min_rungs.max(3) {
                continue;
            }
            for pair in &m.pairs {
                let Some(&bi) = idxs.get(pair.band_index) else {
                    continue;
                };
                let entry = (det.bands[bi].id, b0[bi].v_center);
                match by_size.iter_mut().find(|(s, _)| (*s - pair.size).abs() < 1e-6) {
                    Some((_, pts)) => pts.push(entry),
                    None => by_size.push((pair.size, vec![entry])),
                }
            }
        }
        for (_, pts) in by_size.into_iter().filter(|(_, p)| p.len() >= 2) {
            let mean_v = pts.iter().map(|(_, v)| v).sum::<f64>() / pts.len() as f64;
            for (id, _) in pts {
                shared_v.insert(id, mean_v);
            }
        }
    }
    // Lane *anchors* fix each lane's cross-lane position in u, independent of the
    // control-grid resolution: gel edges at u=0/1, lane p at u=(p+1)/(base_nu-1).
    // Extra horizontal edges only densify the grid (more bending freedom between
    // lanes); they must not move where a lane sits in u, or the band
    // correspondences below would drift.
    let base_nu = lane_centers.len() + 2; // gel edges + one column per lane
    let du_lane = 1.0 / (base_nu - 1) as f64; // u-spacing between adjacent anchors
    let mut anchor_u = Vec::with_capacity(base_nu);
    let mut anchor_x = Vec::with_capacity(base_nu);
    anchor_u.push(0.0);
    anchor_x.push(0.0);
    for (p, &xc) in lane_centers.iter().enumerate() {
        anchor_u.push((p + 1) as f64 * du_lane);
        anchor_x.push(xc);
    }
    anchor_u.push(1.0);
    anchor_x.push(w);
    // Piecewise-linear prior x at any u, interpolating the lane/edge anchors.
    let prior_x = |u: f64| -> f64 {
        let u = u.clamp(0.0, 1.0);
        let mut k = 0;
        while k + 1 < anchor_u.len() && anchor_u[k + 1] < u {
            k += 1;
        }
        let (u0, u1) = (anchor_u[k], anchor_u[k + 1]);
        let (x0, x1) = (anchor_x[k], anchor_x[k + 1]);
        let frac = if (u1 - u0).abs() < 1e-12 {
            0.0
        } else {
            (u - u0) / (u1 - u0)
        };
        x0 + (x1 - x0) * frac
    };
    // Grid resolution: densify by the requested extra edges (0 = one col/lane).
    let nu = base_nu + params.extra_horizontal_edges;
    let nv = (params.extra_vertical_edges + 3).max(4);
    let prior = GelWarp::from_grid(nu, nv, |u, v| [prior_x(u), v * h]);

    // Pin the top (v=0) and bottom (v=1) control rows to the image boundary at
    // every column, so the migration axis spans the image and — crucially — the
    // outer knots stay on-screen and grabbable (bands don't reach the very top/
    // bottom, so without this the edge rows float past the border and the user
    // can only grab inner knots, and normalize-inner never fires).
    let mut corr: Vec<(f64, f64, f64, f64)> = Vec::with_capacity(det.bands.len() * 2 + nu * 2);
    for iu in 0..nu {
        let u = iu as f64 / (nu - 1) as f64;
        let x = prior_x(u);
        corr.push((u, 0.0, x, 0.0));
        corr.push((u, 1.0, x, h));
    }

    // Two correspondences per band, at the lane's left/right in the *prior*
    // x-geometry (so the u→x map is never distorted); only the migration y is
    // shaped, sloped by the band's measured tilt so the iso-v line follows it.
    for (i, db) in det.bands.iter().enumerate() {
        let Some(&p) = lane_pos.get(&db.lane_id) else {
            continue;
        };
        let u_c = (p + 1) as f64 * du_lane;
        // Matched rungs share a common v (straighten); others keep their own.
        let v = shared_v
            .get(&db.id)
            .copied()
            .unwrap_or(b0[i].v_center)
            .clamp(0.0, 1.0);
        let t = db.angle.tan();
        let u_l = (u_c - du_lane * 0.45).max(0.0);
        let u_r = (u_c + du_lane * 0.45).min(1.0);
        let (xl, xr) = (prior_x(u_l), prior_x(u_r));
        corr.push((u_l, v, xl, db.y_center + (xl - db.x_center) * t));
        corr.push((u_r, v, xr, db.y_center + (xr - db.x_center) * t));
    }
    if corr.len() < 4 {
        return None;
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
        // Make the grid follow the measured band tilts (full height, between
        // lanes — no ladder needed). Fall back to the ladder-rung smile, then to
        // the coarse lane-only warp.
        fit_band_warp(&det, &b0, &per_lane0, template0, min_r2, (w, h), params)
            .or_else(|| {
                template0.and_then(|t| fit_smile_warp(&det, &b0, &per_lane0, t, min_r2, (w, h), params))
            })
            .unwrap_or(coarse)
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
        // Known-weight anchors (user-set sizes) are the primary cue: any band in
        // this lane already carrying a `known_size` pins that rung, resolving
        // off-by-one ambiguity when the first/last rung is missing.
        let anchors: Vec<(usize, f64)> = idxs
            .iter()
            .enumerate()
            .filter_map(|(local, &bi)| analysis.bands[bi].known_size.map(|s| (local, s)))
            .collect();
        if let Some(m) =
            best_template_anchored(&positions, cand_refs.iter().copied(), min_r2, &anchors)
        {
            if best
                .as_ref()
                .is_none_or(|(_, bm)| best_ladder_match(&m, bm).is_gt())
            {
                best = Some((lane_id, m));
            }
        }
    }

    if let Some((best_lane, best_m)) = best {
        let template = cand_refs.iter().copied().find(|t| t.name == best_m.template_name);

        // Every lane that matches the *winning* template well is a ladder lane.
        // Each lane's missing/merged rungs are solved independently — a rotated
        // gel or a lane running slightly fast/slow can drop or merge a *different*
        // subset — and then reconciled by pooling all lanes' rung inliers into one
        // shared sizing fit (they share one gel migration model).
        let mut ladder_lanes: Vec<(u32, LadderMatch)> = Vec::new();
        for (&lane_id, idxs) in &per_lane {
            let m = if lane_id == best_lane {
                best_m.clone()
            } else if let Some(t) = template {
                let positions: Vec<f64> =
                    idxs.iter().map(|&i| analysis.bands[i].v_center).collect();
                let anchors: Vec<(usize, f64)> = idxs
                    .iter()
                    .enumerate()
                    .filter_map(|(local, &bi)| analysis.bands[bi].known_size.map(|s| (local, s)))
                    .collect();
                // Same strict bar as detection (r2 ≥ min_r2) plus enough rungs, so
                // sample lanes aren't mistaken for ladders.
                match best_template_anchored(&positions, std::iter::once(t), min_r2, &anchors) {
                    Some(m) if m.pairs.len() >= 4 => m,
                    _ => continue,
                }
            } else {
                continue;
            };
            ladder_lanes.push((lane_id, m));
        }

        // Pool rung inliers (rectified migration ↔ size) across all ladder lanes,
        // excluding merged blobs (their centroid biases the fit), into one fit.
        let mut pooled: Vec<(f64, f64)> = Vec::new();
        for (lane_id, m) in &ladder_lanes {
            let idxs = &per_lane[lane_id];
            for pair in &m.pairs {
                if m.merged.iter().any(|mr| mr.band_index == pair.band_index) {
                    continue;
                }
                if let Some(&bi) = idxs.get(pair.band_index) {
                    pooled.push((analysis.bands[bi].v_center, pair.size));
                }
            }
        }
        let shared_fit = SizingFit::fit(&pooled).unwrap_or(best_m.fit);

        // Mark ladder lanes and assign known sizes to their matched bands.
        let ladder_ids: Vec<u32> = ladder_lanes.iter().map(|(id, _)| *id).collect();
        for (lane_id, m) in &ladder_lanes {
            if let Some(lane) = analysis.lanes.iter_mut().find(|l| l.id == *lane_id) {
                lane.is_ladder = true;
                lane.label.get_or_insert_with(|| m.template_name.clone());
            }
            let idxs = &per_lane[lane_id];
            let mut rung_to_band = Vec::with_capacity(m.pairs.len());
            for pair in &m.pairs {
                if let Some(&bi) = idxs.get(pair.band_index) {
                    analysis.bands[bi].known_size = Some(pair.size);
                    analysis.bands[bi].size = Some(pair.size);
                    analysis.bands[bi].merged_sizes.clear();
                    rung_to_band.push(Some(analysis.bands[bi].id));
                } else {
                    rung_to_band.push(None);
                }
            }
            // Record rungs that merged into an already-matched blob (unresolved
            // large fragments) so the UI can show "N + M bp".
            for mr in &m.merged {
                if let Some(&bi) = idxs.get(mr.band_index) {
                    analysis.bands[bi].merged_sizes.push(mr.size);
                }
            }
            analysis.ladder_assignments.push(LadderAssignment {
                lane_id: *lane_id,
                template_name: m.template_name.clone(),
                rung_to_band,
            });
        }

        // Size every non-ladder band from the shared (co-fitted) semi-log fit.
        for (&lid, idxs) in &per_lane {
            if ladder_ids.contains(&lid) {
                continue;
            }
            for &bi in idxs {
                let v = analysis.bands[bi].v_center;
                analysis.bands[bi].size = Some(shared_fit.size_at(v));
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






