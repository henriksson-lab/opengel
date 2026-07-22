//! Rendering application state into the Slint UI. Overlays (lane columns and
//! band bars) are composited directly into the displayed image so they rotate
//! and zoom together with it. The Trace tab's plot is built from Slint `Path`
//! elements (see `app.slint`) so it can carry real axes and labels; this module
//! only produces the path geometry and colors.

use opengel::core::GrayF32;
use slint::{Color, Image, ModelRc, SharedPixelBuffer, SharedString, VecModel};

use crate::state::{AppState, LaneTrace, TraceMode};
use crate::{AppWindow, LaneItem, TracePath, TreeRow};

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
    selected: bool,
}

/// Full refresh: labels, ladder names, tree and composited image.
pub fn refresh(ui: &AppWindow, state: &AppState) {
    ui.set_gel_type_label(format!("{:?}", state.gel_type).into());

    // Ladder template names (for the "Use as ladder" dialog dropdown).
    let names: Vec<SharedString> = state
        .ladder_names()
        .into_iter()
        .map(SharedString::from)
        .collect();
    ui.set_ladder_names(ModelRc::new(VecModel::from(names.clone())));

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

// The windowed grayscale base and the histogram are invariant to mouse hover
// (they depend only on the frame + contrast window, not the pointer). Caching
// them means a hover/drag only clones the base and redraws the light overlays +
// warp line, instead of re-windowing the whole image and recomputing the
// histogram every mouse-move event. Single UI thread → thread_local is fine.
type BaseKey = (u64, usize, u32, u32, bool, bool, u32, u32); // doc_gen, frame, lo, hi, invert, overexp, w, h
type HistKey = (u64, usize, u32, u32);                       // doc_gen, frame, lo, hi
thread_local! {
    static BASE_CACHE: std::cell::RefCell<Option<(BaseKey, SharedPixelBuffer<slint::Rgb8Pixel>)>> =
        const { std::cell::RefCell::new(None) };
    static HIST_CACHE: std::cell::RefCell<Option<HistKey>> = const { std::cell::RefCell::new(None) };
}

/// Re-render the gel image (selected frame or merged HDR) with the current
/// contrast window, inversion and overlays. Zoom/rotation are applied live by
/// the UI. Also refreshes the histogram thumbnail for the contrast control.
///
/// The windowed base image and the histogram are cached (keyed by frame +
/// window), so a hover or drag only clones the cached base and redraws overlays
/// + the warp line — not the full windowing pass + histogram every event.
pub fn refresh_image(ui: &AppWindow, state: &AppState) {
    // Dewarped view: a fixed, straightened render with overlays in rectified
    // (u,v) space (lanes vertical, bands horizontal). No zoom/pan/rotation/warp
    // grid — just the rectified gel and its annotation boxes.
    if state.show_unwarped {
        if let Some(rect) = state.unwarped_view() {
            let (w, h) = (rect.width() as u32, rect.height() as u32);
            let mut buf =
                window_gray(&rect, state.disp_lo, state.disp_hi, state.invert, state.show_overexposed);
            {
                let px = buf.make_mut_slice();
                for ov in &compute_overlays_unwarped(state) {
                    draw_overlay(px, w, h, ov, state.annotation_alpha);
                }
            }
            ui.set_gel_image(Image::from_rgb8(buf));
        }
        ui.set_frame_index(state.view_frame_index() as i32);
        return;
    }
    if let Some(work) = state.display_gray() {
        let (w, h) = (work.width() as u32, work.height() as u32);
        let base_key: BaseKey = (
            state.doc_gen,
            state.view_frame_index(),
            state.disp_lo.to_bits(),
            state.disp_hi.to_bits(),
            state.invert,
            state.show_overexposed,
            w,
            h,
        );
        // Cached windowed base, or recompute it if the frame/window changed.
        let mut buf = BASE_CACHE.with(|c| {
            let mut c = c.borrow_mut();
            if let Some((k, b)) = c.as_ref() {
                if *k == base_key {
                    return b.clone();
                }
            }
            let b = window_gray(&work, state.disp_lo, state.disp_hi, state.invert, state.show_overexposed);
            *c = Some((base_key, b.clone()));
            b
        });
        // Overlays + warp/line are cheap; always redraw them on top of the base.
        {
            let px = buf.make_mut_slice();
            for ov in &compute_overlays(state) {
                draw_overlay(px, w, h, ov, state.annotation_alpha);
            }
        }
        draw_warp(&mut buf, state);
        ui.set_gel_image(Image::from_rgb8(buf));
    }
    ui.set_frame_index(state.view_frame_index() as i32);

    // Histogram: only recompute when the frame or window changed (never on hover).
    let hist_key: HistKey = (
        state.doc_gen,
        state.view_frame_index(),
        state.disp_lo.to_bits(),
        state.disp_hi.to_bits(),
    );
    let need_hist = HIST_CACHE.with(|c| c.borrow().map_or(true, |k| k != hist_key));
    if need_hist {
        let hist = state.histogram(256);
        ui.set_histogram_image(render_histogram(&hist, state.disp_lo, state.disp_hi, 1024, 120));
        HIST_CACHE.with(|c| *c.borrow_mut() = Some(hist_key));
    }
}

/// Composite the fitted warp onto the gel buffer: the iso-parameter grid (when
/// "Show warp model" is on) and, always, the migration-alignment line through
/// the mouse pointer (the iso-`v` curve at the hovered migration level).
fn draw_warp(buf: &mut SharedPixelBuffer<slint::Rgb8Pixel>, state: &AppState) {
    if !state.show_warp && state.hover_x < 0.0 {
        return;
    }
    let Some(warp) = state.fit_warp() else {
        return;
    };
    let (w, h) = (buf.width(), buf.height());
    let px = buf.make_mut_slice();
    let grid = (150u8, 150u8, 160u8);
    if state.show_warp {
        for i in 0..=6 {
            let t = i as f64 / 6.0;
            polyline(px, w, h, &warp.iso_u(t, 48), grid);
            polyline(px, w, h, &warp.iso_v(t, 48), grid);
        }
        // Draggable control-point handles on top of the grid.
        let knots = state.warp_knots();
        let dragging = state.dragging_knot;
        let (nu, _nv) = warp.grid_size();
        for (i, &(nx, ny)) in knots.iter().enumerate() {
            let cx = (nx * w as f32).round() as i32;
            let cy = (ny * h as f32).round() as i32;
            let active = dragging == Some((i % nu.max(1), i / nu.max(1)));
            // Active knot is larger and orange; others are cyan handles.
            let (color, r) = if active {
                ((255u8, 150u8, 0u8), 6i32)
            } else {
                ((0u8, 210u8, 255u8), 5i32)
            };
            fill_handle(px, w, h, cx, cy, r, color);
        }
    }
    // Alignment line at the hovered migration level.
    if state.hover_x >= 0.0 && state.hover_y >= 0.0 {
        let (hx, hy) = (state.hover_x as f64 * w as f64, state.hover_y as f64 * h as f64);
        let (_, v0) = warp.invert(hx, hy);
        polyline(px, w, h, &warp.iso_v(v0, 96), (0, 210, 255));
    }
}

/// Draw a filled square knot handle with a dark 1px border, clipped to bounds.
fn fill_handle(px: &mut [slint::Rgb8Pixel], w: u32, h: u32, cx: i32, cy: i32, r: i32, c: (u8, u8, u8)) {
    for dy in -r..=r {
        for dx in -r..=r {
            let x = cx + dx;
            let y = cy + dy;
            if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                continue;
            }
            let edge = dx.abs() == r || dy.abs() == r;
            let (rr, gg, bb) = if edge { (20, 20, 20) } else { c };
            let idx = y as usize * w as usize + x as usize;
            px[idx] = slint::Rgb8Pixel { r: rr, g: gg, b: bb };
        }
    }
}

/// Refresh the Live tab: camera name, running state, status, preview image.
pub fn refresh_live(ui: &AppWindow, state: &AppState) {
    ui.set_camera_name(state.camera_name.clone().into());
    ui.set_live_running(state.live_running);
    ui.set_live_status(
        if state.live_running {
            "Previewing…"
        } else {
            "Idle."
        }
        .into(),
    );
    if let Some(p) = state.preview_image() {
        ui.set_live_preview_image(to_slint_image(p, 0.0, 1.0, state.invert, &[]));
    }
    // Live histogram of the preview frame (exposure aid). Rendered wide so it
    // isn't upscaled/blurry when stretched across the column.
    let hist = state.preview_histogram(256);
    ui.set_live_histogram_image(render_histogram(&hist, 0.0, 1.0, 1024, 120));

    // Exposure controls: current exposure, HDR bounds, step count, covered EV.
    ui.set_live_exposure_slider(state.live_exposure_slider());
    ui.set_live_exposure_label(fmt_seconds(state.live_exposure_s).into());
    ui.set_hdr_min_label(fmt_seconds(state.hdr_min_s).into());
    ui.set_hdr_max_label(fmt_seconds(state.hdr_max_s).into());
    ui.set_hdr_range_label(format!("{:.1} EV", state.hdr_range_ev()).into());
    ui.set_hdr_steps_idx(state.hdr_steps_idx() as i32);

    // Camera selection dropdown.
    let names: Vec<SharedString> = state.cameras.iter().map(SharedString::from).collect();
    ui.set_camera_names(ModelRc::new(VecModel::from(names)));
    ui.set_camera_index(state.selected_camera as i32);

    // Capture progress dialog.
    ui.set_capture_active(state.capturing);
    ui.set_capture_status(state.capture_status.clone().into());

    // Manual-exposure capability gates the exposure slider + HDR capture.
    ui.set_exposure_supported(state.exposure_supported);
}

/// Human-readable exposure time: milliseconds under 1 s, else seconds.
fn fmt_seconds(t: f64) -> String {
    if t < 1.0 {
        format!("{:.0} ms", t * 1000.0)
    } else {
        format!("{t:.2} s")
    }
}

/// Overlays for the dewarped view: in rectified space `(u, v)` map directly to
/// the overlay rectangle (lanes vertical, bands horizontal).
fn compute_overlays_unwarped(state: &AppState) -> Vec<OverlayBox> {
    let mut out = Vec::new();
    let Some(a) = state.analysis() else {
        return out;
    };
    let sel_lane = state.selected_lane_id();
    let sel_band = state.selected_band_id();
    for lane in &a.lanes {
        out.push(OverlayBox {
            x: lane.u_min as f32,
            y: 0.0,
            w: (lane.u_max - lane.u_min) as f32,
            h: 1.0,
            ladder: lane.is_ladder,
            is_lane: true,
            selected: sel_lane == Some(lane.id),
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
                selected: sel_band == Some(b.id),
            });
        }
    }
    out
}

fn compute_overlays(state: &AppState) -> Vec<OverlayBox> {
    let mut out = Vec::new();
    let (Some(a), Some(work)) = (state.analysis(), state.view_image()) else {
        return out;
    };
    let w = work.width().max(1) as f32;
    let warp = a.warp_or_identity(work.width() as u32, work.height() as u32);
    let sel_lane = state.selected_lane_id();
    let sel_band = state.selected_band_id();
    for lane in &a.lanes {
        let (x0, x1) = lane.px_x_bounds(&warp);
        out.push(OverlayBox {
            x: x0 as f32 / w,
            y: 0.0,
            w: (x1 - x0) as f32 / w,
            h: 1.0,
            ladder: lane.is_ladder,
            is_lane: true,
            selected: sel_lane == Some(lane.id),
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
                selected: sel_band == Some(b.id),
            });
        }
    }
    out
}

/// Window a working image into a displayable RGB buffer (no overlays). The
/// contrast window maps `[min + lo·span, min + hi·span]` → `[0,255]`
/// (span = max − min); `invert` flips the result; over-exposed pixels turn red.
/// This is the expensive full-image pass; `refresh_image` caches its result so
/// hover/drag only redraw the light overlays on top of it.
fn window_gray(
    work: &GrayF32,
    lo_frac: f32,
    hi_frac: f32,
    invert: bool,
    show_overexposed: bool,
) -> SharedPixelBuffer<slint::Rgb8Pixel> {
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
            let raw = (work.get(x, y) - black) / denom;
            px[y * w as usize + x] = if show_overexposed && raw >= 0.999 {
                slint::Rgb8Pixel { r: 255, g: 0, b: 0 }
            } else {
                let mut v = raw.clamp(0.0, 1.0);
                if invert {
                    v = 1.0 - v;
                }
                let g = (v * 255.0) as u8;
                slint::Rgb8Pixel { r: g, g, b: g }
            };
        }
    }
    buf
}

/// `window_gray` plus overlays composited on top.
fn render_gel(
    work: &GrayF32,
    lo_frac: f32,
    hi_frac: f32,
    invert: bool,
    show_overexposed: bool,
    overlays: &[OverlayBox],
) -> SharedPixelBuffer<slint::Rgb8Pixel> {
    let mut buf = window_gray(work, lo_frac, hi_frac, invert, show_overexposed);
    let (w, h) = (work.width() as u32, work.height() as u32);
    let px = buf.make_mut_slice();
    for ov in overlays {
        draw_overlay(px, w, h, ov, 1.0);
    }
    buf
}

/// Convert a working image to a displayable RGB [`Image`] with overlays.
fn to_slint_image(
    work: &GrayF32,
    lo_frac: f32,
    hi_frac: f32,
    invert: bool,
    overlays: &[OverlayBox],
) -> Image {
    Image::from_rgb8(render_gel(work, lo_frac, hi_frac, invert, false, overlays))
}

/// Draw a polyline (image-pixel coordinates) into an RGB buffer.
fn polyline(px: &mut [slint::Rgb8Pixel], w: u32, h: u32, pts: &[(f64, f64)], c: (u8, u8, u8)) {
    for pair in pts.windows(2) {
        draw_line(
            px,
            w,
            h,
            pair[0].0 as i32,
            pair[0].1 as i32,
            pair[1].0 as i32,
            pair[1].1 as i32,
            c,
        );
    }
}

#[inline]
fn put(px: &mut [slint::Rgb8Pixel], w: u32, h: u32, x: i32, y: i32, c: (u8, u8, u8)) {
    if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
        return;
    }
    px[(y as u32 * w + x as u32) as usize] = slint::Rgb8Pixel { r: c.0, g: c.1, b: c.2 };
}

/// Bresenham line into an RGB buffer.
fn draw_line(px: &mut [slint::Rgb8Pixel], w: u32, h: u32, x0: i32, y0: i32, x1: i32, y1: i32, c: (u8, u8, u8)) {
    let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
    let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
    let (mut x, mut y) = (x0, y0);
    let mut err = dx + dy;
    loop {
        put(px, w, h, x, y, c);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
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

fn draw_overlay(px: &mut [slint::Rgb8Pixel], w: u32, h: u32, ov: &OverlayBox, alpha_scale: f32) {
    let (wf, hf) = (w as f32, h as f32);
    let x0 = (ov.x * wf).round().clamp(0.0, wf - 1.0) as u32;
    let y0 = (ov.y * hf).round().clamp(0.0, hf - 1.0) as u32;
    let x1 = ((ov.x + ov.w) * wf).round().clamp(0.0, wf) as u32;
    let y1 = ((ov.y + ov.h) * hf).round().clamp(0.0, hf) as u32;
    // Selected annotations render in bright green with a thicker, more opaque
    // border so the current selection is obvious while dragging.
    let (cr, cg, cb) = if ov.selected {
        (60u8, 255u8, 90u8)
    } else if ov.ladder {
        (255u8, 204u8, 0u8)
    } else {
        (51u8, 187u8, 255u8)
    };
    let fill = if ov.selected {
        if ov.is_lane { 0.16 } else { 0.42 }
    } else if ov.is_lane {
        0.10
    } else {
        0.30
    };
    let bw = if ov.selected { 2 } else { 1 };
    let alpha_scale = alpha_scale.clamp(0.0, 1.0);
    if alpha_scale <= 0.0 {
        return;
    }
    for y in y0..y1 {
        for x in x0..x1 {
            let i = (y * w + x) as usize;
            let on_border = x < x0 + bw || x + bw >= x1 || y < y0 + bw || y + bw >= y1;
            let a = (if on_border { 0.95 } else { fill }) * alpha_scale;
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
