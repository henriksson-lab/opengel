//! Rendering application state into the Slint UI. Overlays (lane columns and
//! band bars) are composited directly into the displayed image so they rotate
//! and zoom together with it.

use opengel::core::GrayF32;
use slint::{Image, ModelRc, SharedPixelBuffer, SharedString, StandardListViewItem, VecModel};

use crate::state::{AppState, LaneTrace, TraceMode};
use crate::{AppWindow, LaneItem};

/// Distinct colors for sample-lane traces (ladders use a fixed gold).
const PALETTE: [(u8, u8, u8); 6] = [
    (51, 187, 255),
    (255, 110, 110),
    (120, 220, 120),
    (200, 130, 255),
    (120, 220, 220),
    (240, 160, 70),
];

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

    let names: Vec<SharedString> = state
        .ladder_names()
        .into_iter()
        .map(SharedString::from)
        .collect();
    ui.set_ladder_names(ModelRc::new(VecModel::from(names.clone())));

    let mut rows: Vec<ModelRc<StandardListViewItem>> = Vec::new();
    if let Some(a) = state.analysis() {
        let unit = state.gel_type.size_unit();
        for b in &a.bands {
            let quant = a.quantifications.iter().find(|q| q.target_id == b.id);
            let ng = quant
                .and_then(|q| q.mass_ng)
                .map(|m| format!("{m:.1}"))
                .unwrap_or_else(|| "-".into());
            let nmol = quant
                .and_then(|q| q.molarity_nmol)
                .map(|m| format!("{m:.3}"))
                .unwrap_or_else(|| "-".into());
            rows.push(ModelRc::new(VecModel::from(vec![
                cell(&format!("{}", b.lane_id)),
                cell(&b.rf.map(|r| format!("{r:.2}")).unwrap_or_else(|| "-".into())),
                cell(&b
                    .size
                    .map(|s| format!("{s:.0} {unit}"))
                    .unwrap_or_else(|| "-".into())),
                cell(&format!("{:.1}", b.integrated_density)),
                cell(&ng),
                cell(&nmol),
            ])));
        }
        if let Some(assign) = a.ladder_assignments.first() {
            if let Some(idx) = names.iter().position(|n| *n == assign.template_name) {
                ui.set_selected_ladder(idx as i32);
            }
        }
    }

    ui.set_band_rows(ModelRc::new(VecModel::from(rows)));
    refresh_image(ui, state);
    refresh_trace(ui, state);
}

/// Rebuild the Trace tab: lane checklist, mode index, and the plotted image.
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
    ui.set_trace_image(render_trace_plot(&traces, 760, 360));
}

/// Render selected lanes' densitometry traces as a line plot (electropherogram).
fn render_trace_plot(traces: &[LaneTrace], w: u32, h: u32) -> Image {
    let mut buf = SharedPixelBuffer::<slint::Rgb8Pixel>::new(w, h);
    let px = buf.make_mut_slice();
    for p in px.iter_mut() {
        *p = slint::Rgb8Pixel { r: 24, g: 24, b: 28 };
    }
    let (ml, mr, mt, mb) = (46i32, 12i32, 12i32, 26i32);
    let (wi, hi) = (w as i32, h as i32);
    let (px0, py0, px1, py1) = (ml, mt, wi - mr, hi - mb);
    let axis = (90u8, 90u8, 96u8);
    // Axes.
    for y in py0..=py1 {
        put(px, w, h, px0, y, axis);
    }
    for x in px0..=px1 {
        put(px, w, h, x, py1, axis);
    }
    if traces.is_empty() {
        return Image::from_rgb8(buf);
    }
    // Shared y-scale across selected traces.
    let mut vmax = 0.0f64;
    for t in traces {
        for &v in &t.values {
            if v > vmax {
                vmax = v;
            }
        }
    }
    let vmax = vmax.max(1e-9);
    let (pw, ph) = ((px1 - px0).max(1) as f64, (py1 - py0).max(1) as f64);

    for (i, t) in traces.iter().enumerate() {
        let color = if t.ladder {
            (255u8, 204u8, 0u8)
        } else {
            PALETTE[i % PALETTE.len()]
        };
        let n = t.values.len();
        if n < 2 {
            continue;
        }
        let mut prev: Option<(i32, i32)> = None;
        for (k, &v) in t.values.iter().enumerate() {
            let x = px0 + (k as f64 / (n - 1) as f64 * pw).round() as i32;
            let y = py1 - ((v / vmax) * ph).round() as i32;
            if let Some(p) = prev {
                draw_line(px, w, h, p.0, p.1, x, y, color);
            }
            prev = Some((x, y));
        }
    }
    Image::from_rgb8(buf)
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

/// Bresenham line.
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

/// Re-render the gel image with overlays composited in (after a level/annotation
/// change). Rotation and zoom are applied live by the UI.
pub fn refresh_image(ui: &AppWindow, state: &AppState) {
    if let Some(work) = state.view_image() {
        let overlays = compute_overlays(state);
        let img = to_slint_image(&work, ui.get_level().max(0.01), &overlays);
        ui.set_gel_image(img);
    }
}

fn compute_overlays(state: &AppState) -> Vec<OverlayBox> {
    let mut out = Vec::new();
    let (Some(a), Some(work)) = (state.analysis(), state.view_image()) else {
        return out;
    };
    let (w, h) = (work.width().max(1) as f32, work.height().max(1) as f32);
    for lane in &a.lanes {
        out.push(OverlayBox {
            x: lane.x_min as f32 / w,
            y: 0.0,
            w: (lane.x_max - lane.x_min) as f32 / w,
            h: 1.0,
            ladder: lane.is_ladder,
            is_lane: true,
        });
    }
    for b in &a.bands {
        if let Some(lane) = a.lanes.iter().find(|l| l.id == b.lane_id) {
            out.push(OverlayBox {
                x: lane.x_min as f32 / w,
                y: ((b.y_center - b.y_half_width) as f32) / h,
                w: (lane.x_max - lane.x_min) as f32 / w,
                h: (2.0 * b.y_half_width as f32) / h,
                ladder: lane.is_ladder,
                is_lane: false,
            });
        }
    }
    out
}

fn cell(text: &str) -> StandardListViewItem {
    StandardListViewItem::from(SharedString::from(text))
}

/// Convert a working image to a displayable RGB [`Image`] with overlays drawn
/// on top. Grayscale maps `[0, max*level]` → `[0, 255]`.
fn to_slint_image(work: &GrayF32, level: f32, overlays: &[OverlayBox]) -> Image {
    let (w, h) = (work.width() as u32, work.height() as u32);
    let (_lo, hi) = work.min_max();
    let scale = (hi * level).max(1e-6);
    let mut buf = SharedPixelBuffer::<slint::Rgb8Pixel>::new(w, h);
    let px = buf.make_mut_slice();
    for y in 0..h as usize {
        for x in 0..w as usize {
            let v = (work.get(x, y) / scale).clamp(0.0, 1.0);
            let g = (v * 255.0) as u8;
            px[y * w as usize + x] = slint::Rgb8Pixel { r: g, g, b: g };
        }
    }
    for ov in overlays {
        draw_overlay(px, w, h, ov);
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
