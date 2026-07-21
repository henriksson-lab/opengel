//! Rendering application state into the Slint UI models.

use gel_core::GrayF32;
use slint::{Image, ModelRc, SharedPixelBuffer, SharedString, StandardListViewItem, VecModel};

use crate::state::AppState;
use crate::{AppWindow, Overlay};

/// Full refresh: labels, ladder list, overlays, band table and image.
pub fn refresh(ui: &AppWindow, state: &AppState) {
    ui.set_gel_type_label(format!("{:?}", state.gel_type).into());

    // Ladder dropdown.
    let names: Vec<SharedString> = state
        .ladder_names()
        .into_iter()
        .map(SharedString::from)
        .collect();
    ui.set_ladder_names(ModelRc::new(VecModel::from(names.clone())));

    let mut rows: Vec<ModelRc<StandardListViewItem>> = Vec::new();
    let mut overlays: Vec<Overlay> = Vec::new();

    let display = state.view_image();
    if let (Some(a), Some(work)) = (state.analysis(), display.as_ref()) {
        let (w, h) = (work.width().max(1) as f32, work.height().max(1) as f32);
        let unit = state.gel_type.size_unit();

        // Lane outlines (full height) as translucent columns.
        for lane in &a.lanes {
            overlays.push(Overlay {
                x: lane.x_min as f32 / w,
                y: 0.0,
                w: (lane.x_max - lane.x_min) as f32 / w,
                h: 1.0,
                ladder: lane.is_ladder,
                label: lane
                    .label
                    .clone()
                    .unwrap_or_default()
                    .into(),
            });
        }

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
            let cells = vec![
                cell(&format!("{}", b.lane_id)),
                cell(&b.rf.map(|r| format!("{r:.2}")).unwrap_or_else(|| "-".into())),
                cell(&b
                    .size
                    .map(|s| format!("{s:.0} {unit}"))
                    .unwrap_or_else(|| "-".into())),
                cell(&format!("{:.1}", b.integrated_density)),
                cell(&ng),
                cell(&nmol),
            ];
            rows.push(ModelRc::new(VecModel::from(cells)));

            if let Some(lane) = a.lanes.iter().find(|l| l.id == b.lane_id) {
                overlays.push(Overlay {
                    x: lane.x_min as f32 / w,
                    y: ((b.y_center - b.y_half_width) as f32) / h,
                    w: (lane.x_max - lane.x_min) as f32 / w,
                    h: (2.0 * b.y_half_width as f32) / h,
                    ladder: lane.is_ladder,
                    label: b
                        .size
                        .map(|s| format!("{s:.0}"))
                        .unwrap_or_default()
                        .into(),
                });
            }
        }

        // Reflect the identified ladder in the dropdown.
        if let Some(assign) = a.ladder_assignments.first() {
            if let Some(idx) = names.iter().position(|n| *n == assign.template_name) {
                ui.set_selected_ladder(idx as i32);
            }
        }
    }

    ui.set_band_rows(ModelRc::new(VecModel::from(rows)));
    ui.set_overlays(ModelRc::new(VecModel::from(overlays)));
    refresh_image(ui, state);
}

/// Re-render just the gel image (e.g. after a display-level change). Rotation
/// is applied live by the UI, so the raw image is used here.
pub fn refresh_image(ui: &AppWindow, state: &AppState) {
    if let Some(work) = state.view_image() {
        let img = to_slint_image(&work, ui.get_level().max(0.01));
        ui.set_gel_image(img);
    }
}

fn cell(text: &str) -> StandardListViewItem {
    StandardListViewItem::from(SharedString::from(text))
}

/// Convert a working image to a displayable grayscale [`Image`], mapping
/// `[0, max*level]` to `[0, 255]` so lowering `level` brightens faint bands.
fn to_slint_image(work: &GrayF32, level: f32) -> Image {
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
    Image::from_rgb8(buf)
}
