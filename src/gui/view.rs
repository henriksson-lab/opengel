//! Rendering application state into the Slint UI. Overlays (lane columns and
//! band bars) are composited directly into the displayed image so they rotate
//! and zoom together with it. The Trace tab's plot is built from Slint `Path`
//! elements (see `app.slint`) so it can carry real axes and labels; this module
//! only produces the path geometry and colors.

use opengel::core::GrayF32;
use opengel::instrument::acquisition::fmt_seconds;
use opengel::instrument::TrayType;
use slint::{Color, Image, ModelRc, SharedPixelBuffer, SharedString, VecModel};

use crate::geldoc::RunPhase;
use crate::state::{AppState, LaneTrace, TraceMode};
use crate::{
    AppWindow, AxisTick, ChannelItem, FaultItem, LaneItem, MetaRow, TracePath, TreeRow, WarpKnot,
};

/// Rebuild the Metadata tab's rows.
///
/// The section heading is emitted on the first row of each run rather than
/// repeated, so a channel's fields read as one block.
pub fn refresh_metadata(ui: &AppWindow, state: &AppState) {
    let mut last_section = String::new();
    let rows: Vec<MetaRow> = state
        .metadata_rows()
        .into_iter()
        .map(|(section, name, value)| {
            let first = section != last_section;
            last_section = section.clone();
            MetaRow {
                section: section.into(),
                name: name.into(),
                value: value.into(),
                first_in_section: first,
            }
        })
        .collect();
    ui.set_metadata_rows(ModelRc::new(VecModel::from(rows)));
}

/// Tab indices, fixed whatever is greyed out (see the tab bar in `app.slint`).
pub const TAB_GEL: i32 = 0;
pub const TAB_CAPTURE: i32 = 3;

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

/// A normalized-coordinate overlay to draw onto the image.
struct Overlay {
    shape: OverlayShape,
    ladder: bool,
    is_lane: bool,
    selected: bool,
}

enum OverlayShape {
    Rect { x: f32, y: f32, w: f32, h: f32 },
    Polygon(Vec<(f32, f32)>),
}

/// Full refresh: labels, ladder names, tree and composited image.
pub fn refresh(ui: &AppWindow, state: &AppState) {
    let files: Vec<SharedString> = state
        .document_labels()
        .into_iter()
        .map(SharedString::from)
        .collect();
    ui.set_file_names(ModelRc::new(VecModel::from(files)));
    ui.set_file_index(state.active_document_index());
    ui.set_has_open_file(state.has_open_file());
    // Gel, Trace and Metadata are greyed out with nothing open, so the tab that
    // is *showing* must not be one of them.
    if !state.has_open_file() && ui.get_active_tab() != TAB_CAPTURE {
        ui.set_active_tab(TAB_CAPTURE);
    }
    ui.set_active_file_dirty(state.active_document_dirty());
    let gel_types: Vec<SharedString> = AppState::gel_type_names()
        .into_iter()
        .map(SharedString::from)
        .collect();
    ui.set_gel_type_names(ModelRc::new(VecModel::from(gel_types)));
    ui.set_gel_type_index(state.gel_type.index() as i32);
    ui.set_rotation(state.rotation_deg as f32);
    ui.set_disp_lo(state.disp_lo);
    ui.set_disp_hi(state.disp_hi);
    ui.set_invert(state.invert);
    ui.set_show_unwarped(state.show_unwarped);
    ui.set_show_warp(state.show_warp);
    ui.set_normalize_inner_knots(state.normalize_inner_knots);
    ui.set_show_overexposed(state.show_overexposed);
    ui.set_annotation_alpha(state.annotation_alpha);

    // Ladder template names (for the "Use as ladder" dialog dropdown).
    let vendors: Vec<SharedString> = state
        .ladder_vendor_names(state.gel_type)
        .into_iter()
        .map(SharedString::from)
        .collect();
    ui.set_ladder_vendor_names(ModelRc::new(VecModel::from(vendors)));
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

    // Channel selector and the Metadata tab.
    let channels: Vec<SharedString> = state
        .channel_labels()
        .into_iter()
        .map(SharedString::from)
        .collect();
    ui.set_channel_names(ModelRc::new(VecModel::from(channels)));
    ui.set_channel_index(state.view_channel_index() as i32);
    ui.set_is_multichannel(state.is_multichannel());
    refresh_metadata(ui, state);

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
    // Re-detection overwrites annotations; the Detect button warns first when any exist.
    ui.set_has_annotations(
        state
            .analysis()
            .is_some_and(|a| !a.lanes.is_empty() || !a.bands.is_empty()),
    );
    let (sel_lane, sel_band, sel_lad) = state.selection_info();
    ui.set_selected_lane_id(sel_lane);
    ui.set_selected_band_id(sel_band);
    ui.set_selected_lane_is_ladder(sel_lad);
    ui.set_ratio_label(state.ratio_label().into());
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
    let n = traces.iter().map(|t| t.values.len()).max().unwrap_or(0);
    // Default view: crop the migration axis to the visible peaks so the traces
    // fill the plot width. The user can then zoom/pan into a sub-window.
    let (bk0, bk1) = signal_extent(&traces, n);
    let bspan = (bk1 - bk0).max(1e-6);
    let vspan = (bspan / state.trace_zoom.max(1.0)).clamp(1e-6, bspan);
    let center = bk0 + state.trace_pan.clamp(0.0, 1.0) * bspan;
    let hi = (bk1 - vspan).max(bk0); // guard: keep clamp's min <= max
    let k0 = (center - vspan / 2.0).clamp(bk0, hi);
    let k1 = (k0 + vspan).min(bk1);
    state.trace_view.set((k0, k1, n)); // for the hover bp readout
    let (paths, ymax) = build_trace_paths(&traces, k0, k1);
    ui.set_trace_plots(ModelRc::new(VecModel::from(paths)));
    ui.set_trace_zoom(state.trace_zoom as f32);

    // Per-sample molar amounts are tiny for DNA (fmol–pmol range), so pick a
    // readable sub-unit instead of showing all-zero nmol ticks. Non-molarity
    // modes keep their native unit (scale 1).
    let (yscale, yunit): (f64, &str) = match state.trace_mode {
        TraceMode::Molarity => {
            let m = ymax as f64;
            if m <= 0.0 || m >= 1.0 {
                (1.0, "nmol")
            } else if m >= 1e-3 {
                (1e3, "pmol")
            } else if m >= 1e-6 {
                (1e6, "fmol")
            } else {
                (1e9, "amol")
            }
        }
        _ => (1.0, ""),
    };

    // Y ticks (index 0 = top = ymax); X ticks span the cropped migration range.
    let yticks: Vec<SharedString> = (0..5)
        .map(|i| fmt_tick(ymax as f64 * yscale * (4 - i) as f64 / 4.0).into())
        .collect();
    let xticks: Vec<SharedString> = (0..5)
        .map(|i| fmt_tick(k0 + (k1 - k0) * i as f64 / 4.0).into())
        .collect();
    ui.set_trace_yticks(ModelRc::new(VecModel::from(yticks)));
    ui.set_trace_xticks(ModelRc::new(VecModel::from(xticks)));
    ui.set_trace_xlabel("Migration (px)".into());

    // Second x-axis: ladder-based size scale. Rather than sizing at uniform
    // migration positions (giving odd numbers), place ticks at *round* 1-2-5 bp
    // values and position each at its true migration via the semi-log fit's
    // inverse — a proper, readable bp axis normalized to the ladder.
    match state.sizing_fit().filter(|_| n > 0 && k1 > k0) {
        Some(fit) => {
            // Size (bp) sampled at the SAME 5 fixed screen positions as the
            // migration axis, so the two axes stay locked together while panning
            // and zooming (a vertical line reads the same x on both). `k/n = v`
            // is the fit's migration coordinate (one trace sample per rectified
            // pixel row), so `size_at(k/n)` is the size at that column.
            let ticks: Vec<AxisTick> = (0..5)
                .map(|i| {
                    let f = i as f64 / 4.0;
                    let k = k0 + (k1 - k0) * f;
                    AxisTick {
                        pos: f as f32,
                        label: fmt_tick(fit.size_at(k / n as f64)).into(),
                    }
                })
                .collect();
            ui.set_trace_xticks_top(ModelRc::new(VecModel::from(ticks)));
            ui.set_trace_xtoplabel(format!("Size ({})", state.gel_type.size_unit()).into());
        }
        None => {
            ui.set_trace_xticks_top(ModelRc::new(VecModel::from(Vec::<AxisTick>::new())));
            ui.set_trace_xtoplabel(SharedString::new());
        }
    }
    ui.set_trace_ylabel(
        match state.trace_mode {
            TraceMode::Intensity => "Intensity (a.u.)".to_string(),
            TraceMode::Ng => "Mass (ng)".to_string(),
            TraceMode::Molarity => format!("Molarity ({yunit})"),
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
fn build_trace_paths(traces: &[LaneTrace], k0: f64, k1: f64) -> (Vec<TracePath>, f32) {
    let vmax = traces
        .iter()
        .flat_map(|t| t.values.iter().copied())
        .fold(0.0f64, f64::max)
        .max(1e-9);
    let span = (k1 - k0).max(1e-9);
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
        // Map k over the cropped [k0, k1] window to the 0..1000 viewbox; points
        // outside are clipped by the plot's own clip region.
        let mut cmds = String::with_capacity(n * 12);
        for (k, &v) in t.values.iter().enumerate() {
            let x = 1000.0 * (k as f64 - k0) / span;
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
    (out, vmax as f32)
}

/// Migration-index window `[k0, k1]` bracketing the *visible peaks*: the first
/// and last row whose signal (per-row max across traces) exceeds 10% of the peak.
/// This crops empty wells, run-off, and faint fuzz at the extremes, and — unlike
/// a cumulative-mass window — stays tight even when low-level signal is smeared
/// across the lane. Falls back to the full `0..n` range when flat.
fn signal_extent(traces: &[LaneTrace], n: usize) -> (f64, f64) {
    if n == 0 || traces.is_empty() {
        return (0.0, n as f64);
    }
    let mut prof = vec![0.0f64; n];
    for t in traces {
        for (k, &v) in t.values.iter().enumerate().take(n) {
            if v > prof[k] {
                prof[k] = v.max(0.0);
            }
        }
    }
    let peak = prof.iter().cloned().fold(0.0f64, f64::max);
    if peak <= 1e-9 {
        return (0.0, n as f64);
    }
    let thresh = 0.10 * peak;
    let lo = prof.iter().position(|&p| p > thresh);
    let hi = prof.iter().rposition(|&p| p > thresh);
    match (lo, hi) {
        (Some(lo), Some(hi)) if hi > lo => {
            let margin = (0.08 * (hi - lo) as f64).max(5.0);
            (
                (lo as f64 - margin).max(0.0),
                (hi as f64 + margin).min((n - 1) as f64),
            )
        }
        _ => (0.0, n as f64),
    }
}

// The windowed grayscale base and the histogram are invariant to mouse hover
// (they depend only on the frame + contrast window, not the pointer). Caching
// them means a hover/drag only clones the base and redraws the light overlays +
// warp line, instead of re-windowing the whole image and recomputing the
// histogram every mouse-move event. Single UI thread → thread_local is fine.
type BaseKey = (u64, usize, u32, u32, bool, bool, u32, u32); // doc_gen, frame, lo, hi, invert, overexp, w, h
type HistKey = (u64, usize, u32, u32); // doc_gen, frame, lo, hi
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
    // The contrast histogram is independent of the warped/rectified view, so
    // refresh it up front (it was being skipped in the unwarped branch).
    refresh_histogram(ui, state);
    // Dewarped view: a fixed, straightened render with overlays in rectified
    // (u,v) space (lanes vertical, bands horizontal). No zoom/pan/rotation/warp
    // grid — just the rectified gel and its annotation boxes.
    if state.show_unwarped {
        if let Some(rect) = state.unwarped_view() {
            let (w, h) = (rect.width() as u32, rect.height() as u32);
            let mut buf = window_gray(
                &rect,
                state.disp_lo,
                state.disp_hi,
                state.invert,
                state.show_overexposed,
            );
            {
                let px = buf.make_mut_slice();
                for ov in &compute_overlays_unwarped(state) {
                    draw_overlay(px, w, h, ov, state.annotation_alpha);
                }
                // In the rectified view iso-migration is horizontal, so the hover
                // alignment line is a straight horizontal line at the cursor's y.
                if state.hover_y >= 0.0 {
                    let line: Vec<(f64, f64)> = (0..w)
                        .map(|x| (x as f64, state.hover_y as f64 * h as f64))
                        .collect();
                    polyline(px, w, h, &line, (0, 210, 255));
                }
            }
            ui.set_gel_image(Image::from_rgb8(buf));
        }
        ui.set_frame_index(state.view_frame_index() as i32);
        set_warp_knots(ui, &[]); // no warp grid in the rectified view
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
            let b = window_gray(
                &work,
                state.disp_lo,
                state.disp_hi,
                state.invert,
                state.show_overexposed,
            );
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
        draw_migration_arrow(&mut buf, state);
        ui.set_gel_image(Image::from_rgb8(buf));
    }
    set_warp_knots(ui, &state.warp_knot_items());
    ui.set_frame_index(state.view_frame_index() as i32);
}

/// Refresh the contrast histogram thumbnail. Cached by frame + window, so it only
/// recomputes when those change (never on hover).
fn refresh_histogram(ui: &AppWindow, state: &AppState) {
    let hist_key: HistKey = (
        state.doc_gen,
        state.view_frame_index(),
        state.disp_lo.to_bits(),
        state.disp_hi.to_bits(),
    );
    let need_hist = HIST_CACHE.with(|c| c.borrow().is_none_or(|k| k != hist_key));
    if need_hist {
        let hist = state.histogram(256);
        ui.set_histogram_image(render_histogram(
            &hist,
            state.disp_lo,
            state.disp_hi,
            1024,
            120,
        ));
        HIST_CACHE.with(|c| *c.borrow_mut() = Some(hist_key));
    }
}

/// Composite the fitted warp onto the gel buffer: the iso-parameter grid (when
/// "Show warp model" is on) and, always, the migration-alignment line through
/// the mouse pointer (the iso-`v` curve at the hovered migration level).
/// Push the warp control-point handles to the Slint overlay model (drawn as
/// separate elements, so knots at/beyond the image edge aren't clipped).
fn set_warp_knots(ui: &AppWindow, knots: &[(f32, f32, bool)]) {
    let items: Vec<WarpKnot> = knots
        .iter()
        .map(|&(nx, ny, active)| WarpKnot { nx, ny, active })
        .collect();
    ui.set_warp_knots(ModelRc::new(VecModel::from(items)));
}

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
        // Control-point handles are NOT composited here — they are drawn as
        // separate Slint overlay elements (see `set_warp_knots`) so a knot at or
        // beyond the image edge stays visible instead of being clipped to the
        // image buffer.
    }
    // Alignment line at the hovered migration level.
    if state.hover_x >= 0.0 && state.hover_y >= 0.0 {
        let (hx, hy) = (
            state.hover_x as f64 * w as f64,
            state.hover_y as f64 * h as f64,
        );
        let (_, v0) = warp.invert(hx, hy);
        polyline(px, w, h, &warp.iso_v(v0, 96), (0, 210, 255));
    }
}

fn draw_migration_arrow(buf: &mut SharedPixelBuffer<slint::Rgb8Pixel>, state: &AppState) {
    let Some(warp) = state.fit_warp() else {
        return;
    };
    let (w, h) = (buf.width(), buf.height());
    if w == 0 || h == 0 {
        return;
    }
    let (x0, y0) = warp.eval(0.0, 0.0);
    let (x1, y1) = warp.eval(0.0, 1.0);
    let (dx, dy) = (x1 - x0, y1 - y0);
    let len = dx.hypot(dy);
    if len < 1.0 {
        return;
    }

    let (nx_a, ny_a) = (-dy / len, dx / len);
    let (nx, ny) = if x0 + nx_a * 14.0 < x0 - nx_a * 14.0 {
        (nx_a, ny_a)
    } else {
        (-nx_a, -ny_a)
    };
    let offset = 14.0;
    let margin = 5.0;
    let p0 = (
        (x0 + nx * offset).clamp(margin, w as f64 - margin),
        (y0 + ny * offset).clamp(margin, h as f64 - margin),
    );
    let p1 = (
        (x1 + nx * offset).clamp(margin, w as f64 - margin),
        (y1 + ny * offset).clamp(margin, h as f64 - margin),
    );

    let px = buf.make_mut_slice();
    let color = (0, 170, 120);
    draw_line(
        px,
        w,
        h,
        (p0.0 as i32, p0.1 as i32),
        (p1.0 as i32, p1.1 as i32),
        color,
    );

    let ux = dx / len;
    let uy = dy / len;
    let head_len = 12.0;
    let head_w = 5.0;
    for side in [-1.0, 1.0] {
        let hx = p1.0 - ux * head_len + nx * head_w * side;
        let hy = p1.1 - uy * head_len + ny * head_w * side;
        draw_line(
            px,
            w,
            h,
            (p1.0 as i32, p1.1 as i32),
            (hx as i32, hy as i32),
            color,
        );
    }
}

/// Refresh the Gel Doc EZ tab: instrument state, faults, protocols and steps.
///
/// Called from the same event pump that drains the camera, so this runs several
/// times a second while the instrument is polled. It must therefore be cheap and
/// must not clobber anything the user is typing — see `name_field_for`.
pub fn refresh_geldoc(ui: &AppWindow, state: &AppState) {
    let gd = &state.geldoc;

    // --- connection and identity ---
    let names: Vec<SharedString> = gd.instruments.iter().map(SharedString::from).collect();
    ui.set_gd_instrument_names(ModelRc::new(VecModel::from(names)));
    ui.set_gd_instrument_index(gd.selected_instrument as i32);
    ui.set_gd_connected(gd.connected);
    ui.set_gd_simulated(gd.simulated);
    ui.set_gd_model(
        if gd.connected {
            gd.info.model.clone()
        } else {
            "—".into()
        }
        .into(),
    );
    ui.set_gd_versions(
        if gd.connected {
            format!(
                "fw {}  hw {}",
                gd.info.firmware_string(),
                gd.info.hardware_string()
            )
        } else {
            String::new()
        }
        .into(),
    );
    ui.set_gd_serial(
        if gd.info.serial.is_empty() {
            String::new()
        } else {
            format!("SN {}", gd.info.serial)
        }
        .into(),
    );

    // --- live sense state ---
    let tray = gd.inserted_tray();
    ui.set_gd_tray_label(
        tray.map(|t| t.label().to_string())
            .unwrap_or_else(|| {
                if gd.connected {
                    "none".into()
                } else {
                    "—".into()
                }
            })
            .into(),
    );
    ui.set_gd_tray_present(tray.is_some());
    ui.set_gd_door_closed(gd.door_closed());
    ui.set_gd_busy(gd.sense.is_some_and(|s| s.busy));
    // Lit lamps are a state the user must not open the door on, and the
    // instrument does not report them: it is the run phase that knows.
    ui.set_gd_lamps_on(gd.lamps_lit());
    // The sense bits nobody has decoded. Shown, not hidden: the front Run
    // button is believed to be in there, and seeing the mask move when the
    // button is pressed is how it gets identified on real hardware.
    ui.set_gd_undecoded_label(
        if gd.undecoded != 0 {
            format!("Undecoded sense bits high: 0x{:04x}", gd.undecoded)
        } else {
            String::new()
        }
        .into(),
    );

    // --- faults, each with its remedy ---
    let faults: Vec<FaultItem> = gd
        .faults
        .messages()
        .into_iter()
        .map(|m| FaultItem {
            headline: m.headline.into(),
            remedy: m.remedy.into(),
        })
        .collect();
    ui.set_gd_faults(ModelRc::new(VecModel::from(faults)));

    // --- the channels to acquire ---
    //
    // An instrument that cannot change its light has one channel. With a plain
    // camera that is the whole list: a checklist of light sources you cannot
    // select between would be a lie about what the hardware can do.
    let channels: Vec<ChannelItem> = if gd.connected {
        gd.plan
            .channels
            .iter()
            .map(|c| ChannelItem {
                name: c.label().into(),
                selected: c.selected,
                summary: c.summary().into(),
                inserted: tray == Some(c.tray),
            })
            .collect()
    } else {
        vec![ChannelItem {
            name: "Camera".into(),
            selected: true,
            summary: gd.channel().summary().into(),
            inserted: false,
        }]
    };
    ui.set_gd_channel_items(ModelRc::new(VecModel::from(channels)));
    let names: Vec<SharedString> = gd
        .plan
        .channels
        .iter()
        .map(|c| SharedString::from(c.label()))
        .collect();
    ui.set_gd_channel_names(ModelRc::new(VecModel::from(names)));
    ui.set_gd_channel_index(if gd.connected {
        gd.current_channel_index() as i32
    } else {
        0
    });
    ui.set_gd_channel_label(gd.channel().label().into());
    ui.set_gd_gel_type_index(state.gel_type.index() as i32);
    // The bench control mirrors the tray that is actually in, so it cannot sit
    // there claiming "none" over an inserted tray.
    ui.set_gd_sim_tray_index(match tray {
        None => 0,
        Some(TrayType::Uv) => 1,
        Some(TrayType::White) => 2,
        Some(TrayType::Blue) => 3,
        Some(TrayType::StainFree) => 4,
    });

    // --- settings of the channel being edited ---
    let channel = gd.channel();
    ui.set_gd_capture_mode_index(channel.mode.index() as i32);
    ui.set_gd_hdr_steps_idx(state.hdr_steps_idx() as i32);
    ui.set_gd_channel_ready(gd.current_channel_ready());
    // The commonest setup mistake is the right gel under the wrong light, which
    // images as a blank gel with nothing obviously wrong. Say which tray this
    // channel needs whenever it is not the one that is in.
    ui.set_gd_channel_hint(
        match (gd.connected, gd.inserted_tray(), gd.current_channel_ready()) {
            (false, _, _) => String::new(),
            (true, _, true) => format!("Lit: the {} tray is inserted.", channel.label()),
            (true, Some(inserted), false) => format!(
                "Not lit — the {} tray is inserted, this channel needs the {} tray.",
                inserted.label(),
                channel.label()
            ),
            (true, None, false) => format!("Not lit — insert the {} tray.", channel.label()),
        }
        .into(),
    );
    ui.set_gd_activation_applicable(channel.tray == TrayType::StainFree);
    ui.set_gd_activation_s(format!("{:.0}", channel.activation_s).into());
    ui.set_gd_highlight_saturated(gd.plan.highlight_saturated);
    ui.set_gd_last_exposure_label(
        gd.last_exposure_s
            .map(|t| format!("Last exposure: {}.", fmt_seconds(t)))
            .unwrap_or_default()
            .into(),
    );

    // --- run state ---
    let blocker = gd.run_blocker();
    ui.set_gd_can_run(blocker.is_none());
    ui.set_gd_run_blocker(blocker.unwrap_or_default().into());
    ui.set_gd_running(gd.phase.is_running());
    ui.set_gd_phase_label(gd.phase.label().into());
    ui.set_gd_run_progress(gd.run_progress_label().into());
    // A run's progress goes to the window's status bar rather than into a panel
    // of its own: it is one line, it changes every few seconds, and the status
    // bar is where the app already says what it is doing.
    if gd.phase.is_running() {
        let progress = gd.run_progress_label();
        ui.set_status(
            if progress.is_empty() {
                gd.phase.label()
            } else {
                format!("{}  —  {}", gd.phase.label(), progress)
            }
            .into(),
        );
    }
    ui.set_gd_activation_progress(match &gd.phase {
        RunPhase::Activating { elapsed_s, total_s } if *total_s > 0.0 => {
            (elapsed_s / total_s).clamp(0.0, 1.0) as f32
        }
        _ => 0.0,
    });
    ui.set_gd_message(gd.message.clone().into());
}

/// Refresh the Live tab: camera name, running state, status, preview image.
pub fn refresh_live(ui: &AppWindow, state: &AppState) {
    ui.set_live_running(state.live_running);
    if let Some(p) = state.preview_image() {
        // Always flag clipped-high (saturated) pixels in red — a live exposure
        // aid, so you can lower exposure before capturing blown-out bands.
        ui.set_live_preview_image(to_slint_image(p, 0.0, 1.0, state.invert, true, &[]));
    }
    // Live histogram of the preview frame (exposure aid). Rendered wide so it
    // isn't upscaled/blurry when stretched across the column.
    let hist = state.preview_histogram(256);
    ui.set_live_histogram_image(render_histogram(&hist, 0.0, 1.0, 1024, 120));

    // Exposure controls. These belong to the channel being framed, so the Live
    // tab and the Gel Doc EZ tab always show the same numbers for it.
    let channel = state.live_channel();
    ui.set_live_exposure_slider(state.live_exposure_slider());
    ui.set_live_exposure_label(fmt_seconds(channel.exposure_s).into());
    ui.set_hdr_min_label(fmt_seconds(channel.hdr_min_s).into());
    ui.set_hdr_max_label(fmt_seconds(channel.hdr_max_s).into());
    ui.set_hdr_range_label(format!("{:.1} EV", state.hdr_range_ev()).into());
    ui.set_gd_hdr_steps_idx(state.hdr_steps_idx() as i32);
    ui.set_gd_capture_mode_index(channel.mode.index() as i32);

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

/// Overlays for the dewarped view: in rectified space `(u, v)` map directly to
/// the overlay rectangle (lanes vertical, bands horizontal).
fn compute_overlays_unwarped(state: &AppState) -> Vec<Overlay> {
    let mut out = Vec::new();
    let Some(a) = state.analysis() else {
        return out;
    };
    let sel_lane = state.selected_lane_id();
    let sel_band = state.selected_band_id();
    for lane in &a.lanes {
        out.push(Overlay {
            shape: OverlayShape::Rect {
                x: lane.u_min as f32,
                y: 0.0,
                w: (lane.u_max - lane.u_min) as f32,
                h: 1.0,
            },
            ladder: lane.is_ladder,
            is_lane: true,
            selected: sel_lane == Some(lane.id),
        });
    }
    for b in &a.bands {
        if let Some(lane) = a.lanes.iter().find(|l| l.id == b.lane_id) {
            out.push(Overlay {
                shape: OverlayShape::Rect {
                    x: lane.u_min as f32,
                    y: (b.v_center - b.v_half_width) as f32,
                    w: (lane.u_max - lane.u_min) as f32,
                    h: (2.0 * b.v_half_width) as f32,
                },
                ladder: lane.is_ladder,
                is_lane: false,
                selected: sel_band == Some(b.id),
            });
        }
    }
    out
}

fn compute_overlays(state: &AppState) -> Vec<Overlay> {
    let mut out = Vec::new();
    let (Some(a), Some(work)) = (state.analysis(), state.view_image()) else {
        return out;
    };
    let (w, h) = (work.width().max(1) as f64, work.height().max(1) as f64);
    let warp = a.warp_or_identity(work.width() as u32, work.height() as u32);
    let sel_lane = state.selected_lane_id();
    let sel_band = state.selected_band_id();
    for lane in &a.lanes {
        out.push(Overlay {
            shape: OverlayShape::Polygon(warped_lane_polygon(&warp, lane, w, h)),
            ladder: lane.is_ladder,
            is_lane: true,
            selected: sel_lane == Some(lane.id),
        });
    }
    for b in &a.bands {
        if let Some(lane) = a.lanes.iter().find(|l| l.id == b.lane_id) {
            out.push(Overlay {
                shape: OverlayShape::Polygon(warped_band_polygon(&warp, lane, b, w, h)),
                ladder: lane.is_ladder,
                is_lane: false,
                selected: sel_band == Some(b.id),
            });
        }
    }
    out
}

fn warped_lane_polygon(
    warp: &opengel::core::warp::GelWarp,
    lane: &opengel::core::model::Lane,
    w: f64,
    h: f64,
) -> Vec<(f32, f32)> {
    const N: usize = 16;
    let mut pts = Vec::with_capacity(N * 2);
    for i in 0..N {
        let v = i as f64 / (N - 1) as f64;
        pts.push(norm_point(warp.eval(lane.u_min, v), w, h));
    }
    for i in (0..N).rev() {
        let v = i as f64 / (N - 1) as f64;
        pts.push(norm_point(warp.eval(lane.u_max, v), w, h));
    }
    pts
}

/// Band annotation box: a rectangle spanning the lane width and the band's
/// thickness, **rotated to the band's measured tilt** (`band.angle`) and centered
/// at the band's image position. The angle comes from the band's own intensity
/// moments (the reliable orientation cue), so the box hugs the real band even
/// where the fitted warp is imperfect. Center and lane width still come from the
/// warp so the box tracks the rectified lane geometry.
fn warped_band_polygon(
    warp: &opengel::core::warp::GelWarp,
    lane: &opengel::core::model::Lane,
    band: &opengel::core::model::Band,
    w: f64,
    h: f64,
) -> Vec<(f32, f32)> {
    let u_c = (lane.u_min + lane.u_max) * 0.5;
    let v = band.v_center.clamp(0.0, 1.0);
    // Center + lane-width vector, in image pixels, from the warp.
    let (cxp, cyp) = warp.eval(u_c, v);
    let (lxp, lyp) = warp.eval(lane.u_min, v);
    let (rxp, ryp) = warp.eval(lane.u_max, v);
    let half_w = 0.5 * ((rxp - lxp).hypot(ryp - lyp));
    // Band thickness in pixels (v maps ~linearly to y at scale h).
    let half_h = (band.v_half_width * h).max(1.0);
    // Long axis along the measured tilt; short axis perpendicular.
    let (sa, ca) = band.angle.sin_cos();
    let (ax, ay) = (ca * half_w, sa * half_w); // along band
    let (px, py) = (-sa * half_h, ca * half_h); // across band
    let corners = [
        (cxp - ax - px, cyp - ay - py),
        (cxp + ax - px, cyp + ay - py),
        (cxp + ax + px, cyp + ay + py),
        (cxp - ax + px, cyp - ay + py),
    ];
    corners.iter().map(|&p| norm_point(p, w, h)).collect()
}

fn norm_point((x, y): (f64, f64), w: f64, h: f64) -> (f32, f32) {
    ((x / w) as f32, (y / h) as f32)
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
    overlays: &[Overlay],
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
    show_overexposed: bool,
    overlays: &[Overlay],
) -> Image {
    Image::from_rgb8(render_gel(
        work,
        lo_frac,
        hi_frac,
        invert,
        show_overexposed,
        overlays,
    ))
}

/// Draw a polyline (image-pixel coordinates) into an RGB buffer.
fn polyline(px: &mut [slint::Rgb8Pixel], w: u32, h: u32, pts: &[(f64, f64)], c: (u8, u8, u8)) {
    for pair in pts.windows(2) {
        draw_line(
            px,
            w,
            h,
            (pair[0].0 as i32, pair[0].1 as i32),
            (pair[1].0 as i32, pair[1].1 as i32),
            c,
        );
    }
}

#[inline]
fn put(px: &mut [slint::Rgb8Pixel], w: u32, h: u32, x: i32, y: i32, c: (u8, u8, u8)) {
    if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
        return;
    }
    px[(y as u32 * w + x as u32) as usize] = slint::Rgb8Pixel {
        r: c.0,
        g: c.1,
        b: c.2,
    };
}

/// Bresenham line into an RGB buffer.
fn draw_line(
    px: &mut [slint::Rgb8Pixel],
    w: u32,
    h: u32,
    p0: (i32, i32),
    p1: (i32, i32),
    c: (u8, u8, u8),
) {
    let (x0, y0) = p0;
    let (x1, y1) = p1;
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
    let bg = slint::Rgb8Pixel {
        r: 250,
        g: 250,
        b: 250,
    };
    let win = slint::Rgb8Pixel {
        r: 222,
        g: 236,
        b: 252,
    };
    for p in px.iter_mut() {
        *p = bg;
    }
    let n = hist.len().max(1);
    // Vertical scale reference: a high percentile of the non-empty bins rather
    // than the absolute max. A gel is mostly dark background, so one or two bins
    // dwarf the rest; scaling to the max would squash the informative band-signal
    // tail to nothing. Using the ~98th percentile lets the distribution fill the
    // height, with the background spike simply clipping to the top. Log-scaled so
    // faint bins stay visible.
    let refc = {
        let mut counts: Vec<u32> = hist.iter().copied().filter(|&c| c > 0).collect();
        counts.sort_unstable();
        if counts.is_empty() {
            1.0
        } else {
            let idx = ((counts.len() as f64 * 0.98).ceil() as usize).min(counts.len() - 1);
            counts[idx].max(1) as f64
        }
    };
    let lref = (1.0 + refc).ln();
    let (lo_x, hi_x) = ((lo * w as f32) as u32, (hi * w as f32) as u32);
    for x in 0..w {
        // Window highlight band behind the bars.
        if x >= lo_x && x <= hi_x {
            for y in 0..h {
                px[(y * w + x) as usize] = win;
            }
        }
        let bin = (x as usize * n / w as usize).min(n - 1);
        let frac = ((1.0 + hist[bin] as f64).ln() / lref).min(1.0);
        let bar = ((frac * (h as f64 - 2.0)).round() as u32).min(h - 1);
        for y in (h - bar)..h {
            px[(y * w + x) as usize] = slint::Rgb8Pixel {
                r: 90,
                g: 90,
                b: 96,
            };
        }
    }
    Image::from_rgb8(buf)
}

fn draw_overlay(px: &mut [slint::Rgb8Pixel], w: u32, h: u32, ov: &Overlay, alpha_scale: f32) {
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
        if ov.is_lane {
            0.16
        } else {
            0.42
        }
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
    let style = OverlayStyle {
        rgb: (cr, cg, cb),
        fill,
        border_alpha: 0.95,
        border_width: bw,
        alpha_scale,
    };
    match &ov.shape {
        OverlayShape::Rect { x, y, w: ow, h: oh } => {
            draw_overlay_rect(px, w, h, (*x, *y, *ow, *oh), style);
        }
        OverlayShape::Polygon(points) => draw_overlay_polygon(px, w, h, points, style),
    }
}

#[derive(Clone, Copy)]
struct OverlayStyle {
    rgb: (u8, u8, u8),
    fill: f32,
    border_alpha: f32,
    border_width: u32,
    alpha_scale: f32,
}

fn draw_overlay_rect(
    px: &mut [slint::Rgb8Pixel],
    w: u32,
    h: u32,
    rect: (f32, f32, f32, f32),
    style: OverlayStyle,
) {
    let (wf, hf) = (w as f32, h as f32);
    let (x, y, ow, oh) = rect;
    let x0 = (x * wf).round().clamp(0.0, wf - 1.0) as u32;
    let y0 = (y * hf).round().clamp(0.0, hf - 1.0) as u32;
    let x1 = ((x + ow) * wf).round().clamp(0.0, wf) as u32;
    let y1 = ((y + oh) * hf).round().clamp(0.0, hf) as u32;
    for y in y0..y1 {
        for x in x0..x1 {
            let i = (y * w + x) as usize;
            let on_border = x < x0 + style.border_width
                || x + style.border_width >= x1
                || y < y0 + style.border_width
                || y + style.border_width >= y1;
            let a = (if on_border {
                style.border_alpha
            } else {
                style.fill
            }) * style.alpha_scale;
            let p = &mut px[i];
            p.r = blend(p.r, style.rgb.0, a);
            p.g = blend(p.g, style.rgb.1, a);
            p.b = blend(p.b, style.rgb.2, a);
        }
    }
}

fn draw_overlay_polygon(
    px: &mut [slint::Rgb8Pixel],
    w: u32,
    h: u32,
    points: &[(f32, f32)],
    style: OverlayStyle,
) {
    if points.len() < 3 {
        return;
    }
    let wf = w as f32;
    let hf = h as f32;
    let pix: Vec<(f32, f32)> = points.iter().map(|&(x, y)| (x * wf, y * hf)).collect();
    let (min_x, max_x, min_y, max_y) = polygon_bounds(&pix, w, h);
    for y in min_y..max_y {
        for x in min_x..max_x {
            let pxy = (x as f32 + 0.5, y as f32 + 0.5);
            if point_in_polygon(pxy, &pix) {
                let i = (y * w + x) as usize;
                let p = &mut px[i];
                let a = style.fill * style.alpha_scale;
                p.r = blend(p.r, style.rgb.0, a);
                p.g = blend(p.g, style.rgb.1, a);
                p.b = blend(p.b, style.rgb.2, a);
            }
        }
    }

    for pair in pix.windows(2) {
        draw_thick_line(px, w, h, pair[0], pair[1], style);
    }
    draw_thick_line(px, w, h, *pix.last().unwrap(), pix[0], style);
}

fn polygon_bounds(points: &[(f32, f32)], w: u32, h: u32) -> (u32, u32, u32, u32) {
    let min_x = points
        .iter()
        .map(|p| p.0)
        .fold(f32::INFINITY, f32::min)
        .floor()
        .clamp(0.0, w.saturating_sub(1) as f32) as u32;
    let max_x = points
        .iter()
        .map(|p| p.0)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .clamp(0.0, w as f32) as u32;
    let min_y = points
        .iter()
        .map(|p| p.1)
        .fold(f32::INFINITY, f32::min)
        .floor()
        .clamp(0.0, h.saturating_sub(1) as f32) as u32;
    let max_y = points
        .iter()
        .map(|p| p.1)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .clamp(0.0, h as f32) as u32;
    (min_x, max_x, min_y, max_y)
}

fn point_in_polygon((x, y): (f32, f32), points: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let mut j = points.len() - 1;
    for i in 0..points.len() {
        let (xi, yi) = points[i];
        let (xj, yj) = points[j];
        let crosses = (yi > y) != (yj > y);
        if crosses {
            let x_at_y = (xj - xi) * (y - yi) / (yj - yi) + xi;
            if x < x_at_y {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

fn draw_thick_line(
    px: &mut [slint::Rgb8Pixel],
    w: u32,
    h: u32,
    p0: (f32, f32),
    p1: (f32, f32),
    style: OverlayStyle,
) {
    let radius = style.border_width.saturating_sub(1) as i32;
    let alpha = style.border_alpha * style.alpha_scale;
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            draw_alpha_line(
                px,
                w,
                h,
                (p0.0.round() as i32 + dx, p0.1.round() as i32 + dy),
                (p1.0.round() as i32 + dx, p1.1.round() as i32 + dy),
                style.rgb,
                alpha,
            );
        }
    }
}

fn draw_alpha_line(
    px: &mut [slint::Rgb8Pixel],
    w: u32,
    h: u32,
    p0: (i32, i32),
    p1: (i32, i32),
    c: (u8, u8, u8),
    alpha: f32,
) {
    let (x0, y0) = p0;
    let (x1, y1) = p1;
    let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
    let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
    let (mut x, mut y) = (x0, y0);
    let mut err = dx + dy;
    loop {
        put_alpha(px, w, h, x, y, c, alpha);
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

#[inline]
fn put_alpha(
    px: &mut [slint::Rgb8Pixel],
    w: u32,
    h: u32,
    x: i32,
    y: i32,
    c: (u8, u8, u8),
    alpha: f32,
) {
    if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
        return;
    }
    let p = &mut px[(y as u32 * w + x as u32) as usize];
    p.r = blend(p.r, c.0, alpha);
    p.g = blend(p.g, c.1, alpha);
    p.b = blend(p.b, c.2, alpha);
}

#[inline]
fn blend(bg: u8, fg: u8, a: f32) -> u8 {
    (bg as f32 * (1.0 - a) + fg as f32 * a)
        .round()
        .clamp(0.0, 255.0) as u8
}
