//! Rendering application state into the Slint UI. Overlays (lane columns and
//! band bars) are composited directly into the displayed image so they rotate
//! and zoom together with it.

use gel_core::GrayF32;
use slint::{Image, ModelRc, SharedPixelBuffer, SharedString, StandardListViewItem, VecModel};

use crate::state::AppState;
use crate::AppWindow;

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
