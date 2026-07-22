//! OpenGel desktop GUI.
//!
//! Wires the Slint UI (`ui/app.slint`) to `gel-core` (documents, HDR, quant),
//! `gel-detect` (analysis pipeline) and `gel-camera` (capture).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod camera_glue;
mod state;
mod view;

use std::cell::RefCell;
use std::rc::Rc;

use slint::ComponentHandle;

use state::AppState;

slint::include_modules!();

fn main() -> anyhow::Result<()> {
    let ui = AppWindow::new()?;
    let state = Rc::new(RefCell::new(AppState::new()));

    // CLI: `opengel [PATH] [--demo]`. `--demo` loads the synthetic demo gel;
    // otherwise a positional path is opened immediately if given.
    {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.iter().any(|a| a == "--demo") {
            let msg = state.borrow_mut().load_demo();
            ui.set_status(msg.into());
        } else if let Some(path) = args.iter().find(|a| !a.starts_with("--")) {
            if let Err(e) = state.borrow_mut().open_path(std::path::Path::new(path)) {
                ui.set_status(format!("Open failed: {e}").into());
            }
        }
    }
    view::refresh(&ui, &state.borrow());

    // ---- callbacks ----
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_open(move || {
            let ui = ui_weak.unwrap();
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("OpenGel", &["zip"])
                .pick_file()
            {
                match state.borrow_mut().open_path(&path) {
                    Ok(()) => {}
                    Err(e) => ui.set_status(format!("Open failed: {e}").into()),
                }
                view::refresh(&ui, &state.borrow());
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_save(move || {
            let ui = ui_weak.unwrap();
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("OpenGel", &["zip"])
                .set_file_name("gel.gel.zip")
                .save_file()
            {
                match state.borrow().save_path(&path) {
                    Ok(()) => ui.set_status(format!("Saved {}", path.display()).into()),
                    Err(e) => ui.set_status(format!("Save failed: {e}").into()),
                }
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_capture(move || {
            let ui = ui_weak.unwrap();
            match state.borrow_mut().capture() {
                Ok(msg) => ui.set_status(msg.into()),
                Err(e) => ui.set_status(format!("Capture failed: {e}").into()),
            }
            view::refresh(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_analyze(move || {
            let ui = ui_weak.unwrap();
            state.borrow_mut().optical_flow = ui.get_optical_flow();
            match state.borrow_mut().analyze(None) {
                Ok(msg) => ui.set_status(msg.into()),
                Err(e) => ui.set_status(format!("Analyze failed: {e}").into()),
            }
            view::refresh(&ui, &state.borrow());
        });
    }
    // Per-lane ladder assignment and the "set all" convenience control.
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_set_all_ladders(move |idx| {
            let ui = ui_weak.unwrap();
            let msg = state.borrow_mut().set_all_ladders(idx as usize);
            ui.set_status(msg.into());
            view::refresh(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_lane_ladder_changed(move |lane_id, tidx| {
            let ui = ui_weak.unwrap();
            let msg = state
                .borrow_mut()
                .set_lane_ladder(lane_id as u32, tidx as usize);
            ui.set_status(msg.into());
            view::refresh(&ui, &state.borrow());
        });
    }
    {
        let state = state.clone();
        ui.on_lane_load_changed(move |lane_id, text| {
            if let Ok(ng) = text.parse::<f64>() {
                state.borrow_mut().set_ladder_load(lane_id as u32, ng);
            }
        });
    }

    // Tree list: expand/collapse, ladder "Set", the "…" menu, and rename.
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_lane_toggle(move |lane_id| {
            let ui = ui_weak.unwrap();
            state.borrow_mut().toggle_expanded(lane_id as u32);
            view::refresh(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_lane_set(move |lane_id| {
            let ui = ui_weak.unwrap();
            let msg = state.borrow_mut().reapply_lane_ladder(lane_id as u32);
            ui.set_status(msg.into());
            view::refresh(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_lane_menu(move |lane_id, action| {
            let ui = ui_weak.unwrap();
            let msg = match action.as_str() {
                "delete" => state.borrow_mut().delete_lane(lane_id as u32),
                "ladder" => {
                    // Toggle: mark if not a ladder, unmark if it is.
                    let is = state
                        .borrow()
                        .analysis()
                        .map(|a| a.lanes.iter().any(|l| l.id == lane_id as u32 && l.is_ladder))
                        .unwrap_or(false);
                    state.borrow_mut().set_lane_is_ladder(lane_id as u32, !is)
                }
                _ => String::new(),
            };
            ui.set_status(msg.into());
            view::refresh(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_lane_renamed(move |lane_id, name| {
            let ui = ui_weak.unwrap();
            let msg = state.borrow_mut().set_lane_label(lane_id as u32, name.as_str());
            ui.set_status(msg.into());
            view::refresh(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_compare(move || {
            let ui = ui_weak.unwrap();
            let vol: f64 = ui.get_volume_ul().parse().unwrap_or(0.0);
            let msg = state.borrow().compare_first_two(vol);
            ui.set_status(msg.into());
        });
    }
    {
        let ui_weak = ui.as_weak();
        ui.on_about(move || {
            ui_weak.unwrap().set_status(
                "OpenGel — capture, detect and quantify gel images. MIT/Apache-2.0.".into(),
            );
        });
    }
    // Switch which captured frame is displayed (or the merged HDR image).
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_frame_changed(move |idx| {
            let ui = ui_weak.unwrap();
            state.borrow_mut().set_view_frame(idx as usize);
            view::refresh_image(&ui, &state.borrow());
        });
    }
    // Re-render when the contrast window or invert toggle changes.
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_display_changed(move || {
            let ui = ui_weak.unwrap();
            {
                let mut s = state.borrow_mut();
                s.set_display_window(ui.get_disp_lo(), ui.get_disp_hi());
                s.set_invert(ui.get_invert());
                s.set_show_unwarped(ui.get_show_unwarped());
            }
            view::refresh_image(&ui, &state.borrow());
        });
    }
    // Live rotation is applied visually by the UI (Slint rotation-angle); we
    // only record the angle for the deferred detection path.
    {
        let state = state.clone();
        ui.on_rotate(move |deg| {
            state.borrow_mut().set_rotation(deg as f64);
        });
    }
    // Region measurement from the current annotation.
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_measure(move || {
            let ui = ui_weak.unwrap();
            let msg = state.borrow_mut().measure_regions();
            ui.set_status(msg.into());
            view::refresh(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_auto_straighten(move || {
            let ui = ui_weak.unwrap();
            let msg = state.borrow_mut().auto_straighten();
            ui.set_rotation(state.borrow().rotation_deg as f32);
            ui.set_status(msg.into());
            view::refresh(&ui, &state.borrow());
        });
    }
    // Editing: each acts on the last click position (click-x/click-y).
    macro_rules! edit_cb {
        ($setter:ident, $body:expr) => {{
            let ui_weak = ui.as_weak();
            let state = state.clone();
            ui.$setter(move || {
                let ui = ui_weak.unwrap();
                let (nx, ny) = (ui.get_click_x() as f64, ui.get_click_y() as f64);
                #[allow(clippy::redundant_closure_call)]
                let msg = ($body)(&mut *state.borrow_mut(), nx, ny);
                ui.set_status(msg.into());
                view::refresh(&ui, &state.borrow());
            });
        }};
    }
    edit_cb!(on_add_lane, |s: &mut AppState, nx: f64, _ny: f64| s.add_lane_at(nx));
    edit_cb!(on_del_lane, |s: &mut AppState, nx: f64, _ny: f64| s.delete_lane_near(nx));
    edit_cb!(on_toggle_ladder, |s: &mut AppState, nx: f64, _ny: f64| s.toggle_ladder_near(nx));
    edit_cb!(on_add_band, |s: &mut AppState, nx: f64, ny: f64| s.add_band_at(nx, ny));
    edit_cb!(on_del_band, |s: &mut AppState, nx: f64, ny: f64| s.delete_band_near(nx, ny));
    // Absolute calibration.
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_calibrate(move || {
            let ui = ui_weak.unwrap();
            let vol: f64 = ui.get_volume_ul().parse().unwrap_or(0.0);
            let msg = state.borrow_mut().calibrate(vol);
            ui.set_status(msg.into());
            view::refresh(&ui, &state.borrow());
        });
    }

    // Trace tab: lane selection + y-axis mode.
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_toggle_lane(move |id| {
            let ui = ui_weak.unwrap();
            state.borrow_mut().toggle_lane(id as u32);
            view::refresh_trace(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_trace_mode_changed(move |idx| {
            let ui = ui_weak.unwrap();
            state.borrow_mut().set_trace_mode(idx as usize);
            let mode = state.borrow().trace_mode.label();
            ui.set_status(format!("Trace y-axis: {mode}").into());
            view::refresh_trace(&ui, &state.borrow());
        });
    }

    ui.run()?;
    Ok(())
}
