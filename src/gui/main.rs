//! OpenGel desktop GUI.
//!
//! Wires the Slint UI (`ui/app.slint`) to the core document/HDR/quant modules,
//! the analysis pipeline and camera capture.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod camera_glue;
mod camera_worker;
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

    // CLI: `opengel [PATH] [FLAGS]`.
    //   --demo            load the synthetic demo gel instead of a path
    //   --detect          run lane/band detection (fits the NURBS warp) on startup
    //   --optical-flow    fit the warp by optical flow (implies + used by --detect)
    //   --show-warp       overlay the NURBS warp grid
    //   --show-unwarped   show the rectified (dewarped) view
    //   --invert          invert display colors
    // The view toggles set both app state (drives rendering) and the matching UI
    // property (so the checkboxes reflect the startup state) — handy for scripted
    // screenshots.
    {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let has = |name: &str| args.iter().any(|a| a == name);
        // Value of `--flag=VALUE` or `--flag VALUE`.
        let value = |name: &str| -> Option<String> {
            for (i, a) in args.iter().enumerate() {
                if let Some(v) = a.strip_prefix(&format!("{name}=")) {
                    return Some(v.to_string());
                }
                if a == name {
                    return args.get(i + 1).cloned();
                }
            }
            None
        };

        // Load the document first (demo or positional path).
        if has("--demo") {
            let msg = state.borrow_mut().load_demo();
            ui.set_status(msg.into());
        } else if let Some(path) = args.iter().find(|a| !a.starts_with("--")) {
            if let Err(e) = state.borrow_mut().open_path(std::path::Path::new(path)) {
                ui.set_status(format!("Open failed: {e}").into());
            }
        }

        // View + analysis toggles.
        let optical_flow = has("--optical-flow");
        state.borrow_mut().optical_flow = optical_flow;
        ui.set_optical_flow(optical_flow);

        if has("--invert") {
            state.borrow_mut().set_invert(true);
            ui.set_invert(true);
        }
        // `--transparency PCT` (0 = opaque overlays, 100 = fully transparent).
        if let Some(pct) = value("--transparency").and_then(|s| s.parse::<f32>().ok()) {
            let alpha = (1.0 - pct / 100.0).clamp(0.0, 1.0);
            state.borrow_mut().set_annotation_alpha(alpha);
            ui.set_annotation_alpha(alpha);
        }
        if has("--detect") {
            match state.borrow_mut().analyze(None) {
                Ok(msg) => ui.set_status(msg.into()),
                Err(e) => ui.set_status(format!("Detect failed: {e}").into()),
            }
        }
        if has("--show-warp") {
            state.borrow_mut().set_show_warp(true);
            ui.set_show_warp(true);
        }
        if has("--show-unwarped") {
            state.borrow_mut().set_show_unwarped(true);
            ui.set_show_unwarped(true);
        }
    }
    view::refresh(&ui, &state.borrow());
    view::refresh_live(&ui, &state.borrow());

    // ---- Camera worker thread + event pump ----
    // All camera I/O runs off the UI thread; the worker pushes events which a
    // UI-thread timer drains and applies. This keeps the GUI responsive during
    // slow opens and long exposures.
    let (cam_handle, cam_events) = camera_worker::spawn();
    cam_handle.list_cameras();
    state.borrow_mut().cam = Some(cam_handle);

    let event_pump = Rc::new(slint::Timer::default());
    {
        use camera_worker::CamEvent;
        let ui_weak = ui.as_weak();
        let state = state.clone();
        event_pump.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(30),
            move || {
                let ui = ui_weak.unwrap();
                let mut live_dirty = false;
                let mut doc_dirty = false;
                while let Ok(evt) = cam_events.try_recv() {
                    let mut st = state.borrow_mut();
                    match evt {
                        CamEvent::Cameras(names) => {
                            st.set_cameras(names);
                            live_dirty = true;
                        }
                        CamEvent::Opened {
                            name,
                            manual_exposure,
                        } => {
                            st.camera_name = name;
                            st.exposure_supported = manual_exposure;
                            live_dirty = true;
                        }
                        CamEvent::OpenFailed(e) => {
                            st.live_running = false;
                            drop(st);
                            ui.set_status(format!("Camera open failed: {e}").into());
                            live_dirty = true;
                        }
                        CamEvent::Preview(frame) => {
                            st.preview = Some(frame);
                            live_dirty = true;
                        }
                        CamEvent::CaptureProgress { done, total } => {
                            st.capture_status = if total <= 1 {
                                "Capturing…".into()
                            } else {
                                format!("Capturing HDR bracket ({done}/{total})…")
                            };
                            live_dirty = true;
                        }
                        CamEvent::CaptureDone(frames) => {
                            let n = frames.len();
                            let (imgs, metas): (Vec<_>, Vec<_>) = frames.into_iter().unzip();
                            st.adopt_capture(imgs, metas);
                            st.capturing = false;
                            let name = st.camera_name.clone();
                            drop(st);
                            ui.set_status(
                                if n <= 1 {
                                    format!("Captured single frame from {name}.")
                                } else {
                                    format!("Captured {n}-frame HDR bracket from {name}.")
                                }
                                .into(),
                            );
                            doc_dirty = true;
                        }
                        CamEvent::CaptureFailed(e) => {
                            st.capturing = false;
                            drop(st);
                            ui.set_status(format!("Capture failed: {e}").into());
                            live_dirty = true;
                        }
                        CamEvent::Cancelled => {
                            st.capturing = false;
                            drop(st);
                            ui.set_status("Capture cancelled.".into());
                            live_dirty = true;
                        }
                    }
                }
                if doc_dirty {
                    view::refresh(&ui, &state.borrow());
                }
                if doc_dirty || live_dirty {
                    view::refresh_live(&ui, &state.borrow());
                }
            },
        );
    }

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
            match state.borrow_mut().analyze(None) {
                Ok(msg) => ui.set_status(msg.into()),
                Err(e) => ui.set_status(format!("Analyze failed: {e}").into()),
            }
            view::refresh(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_open_fit_dialog(move || {
            let ui = ui_weak.unwrap();
            let st = state.borrow();
            ui.set_fit_dialog_optical_flow(st.optical_flow);
            ui.set_fit_dialog_extra_edges(st.extra_vertical_edges.max(2).to_string().into());
            ui.set_fit_dialog_warp_regularization(format!("{:.4}", st.warp_regularization).into());
            ui.set_fit_dialog_row_spacing(format!("{:.3}", st.row_spacing_weight).into());
            ui.set_fit_dialog_flow_smoothness(format!("{:.3}", st.flow_smoothness).into());
            ui.set_fit_dialog_open(true);
        });
    }
    {
        let ui_weak = ui.as_weak();
        ui.on_cancel_fit_dialog(move || {
            let ui = ui_weak.unwrap();
            ui.set_fit_dialog_open(false);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_apply_fit_dialog(move || {
            let ui = ui_weak.unwrap();
            let extra_edges = ui
                .get_fit_dialog_extra_edges()
                .parse::<usize>()
                .unwrap_or(2)
                .max(2);
            let warp_regularization = ui
                .get_fit_dialog_warp_regularization()
                .parse::<f64>()
                .unwrap_or(1e-2)
                .max(0.0);
            let row_spacing_weight = ui
                .get_fit_dialog_row_spacing()
                .parse::<f64>()
                .unwrap_or(10.0)
                .max(0.0);
            let flow_smoothness = ui
                .get_fit_dialog_flow_smoothness()
                .parse::<f64>()
                .unwrap_or(8.0)
                .max(0.0);
            {
                let mut st = state.borrow_mut();
                st.optical_flow = ui.get_fit_dialog_optical_flow();
                st.extra_vertical_edges = extra_edges;
                st.warp_regularization = warp_regularization;
                st.row_spacing_weight = row_spacing_weight;
                st.flow_smoothness = flow_smoothness;
            }
            ui.set_fit_dialog_extra_edges(extra_edges.to_string().into());
            ui.set_fit_dialog_warp_regularization(format!("{warp_regularization:.4}").into());
            ui.set_fit_dialog_row_spacing(format!("{row_spacing_weight:.3}").into());
            ui.set_fit_dialog_flow_smoothness(format!("{flow_smoothness:.3}").into());
            ui.set_fit_dialog_open(false);
            match state.borrow_mut().analyze(None) {
                Ok(msg) => ui.set_status(msg.into()),
                Err(e) => ui.set_status(format!("Analyze failed: {e}").into()),
            }
            view::refresh(&ui, &state.borrow());
        });
    }
    // "Use as ladder" dialog: open (prefill), apply, cancel.
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_open_ladder_dialog(move |lane_id| {
            let ui = ui_weak.unwrap();
            let (name, tidx, vol, conc) = state.borrow().ladder_dialog_prefill(lane_id as u32);
            ui.set_dialog_lane_name(name.into());
            ui.set_dialog_ladder_index(tidx.max(0));
            ui.set_dialog_volume(format!("{vol:.1}").into());
            ui.set_dialog_conc(format!("{conc:.1}").into());
            ui.set_ladder_dialog_lane(lane_id);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_apply_ladder_dialog(move || {
            let ui = ui_weak.unwrap();
            let lane_id = ui.get_ladder_dialog_lane();
            if lane_id >= 0 {
                let tidx = ui.get_dialog_ladder_index().max(0) as usize;
                let vol: f64 = ui.get_dialog_volume().parse().unwrap_or(0.0);
                let conc: f64 = ui.get_dialog_conc().parse().unwrap_or(0.0);
                let msg = state
                    .borrow_mut()
                    .apply_ladder_dialog(lane_id as u32, tidx, vol, conc);
                ui.set_status(msg.into());
            }
            ui.set_ladder_dialog_lane(-1);
            view::refresh(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        ui.on_cancel_ladder_dialog(move || {
            ui_weak.unwrap().set_ladder_dialog_lane(-1);
        });
    }
    // Band "…" menu.
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_band_menu(move |band_id, action| {
            let ui = ui_weak.unwrap();
            if action.as_str() == "delete" {
                let msg = state.borrow_mut().delete_band_by_id(band_id as u32);
                ui.set_status(msg.into());
                view::refresh(&ui, &state.borrow());
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
                "unmark" => state.borrow_mut().set_lane_is_ladder(lane_id as u32, false),
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
            let msg = state
                .borrow_mut()
                .set_lane_label(lane_id as u32, name.as_str());
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
                s.set_show_warp(ui.get_show_warp());
                s.set_normalize_inner_knots(ui.get_normalize_inner_knots());
                s.set_show_overexposed(ui.get_show_overexposed());
                s.set_annotation_alpha(ui.get_annotation_alpha());
            }
            view::refresh_image(&ui, &state.borrow());
        });
    }
    // Mouse hover over the gel → migration-alignment line (follows the warp).
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_hover_moved(move |x, y| {
            let ui = ui_weak.unwrap();
            state.borrow_mut().set_hover(x, y);
            let st = state.borrow();
            // Live size readout in the status bar (only when a ladder is fitted).
            if let Some(label) = st.hover_size_label(y) {
                ui.set_status(label.into());
            }
            // Only the cheap re-render is needed; skip when nothing to draw.
            if st.show_warp || st.analysis().is_some() {
                view::refresh_image(&ui, &st);
            }
        });
    }
    // Select / drag annotations.
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_press_at(move |x, y| {
            let ui = ui_weak.unwrap();
            // Prefer grabbing a warp knot (when the grid is shown); otherwise
            // fall back to selecting/dragging an annotation.
            let hit = {
                let mut st = state.borrow_mut();
                st.press_warp_knot(x as f64, y as f64) || st.press_annotation(x as f64, y as f64)
            };
            view::refresh_image(&ui, &state.borrow());
            hit
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_drag_to(move |x, y| {
            let ui = ui_weak.unwrap();
            {
                let mut st = state.borrow_mut();
                if st.is_dragging_knot() {
                    st.drag_warp_knot(x as f64, y as f64);
                } else {
                    st.drag_selection(x as f64, y as f64);
                }
            }
            view::refresh_image(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_release_annotation(move || {
            let ui = ui_weak.unwrap();
            {
                let mut st = state.borrow_mut();
                if st.is_dragging_knot() {
                    st.release_warp_knot();
                } else {
                    st.release_selection();
                }
            }
            view::refresh(&ui, &state.borrow());
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
    edit_cb!(on_add_lane, |s: &mut AppState, nx: f64, _ny: f64| s
        .add_lane_at(nx));
    edit_cb!(on_add_band, |s: &mut AppState, nx: f64, ny: f64| s
        .add_band_at(nx, ny));
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

    // ---- Live tab (camera I/O runs on the worker thread; see the event pump) ----
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_live_start(move || {
            let ui = ui_weak.unwrap();
            let msg = state.borrow_mut().live_start();
            ui.set_status(msg.into());
            view::refresh_live(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_live_stop(move || {
            let ui = ui_weak.unwrap();
            state.borrow_mut().live_stop();
            view::refresh_live(&ui, &state.borrow());
            ui.set_status("Live preview stopped.".into());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_camera_selected(move |idx| {
            let ui = ui_weak.unwrap();
            state.borrow_mut().select_camera(idx as usize);
            view::refresh_live(&ui, &state.borrow());
        });
    }
    {
        let state = state.clone();
        ui.on_rescan_cameras(move || {
            state.borrow().refresh_cameras();
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_live_exposure_changed(move |f| {
            let ui = ui_weak.unwrap();
            state.borrow_mut().set_live_exposure_slider(f);
            view::refresh_live(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_set_hdr_lower(move || {
            let ui = ui_weak.unwrap();
            state.borrow_mut().set_hdr_lower_from_current();
            view::refresh_live(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_set_hdr_upper(move || {
            let ui = ui_weak.unwrap();
            state.borrow_mut().set_hdr_upper_from_current();
            view::refresh_live(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_hdr_steps_changed(move |idx| {
            let ui = ui_weak.unwrap();
            state.borrow_mut().set_hdr_steps_idx(idx as usize);
            view::refresh_live(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_live_capture(move || {
            let ui = ui_weak.unwrap();
            state.borrow_mut().live_capture();
            view::refresh_live(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_capture_single(move || {
            let ui = ui_weak.unwrap();
            state.borrow_mut().capture_single();
            view::refresh_live(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_cancel_capture(move || {
            let ui = ui_weak.unwrap();
            state.borrow_mut().cancel_capture();
            view::refresh_live(&ui, &state.borrow());
        });
    }

    ui.run()?;
    Ok(())
}
