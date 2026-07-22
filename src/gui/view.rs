//! Rendering application state into the Slint UI. Overlays (lane columns and
//! band bars) are composited directly into the displayed image so they rotate
//! and zoom together with it. The Trace tab's plot is built from Slint `Path`
//! elements (see `app.slint`) so it can carry real axes and labels; this module
//! only produces the path geometry and colors.

use opengel::core::GrayF32;
use slint::{Color, Image, ModelRc, SharedPixelBuffer, SharedString, VecModel};

use crate::state::{AppState, LaneTrace, TraceMode};
use crate::{AppWindow, LadderLaneItem, LaneItem, TracePath, TreeRow};

/// Distinct colors for sample-lane traces (ladders use a fixed gold).
const PALETTE: [(u8, u8, u8); 6] = [
    (33, 118, 214),
    (214, 45, 45),
    (34, 160, 68),
    (150, 60, 200),
    (0, 150, 150),
    (214, 120, 20),
];
const LADDER_RGB: (u8, u8, u8) = (196, 140, 0);

/// A normalized-coordinate overlay rectangle to draw onto the image.
struct OverlayBox {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    ladder: bool,
    is_lane: bool,
}

/// Full refresh: labels, ladder list, band table and composited image.
pub fn refresh(ui: &AppWindow, state: &AppState) {
    ui.set_gel_type_label(format!("{:?}", state.gel_type).into());

    // Ladder template names (for the per-lane and "set all" dropdowns).
    let names: Vec<SharedString> = state
        .ladder_names()
        .into_iter()
        .map(SharedString::from)
        .collect();
    ui.set_ladder_names(ModelRc::new(VecModel::from(names.clone())));

    // Per-ladder-lane controls: any number of ladder lanes, each tunable.
    let ladder_lanes: Vec<LadderLaneItem> = state
        .ladder_lanes()
        .into_iter()
        .map(|(id, label, tidx, load)| LadderLaneItem {
            id: id as i32,
            label: label.into(),
            template_index: tidx,
            load_ng: format!("{load:.0}").into(),
        })
        .collect();
    ui.set_ladder_lanes(ModelRc::new(VecModel::from(ladder_lanes)));

    // Frame selector (HDR merged + each captured exposure).
    let frames: Vec<SharedString> = state
        .frame_labels()
        .into_iter()
        .map(SharedString::from)
        .collect();
    ui.set_frame_names(ModelRc::new(VecModel::from(frames)));
    ui.set_frame_index(state.view_frame_index() as i32);

    let tree: Vec<TreeRow> = state
        .tree_rows()
        .into_iter()
        .map(|r| TreeRow {
            kind: r.kind,
            lane_id: r.lane_id as i32,
            band_id: r.band_id,
            expanded: r.expanded,
            is_ladder: r.is_ladder,
            name: r.name.into(),
            rf: r.rf.into(),
            size: r.size.into(),
            density: r.density.into(),
            ng: r.ng.into(),
            nmol: r.nmol.into(),
        })
        .collect();
    ui.set_tree_rows(ModelRc::new(VecModel::from(tree)));
    refresh_image(ui, state);
    refresh_trace(ui, state);
}

/// Rebuild the Trace tab: lane checklist, mode index, and the plotted paths.
pub fn refresh_trace(ui: &AppWindow, state: &AppState) {
    let items: Vec<LaneItem> = state
        .lane_items()
        .into_iter()
        .map(|(id, label, ladder, selected)| LaneItem {
            id: id as i32,
            label: label.into(),
            ladder,
            selected,
        })
        .collect();
    ui.set_lane_items(ModelRc::new(VecModel::from(items)));
    ui.set_trace_mode_idx(match state.trace_mode {
        TraceMode::Intensity => 0,
        TraceMode::Ng => 1,
        TraceMode::Molarity => 2,
    });

    let traces = state.compute_traces();
    let (paths, xmax, ymax) = build_trace_paths(&traces);
    ui.set_trace_plots(ModelRc::new(VecModel::from(paths)));

    // Five axis ticks each. Y runs top→bottom (index 0 = ymax), X left→right.
    let yticks: Vec<SharedString> = (0..5)
        .map(|i| fmt_tick(ymax as f64 * (4 - i) as f64 / 4.0).into())
        .collect();
    let xticks: Vec<SharedString> = (0..5)
        .map(|i| fmt_tick(xmax as f64 * i as f64 / 4.0).into())
        .collect();
    ui.set_trace_yticks(ModelRc::new(VecModel::from(yticks)));
    ui.set_trace_xticks(ModelRc::new(VecModel::from(xticks)));
    ui.set_trace_xlabel("Migration (px)".into());
    ui.set_trace_ylabel(
        match state.trace_mode {
            TraceMode::Intensity => "Intensity (a.u.)",
            TraceMode::Ng => "Mass (ng)",
            TraceMode::Molarity => "Molarity (nmol)",
        }
        .into(),
    );
}

/// Short, human-friendly axis-tick label.
fn fmt_tick(v: f64) -> String {
    if v == 0.0 {
        "0".into()
    } else if v.abs() >= 100.0 {
        format!("{v:.0}")
    } else if v.abs() >= 1.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.3}")
    }
}

/// Build one SVG-path polyline per selected lane in a fixed `0..1000` viewbox
/// (x = migration left→right, y = value bottom→top). Returns the paths plus the
/// data extents used to label the axes.
fn build_trace_paths(traces: &[LaneTrace]) -> (Vec<TracePath>, f32, f32) {
    let mut xmax = 0usize;
    let mut vmax = 0.0f64;
    for t in traces {
        xmax = xmax.max(t.values.len());
        for &v in &t.values {
            if v > vmax {
                vmax = v;
            }
        }
    }
    let vmax = vmax.max(1e-9);
    let mut out = Vec::new();
    let mut sample_i = 0usize;
    for t in traces {
        let n = t.values.len();
        if n < 2 {
            continue;
        }
        let (r, g, b) = if t.ladder {
            LADDER_RGB
        } else {
            let c = PALETTE[sample_i % PALETTE.len()];
            sample_i += 1;
            c
        };
        let mut cmds = String::with_capacity(n * 12);
        for (k, &v) in t.values.iter().enumerate() {
            let x = 1000.0 * k as f64 / (n - 1) as f64;
            let y = 1000.0 * (1.0 - v / vmax);
            cmds.push_str(if k == 0 { "M " } else { "L " });
            cmds.push_str(&format!("{x:.1} {y:.1} "));
        }
        out.push(TracePath {
            commands: cmds.into(),
            color: Color::from_rgb_u8(r, g, b),
            label: t.label.clone().into(),
            ladder: t.ladder,
        });
    }
    (out, xmax as f32, vmax as f32)
}

/// Re-render the gel image (selected frame or merged HDR) with the current
/// contrast window, inversion and overlays. Zoom/rotation are applied live by
/// the UI. Also refreshes the histogram thumbnail for the contrast control.
pub fn refresh_image(ui: &AppWindow, state: &AppState) {
    if state.show_unwarped {
        // Dewarped: show the rectified gel with overlays in rectified (u,v)
        // coordinates (lanes vertical, bands horizontal).
        if let Some(rect) = state.unwarped_view() {
            let overlays = compute_overlays_unwarped(state);
            let img = to_slint_image(&rect, state.disp_lo, state.disp_hi, state.invert, &overlays);
            ui.set_gel_image(img);
        }
    } else if let Some(work) = state.display_gray() {
        let overlays = compute_overlays(state);
        let img = to_slint_image(&work, state.disp_lo, state.disp_hi, state.invert, &overlays);
        ui.set_gel_image(img);
    }
    ui.set_frame_index(state.view_frame_index() as i32);
    let hist = state.histogram(160);
    ui.set_histogram_image(render_histogram(&hist, state.disp_lo, state.disp_hi, 240, 60));
}

fn compute_overlays(state: &AppState) -> Vec<OverlayBox> {
    let mut out = Vec::new();
    let (Some(a), Some(work)) = (state.analysis(), state.view_image()) else {
        return out;
    };
    let w = work.width().max(1) as f32;
    // Overlays are axis-aligned bounding rectangles of each lane/band footprint,
    // derived from the fitted warp (identity when none). Drawing the true curved
    // strip/smile outline is a later refinement.
    let warp = a.warp_or_identity(work.width() as u32, work.height() as u32);
    for lane in &a.lanes {
        let (x0, x1) = lane.px_x_bounds(&warp);
        out.push(OverlayBox {
            x: x0 as f32 / w,
            y: 0.0,
            w: (x1 - x0) as f32 / w,
            h: 1.0,
            ladder: lane.is_ladder,
            is_lane: true,
        });
    }
    for b in &a.bands {
        if let Some(lane) = a.lanes.iter().find(|l| l.id == b.lane_id) {
            let (x0, x1) = lane.px_x_bounds(&warp);
            out.push(OverlayBox {
                x: x0 as f32 / w,
                y: (b.v_center - b.v_half_width) as f32,
                w: (x1 - x0) as f32 / w,
                h: (2.0 * b.v_half_width) as f32,
                ladder: lane.is_ladder,
                is_lane: false,
            });
        }
    }
    out
}

/// Overlays for the dewarped view: in rectified space the lane strip and band
/// footprint are axis-aligned, so `(u, v)` map directly to the overlay's
/// normalized rectangle.
fn compute_overlays_unwarped(state: &AppState) -> Vec<OverlayBox> {
    let mut out = Vec::new();
    let Some(a) = state.analysis() else {
        return out;
    };
    for lane in &a.lanes {
        out.push(OverlayBox {
            x: lane.u_min as f32,
            y: 0.0,
            w: (lane.u_max - lane.u_min) as f32,
            h: 1.0,
            ladder: lane.is_ladder,
            is_lane: true,
        });
    }
    for b in &a.bands {
        if let Some(lane) = a.lanes.iter().find(|l| l.id == b.lane_id) {
            out.push(OverlayBox {
                x: lane.u_min as f32,
                y: (b.v_center - b.v_half_width) as f32,
                w: (lane.u_max - lane.u_min) as f32,
                h: (2.0 * b.v_half_width) as f32,
                ladder: lane.is_ladder,
                is_lane: false,
            });
        }
    }
    out
}

/// Convert a working image to a displayable RGB [`Image`] with overlays drawn
/// on top. The contrast window maps `[min + lo·span, min + hi·span]` → `[0,255]`
/// (span = max − min); `invert` flips the result.
fn to_slint_image(
    work: &GrayF32,
    lo_frac: f32,
    hi_frac: f32,
    invert: bool,
    overlays: &[OverlayBox],
) -> Image {
    let (w, h) = (work.width() as u32, work.height() as u32);
    let (mn, mx) = work.min_max();
    let span = (mx - mn).max(1e-6);
    let black = mn + lo_frac * span;
    let white = mn + hi_frac * span;
    let denom = (white - black).max(1e-6);
    let mut buf = SharedPixelBuffer::<slint::Rgb8Pixel>::new(w, h);
    let px = buf.make_mut_slice();
    for y in 0..h as usize {
        for x in 0..w as usize {
            let mut v = ((work.get(x, y) - black) / denom).clamp(0.0, 1.0);
            if invert {
                v = 1.0 - v;
            }
            let g = (v * 255.0) as u8;
            px[y * w as usize + x] = slint::Rgb8Pixel { r: g, g, b: g };
        }
    }
    for ov in overlays {
        draw_overlay(px, w, h, ov);
    }
    Image::from_rgb8(buf)
}

/// Render the intensity histogram of the displayed image as a small thumbnail,
/// with the selected contrast window `[lo, hi]` highlighted.
fn render_histogram(hist: &[u32], lo: f32, hi: f32, w: u32, h: u32) -> Image {
    let mut buf = SharedPixelBuffer::<slint::Rgb8Pixel>::new(w, h);
    let px = buf.make_mut_slice();
    let bg = slint::Rgb8Pixel { r: 250, g: 250, b: 250 };
    let win = slint::Rgb8Pixel { r: 222, g: 236, b: 252 };
    for p in px.iter_mut() {
        *p = bg;
    }
    let n = hist.len().max(1);
    // Log-scaled peak so faint bins stay visible next to a dominant background.
    let peak = hist.iter().copied().max().unwrap_or(1).max(1) as f64;
    let lpeak = (1.0 + peak).ln();
    let (lo_x, hi_x) = ((lo * w as f32) as u32, (hi * w as f32) as u32);
    for x in 0..w {
        // Window highlight band behind the bars.
        if x >= lo_x && x <= hi_x {
            for y in 0..h {
                px[(y * w + x) as usize] = win;
            }
        }
        let bin = (x as usize * n / w as usize).min(n - 1);
        let frac = (1.0 + hist[bin] as f64).ln() / lpeak;
        let bar = ((frac * (h as f64 - 2.0)).round() as u32).min(h - 1);
        for y in (h - bar)..h {
            px[(y * w + x) as usize] = slint::Rgb8Pixel { r: 90, g: 90, b: 96 };
        }
    }
    Image::from_rgb8(buf)
}

fn draw_overlay(px: &mut [slint::Rgb8Pixel], w: u32, h: u32, ov: &OverlayBox) {
    let (wf, hf) = (w as f32, h as f32);
    let x0 = (ov.x * wf).round().clamp(0.0, wf - 1.0) as u32;
    let y0 = (ov.y * hf).round().clamp(0.0, hf - 1.0) as u32;
    let x1 = ((ov.x + ov.w) * wf).round().clamp(0.0, wf) as u32;
    let y1 = ((ov.y + ov.h) * hf).round().clamp(0.0, hf) as u32;
    let (cr, cg, cb) = if ov.ladder {
        (255u8, 204u8, 0u8)
    } else {
        (51u8, 187u8, 255u8)
    };
    let fill = if ov.is_lane { 0.10 } else { 0.30 };
    for y in y0..y1 {
        for x in x0..x1 {
            let i = (y * w + x) as usize;
            let on_border = x == x0 || x + 1 == x1 || y == y0 || y + 1 == y1;
            let a = if on_border { 0.9 } else { fill };
            let p = &mut px[i];
            p.r = blend(p.r, cr, a);
            p.g = blend(p.g, cg, a);
            p.b = blend(p.b, cb, a);
        }
    }
}

#[inline]
fn blend(bg: u8, fg: u8, a: f32) -> u8 {
    (bg as f32 * (1.0 - a) + fg as f32 * a).round().clamp(0.0, 255.0) as u8
}
