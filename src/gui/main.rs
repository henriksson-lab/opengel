//! OpenGel desktop GUI.
//!
//! Wires the Slint UI (`ui/app.slint`) to the core document/HDR/quant modules,
//! the analysis pipeline and camera capture.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod camera_glue;
mod camera_worker;
mod config;
mod email;
mod geldoc;
mod instrument_worker;
mod state;
mod view;

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use state::AppState;

slint::include_modules!();

const SOURCE_URL: &str = "https://github.com/henriksson-lab/opengel";
const CITING_URL: &str = "https://github.com/henriksson-lab/opengel#citing";

fn gelgenie_model_available() -> bool {
    #[cfg(feature = "gelgenie-ml")]
    {
        opengel::detect::GelGenieDetector::model_available()
    }
    #[cfg(not(feature = "gelgenie-ml"))]
    {
        false
    }
}

fn set_ladder_dialog_models(ui: &AppWindow, state: &AppState, vendor_index: usize) {
    let (vendors, names) = state.ladder_dialog_options_for_vendor_index(vendor_index);
    let vendor_model: Vec<SharedString> = vendors.into_iter().map(SharedString::from).collect();
    let ladder_model: Vec<SharedString> = names.into_iter().map(SharedString::from).collect();
    ui.set_ladder_vendor_names(ModelRc::new(VecModel::from(vendor_model)));
    ui.set_ladder_names(ModelRc::new(VecModel::from(ladder_model)));
}

fn main() -> anyhow::Result<()> {
    let ui = AppWindow::new()?;
    let state = Rc::new(RefCell::new(AppState::new()));
    let app_config = Rc::new(RefCell::new(config::load_config()));
    state
        .borrow_mut()
        .set_recent_ladders(app_config.borrow().recent_ladders.clone());
    let email_settings = Rc::new(RefCell::new(email::load_settings()));

    // CLI: `opengel [PATH] [FLAGS]`.
    //   --demo            load the synthetic demo gel instead of a path
    //   --detect          run lane/band detection (fits the NURBS warp) on startup
    //   --optical-flow    fit the warp by optical flow (implies + used by --detect)
    //   --show-warp       overlay the NURBS warp grid
    //   --show-unwarped   show the rectified (dewarped) view
    //   --invert          invert display colors
    //   --tab N           select a tab (0 Gel, 1 Trace, 2 Live, 3 Gel Doc EZ,
    //                     4 Metadata)
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

        // Flags that take a separate value. The value looks like a positional
        // argument, so it has to be excluded before hunting for the file path —
        // otherwise `--tab 3` tries to open a file called "3".
        const VALUE_FLAGS: [&str; 2] = ["--transparency", "--tab"];
        let is_flag_value = |i: usize| {
            i > 0 && VALUE_FLAGS.contains(&args[i - 1].as_str())
        };

        // Load the document first (demo or positional path).
        if has("--demo") {
            let msg = state.borrow_mut().load_demo();
            ui.set_status(msg.into());
        } else if let Some(path) = args
            .iter()
            .enumerate()
            .find(|(i, a)| !a.starts_with("--") && !is_flag_value(*i))
            .map(|(_, a)| a)
        {
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
                Ok(msg) => {
                    ui.set_rotation(state.borrow().rotation_deg as f32);
                    ui.set_status(msg.into());
                }
                Err(e) => ui.set_status(format!("Detect failed: {e}").into()),
            }
        }
        if has("--show-warp") {
            state.borrow_mut().set_show_warp(true);
            ui.set_show_warp(true);
        }
        // `--tab N` selects a tab (0 = Gel, 1 = Trace, 2 = Live, 3 = Gel Doc EZ,
        // 4 = Metadata),
        // so a screenshot script can capture any of them.
        if let Some(tab) = value("--tab").and_then(|s| s.parse::<i32>().ok()) {
            ui.set_active_tab(tab.max(0));
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

    // ---- Instrument worker thread ----
    // Same pattern as the camera: the enclosure is polled off the UI thread, and
    // a run is sequenced across both workers by the pump below.
    let (inst_handle, inst_events) = instrument_worker::spawn();
    inst_handle.list();
    {
        let mut st = state.borrow_mut();
        st.geldoc.library = app_config
            .borrow()
            .geldoc_protocols
            .clone()
            .unwrap_or_else(opengel::instrument::protocol::ProtocolLibrary::starter);
        st.geldoc.inst = Some(inst_handle);
    }
    view::refresh_geldoc(&ui, &state.borrow());

    let event_pump = Rc::new(slint::Timer::default());
    {
        use camera_worker::CamEvent;
        use instrument_worker::InstEvent;
        let ui_weak = ui.as_weak();
        let state = state.clone();
        event_pump.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(30),
            move || {
                let ui = ui_weak.unwrap();
                let mut live_dirty = false;
                let mut doc_dirty = false;
                let mut geldoc_dirty = false;

                // --- instrument events ---
                while let Ok(evt) = inst_events.try_recv() {
                    let mut st = state.borrow_mut();
                    geldoc_dirty = true;
                    match evt {
                        InstEvent::Instruments(names) => {
                            if st.geldoc.selected_instrument >= names.len() {
                                st.geldoc.selected_instrument = 0;
                            }
                            st.geldoc.instruments = names;
                        }
                        InstEvent::Connected { info, simulated } => {
                            st.geldoc.connected = true;
                            st.geldoc.simulated = simulated;
                            st.geldoc.message = format!("Connected to {}.", info.model);
                            st.geldoc.info = info;
                            let watch = st.geldoc.watch_run_button;
                            if let Some(inst) = &st.geldoc.inst {
                                inst.watch_run_button(watch);
                            }
                        }
                        InstEvent::ConnectFailed(e) => {
                            st.geldoc.connected = false;
                            st.geldoc.message = format!("Connection failed: {e}");
                            drop(st);
                            ui.set_status(format!("Instrument connection failed: {e}").into());
                        }
                        InstEvent::Disconnected => {
                            st.geldoc.connected = false;
                            st.geldoc.sense = None;
                            st.geldoc.faults = opengel::instrument::Faults::NONE;
                            st.geldoc.message = "Disconnected.".into();
                        }
                        InstEvent::Status {
                            sense,
                            faults,
                            undecoded,
                        } => {
                            st.geldoc.sense = Some(sense);
                            st.geldoc.faults = faults;
                            st.geldoc.undecoded = undecoded;
                        }
                        InstEvent::ButtonPressed { mask } => {
                            // The hardware Run button (most likely — see the
                            // worker). It runs the default protocol for whatever
                            // tray is actually in, which is what the button
                            // means on the instrument.
                            if st.geldoc.watch_run_button && !st.geldoc.phase.is_running() {
                                let msg = st.geldoc_button_run();
                                drop(st);
                                ui.set_status(msg.into());
                            } else {
                                st.geldoc.message =
                                    format!("Sense bit 0x{mask:04x} went high.");
                            }
                        }
                        InstEvent::Activating { elapsed_s, total_s } => {
                            st.geldoc.phase = geldoc::RunPhase::Activating { elapsed_s, total_s };
                            st.geldoc.message = st.geldoc.phase.label();
                        }
                        InstEvent::LightsReady => {
                            st.geldoc_lights_ready();
                            live_dirty = true;
                        }
                        InstEvent::RunRefused(reason) => {
                            st.geldoc.phase = geldoc::RunPhase::Idle;
                            st.geldoc.message = reason.clone();
                            st.capturing = false;
                            drop(st);
                            ui.set_status(format!("Run refused: {reason}").into());
                        }
                        InstEvent::RunFinished {
                            faults,
                            door_violation,
                        } => {
                            let msg = st.geldoc_run_finished(faults, door_violation);
                            drop(st);
                            ui.set_status(msg.into());
                            doc_dirty = true;
                            live_dirty = true;
                        }
                        InstEvent::Error(e) => {
                            st.geldoc.message = e.clone();
                            drop(st);
                            ui.set_status(e.into());
                        }
                    }
                }

                // --- camera events ---
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
                        CamEvent::Metering { attempt, exposure_s } => {
                            st.capture_status =
                                format!("Metering (attempt {attempt}) at {exposure_s:.3} s…");
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
                            st.capturing = false;
                            if st.cancel_requested {
                                // Arrived after the user cancelled — discard it.
                                st.cancel_requested = false;
                                drop(st);
                                live_dirty = true;
                            } else if st.geldoc.phase == geldoc::RunPhase::Exposing {
                                // A Gel Doc EZ run: hold the frames rather than
                                // adopting them, until the instrument confirms
                                // the door stayed shut for the whole exposure.
                                st.geldoc_capture_done(frames);
                                drop(st);
                                geldoc_dirty = true;
                                live_dirty = true;
                            } else {
                                let n = frames.len();
                                let (imgs, metas): (Vec<_>, Vec<_>) = frames.into_iter().unzip();
                                st.adopt_capture(imgs, metas);
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
                        }
                        CamEvent::CaptureFailed(e) => {
                            st.capturing = false;
                            let cancelled = st.cancel_requested;
                            st.cancel_requested = false;
                            // A failed exposure still leaves the lamps on, so
                            // the run has to be ended rather than just reported.
                            if st.geldoc.phase.is_running() {
                                st.geldoc.abort_run(format!("Exposure failed: {e}"));
                                geldoc_dirty = true;
                            }
                            drop(st);
                            if !cancelled {
                                ui.set_status(format!("Capture failed: {e}").into());
                            }
                            live_dirty = true;
                        }
                        CamEvent::Cancelled => {
                            st.capturing = false;
                            st.cancel_requested = false;
                            if st.geldoc.phase.is_running() {
                                st.geldoc.abort_run("Run cancelled.");
                                geldoc_dirty = true;
                            }
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
                if doc_dirty || geldoc_dirty {
                    view::refresh_geldoc(&ui, &state.borrow());
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
                .add_filter("Gel documents", &["zip", "scn", "mscn", "sscn", "smscn"])
                .add_filter("OpenGel", &["zip"])
                .add_filter("Bio-Rad Image Lab", &["scn", "mscn", "sscn", "smscn"])
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
        ui.on_file_changed(move |idx| {
            if idx < 0 {
                return;
            }
            let ui = ui_weak.unwrap();
            let msg = state.borrow_mut().set_active_document(idx as usize);
            ui.set_status(msg.into());
            view::refresh(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_close_current_file(move || {
            let ui = ui_weak.unwrap();
            ui.set_close_confirm_open(false);
            let msg = state.borrow_mut().close_active_document();
            ui.set_status(msg.into());
            view::refresh(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_save(move || {
            let ui = ui_weak.unwrap();
            let existing_path = state.borrow().source_path.clone();
            match existing_path {
                Some(path) => {
                    match state.borrow_mut().save_path(&path) {
                        Ok(()) => ui.set_status(format!("Saved {}", path.display()).into()),
                        Err(e) => ui.set_status(format!("Save failed: {e}").into()),
                    }
                    view::refresh(&ui, &state.borrow());
                }
                None => {
                    let default_name = state.borrow().save_dialog_filename();
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("OpenGel", &["zip"])
                        .set_file_name(default_name)
                        .save_file()
                    {
                        match state.borrow_mut().save_as(&path) {
                            Ok(()) => ui.set_status(format!("Saved {}", path.display()).into()),
                            Err(e) => ui.set_status(format!("Save failed: {e}").into()),
                        }
                        view::refresh(&ui, &state.borrow());
                    }
                }
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_save_as(move || {
            let ui = ui_weak.unwrap();
            let default_name = state.borrow().save_dialog_filename();
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("OpenGel", &["zip"])
                .set_file_name(default_name)
                .save_file()
            {
                match state.borrow_mut().save_as(&path) {
                    Ok(()) => ui.set_status(format!("Saved {}", path.display()).into()),
                    Err(e) => ui.set_status(format!("Save failed: {e}").into()),
                }
                view::refresh(&ui, &state.borrow());
            }
        });
    }
    // Recompute-HDR dialog: open (sync toggles), apply (recompute), cancel.
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_open_hdr_dialog(move || {
            let ui = ui_weak.unwrap();
            let st = state.borrow();
            ui.set_hdr_bias_subtraction(st.hdr_bias_subtraction);
            ui.set_hdr_align(st.hdr_align);
            ui.set_hdr_deghost(st.hdr_deghost);
            ui.set_hdr_dialog_open(true);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_apply_hdr_dialog(move || {
            let ui = ui_weak.unwrap();
            ui.set_hdr_dialog_open(false);
            {
                let mut st = state.borrow_mut();
                st.hdr_bias_subtraction = ui.get_hdr_bias_subtraction();
                st.hdr_align = ui.get_hdr_align();
                st.hdr_deghost = ui.get_hdr_deghost();
            }
            let msg = state.borrow_mut().recompute_hdr();
            ui.set_status(msg.into());
            view::refresh(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        ui.on_cancel_hdr_dialog(move || {
            ui_weak.unwrap().set_hdr_dialog_open(false);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let email_settings = email_settings.clone();
        ui.on_open_email_settings(move || {
            let ui = ui_weak.unwrap();
            let settings = email_settings.borrow();
            ui.set_email_smtp_host(settings.smtp_host.clone().into());
            ui.set_email_smtp_port(settings.smtp_port.to_string().into());
            ui.set_email_smtp_username(settings.smtp_username.clone().into());
            ui.set_email_smtp_password(settings.smtp_password.clone().into());
            ui.set_email_from_address(settings.from_address.clone().into());
            ui.set_email_security_index(settings.security.index());
            ui.set_email_settings_open(true);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let email_settings = email_settings.clone();
        ui.on_apply_email_settings(move || {
            let ui = ui_weak.unwrap();
            let port = ui.get_email_smtp_port().to_string();
            let Ok(port) = port.trim().parse::<u16>() else {
                ui.set_status("Email settings not saved: SMTP port must be a number.".into());
                return;
            };
            let settings = email::EmailSettings {
                smtp_host: ui.get_email_smtp_host().trim().to_string(),
                smtp_port: port,
                smtp_username: ui.get_email_smtp_username().trim().to_string(),
                smtp_password: ui.get_email_smtp_password().to_string(),
                from_address: ui.get_email_from_address().trim().to_string(),
                security: email::EmailSecurity::from_index(ui.get_email_security_index()),
            };
            match email::save_settings(&settings) {
                Ok(path) => {
                    *email_settings.borrow_mut() = settings;
                    ui.set_email_settings_open(false);
                    ui.set_status(format!("Email settings saved to {}", path.display()).into());
                }
                Err(e) => ui.set_status(format!("Email settings save failed: {e}").into()),
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        ui.on_cancel_email_settings(move || {
            ui_weak.unwrap().set_email_settings_open(false);
        });
    }
    {
        let ui_weak = ui.as_weak();
        ui.on_open_email_to_dialog(move || {
            let ui = ui_weak.unwrap();
            ui.set_email_to_address("".into());
            ui.set_email_to_dialog_open(true);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        let email_settings = email_settings.clone();
        ui.on_send_email_to(move || {
            let ui = ui_weak.unwrap();
            let to = ui.get_email_to_address().trim().to_string();
            if to.is_empty() {
                ui.set_status("Email not sent: recipient address is empty.".into());
                return;
            }
            let (name, bytes) = match state.borrow().email_attachment() {
                Ok(file) => file,
                Err(e) => {
                    ui.set_status(format!("Email not sent: {e}").into());
                    return;
                }
            };
            match email::send_data_file(&email_settings.borrow(), &to, &name, bytes) {
                Ok(()) => {
                    ui.set_email_to_dialog_open(false);
                    ui.set_status(format!("Emailed {name} to {to}").into());
                }
                Err(e) => ui.set_status(format!("Email send failed: {e}").into()),
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        ui.on_cancel_email_to(move || {
            ui_weak.unwrap().set_email_to_dialog_open(false);
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
            let result = state.borrow_mut().analyze(None);
            match result {
                Ok(msg) => {
                    // Reveal the fitted NURBS so the user knows it landed and can
                    // adjust the knots for cues we don't model.
                    state.borrow_mut().set_show_warp(true);
                    ui.set_show_warp(true);
                    ui.set_rotation(state.borrow().rotation_deg as f32);
                    ui.set_status(msg.into());
                }
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
            // Checkbox is "Use band tilts" — the inverse of optical-flow mode.
            ui.set_fit_dialog_use_band_tilts(!st.optical_flow);
            ui.set_fit_dialog_use_gelgenie(st.use_gelgenie_ml);
            ui.set_fit_dialog_gelgenie_runtime(st.gelgenie_runtime_index);
            ui.set_fit_dialog_gelgenie_available(gelgenie_model_available());
            ui.set_fit_dialog_extra_edges(st.extra_vertical_edges.to_string().into());
            ui.set_fit_dialog_extra_edges_h(st.extra_horizontal_edges.to_string().into());
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
                .unwrap_or(2);
            let extra_edges_h = ui
                .get_fit_dialog_extra_edges_h()
                .parse::<usize>()
                .unwrap_or(0);
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
                // "Use band tilts" checked ⇒ band-tilt fit ⇒ optical flow off.
                st.optical_flow = !ui.get_fit_dialog_use_band_tilts();
                st.use_gelgenie_ml = ui.get_fit_dialog_use_gelgenie();
                st.gelgenie_runtime_index = ui.get_fit_dialog_gelgenie_runtime().clamp(0, 1);
                st.extra_vertical_edges = extra_edges;
                st.extra_horizontal_edges = extra_edges_h;
                st.warp_regularization = warp_regularization;
                st.row_spacing_weight = row_spacing_weight;
                st.flow_smoothness = flow_smoothness;
            }
            ui.set_fit_dialog_extra_edges(extra_edges.to_string().into());
            ui.set_fit_dialog_gelgenie_runtime(ui.get_fit_dialog_gelgenie_runtime().clamp(0, 1));
            ui.set_fit_dialog_extra_edges_h(extra_edges_h.to_string().into());
            ui.set_fit_dialog_warp_regularization(format!("{warp_regularization:.4}").into());
            ui.set_fit_dialog_row_spacing(format!("{row_spacing_weight:.3}").into());
            ui.set_fit_dialog_flow_smoothness(format!("{flow_smoothness:.3}").into());
            ui.set_fit_dialog_open(false);
            let result = state.borrow_mut().analyze(None);
            match result {
                Ok(msg) => {
                    // Reveal the fitted NURBS so the user knows it landed and can
                    // adjust the knots for cues we don't model.
                    state.borrow_mut().set_show_warp(true);
                    ui.set_show_warp(true);
                    ui.set_rotation(state.borrow().rotation_deg as f32);
                    ui.set_status(msg.into());
                }
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
            let (name, vidx, tidx, vol, conc) =
                state.borrow().ladder_dialog_prefill(lane_id as u32);
            set_ladder_dialog_models(&ui, &state.borrow(), vidx.max(0) as usize);
            ui.set_dialog_lane_name(name.into());
            ui.set_dialog_ladder_vendor_index(vidx.max(0));
            ui.set_dialog_ladder_index(tidx.max(0));
            ui.set_dialog_volume(format!("{vol:.1}").into());
            ui.set_dialog_conc(format!("{conc:.1}").into());
            ui.set_ladder_dialog_lane(lane_id);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_ladder_vendor_changed(move |vendor_index| {
            let ui = ui_weak.unwrap();
            let vendor_index = vendor_index.max(0) as usize;
            set_ladder_dialog_models(&ui, &state.borrow(), vendor_index);
            ui.set_dialog_ladder_vendor_index(vendor_index as i32);
            ui.set_dialog_ladder_index(0);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        let app_config = app_config.clone();
        ui.on_apply_ladder_dialog(move || {
            let ui = ui_weak.unwrap();
            let lane_id = ui.get_ladder_dialog_lane();
            if lane_id >= 0 {
                let vidx = ui.get_dialog_ladder_vendor_index().max(0) as usize;
                let tidx = ui.get_dialog_ladder_index().max(0) as usize;
                let vol: f64 = ui.get_dialog_volume().parse().unwrap_or(0.0);
                let conc: f64 = ui.get_dialog_conc().parse().unwrap_or(0.0);
                let ladder_name = {
                    let (_, names) = state.borrow().ladder_dialog_options_for_vendor_index(vidx);
                    names.get(tidx).cloned()
                };
                let msg = match ladder_name {
                    Some(name) => state.borrow_mut().apply_ladder_dialog_by_name(
                        lane_id as u32,
                        &name,
                        vol,
                        conc,
                    ),
                    None => "No ladder selected.".to_string(),
                };
                {
                    let mut cfg = app_config.borrow_mut();
                    cfg.recent_ladders = state.borrow().recent_ladders.clone();
                    if let Err(e) = config::save_config(&cfg) {
                        ui.set_status(format!("{msg} Recent ladders were not saved: {e}").into());
                        ui.set_ladder_dialog_lane(-1);
                        view::refresh(&ui, &state.borrow());
                        return;
                    }
                }
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
                format!(
                    "OpenGel — capture, detect and quantify gel images. MIT/Apache-2.0. Source: {SOURCE_URL}"
                )
                .into(),
            );
        });
    }
    {
        let ui_weak = ui.as_weak();
        ui.on_how_to_cite(move || {
            let ui = ui_weak.unwrap();
            match webbrowser::open(CITING_URL) {
                Ok(_) => {
                    ui.set_status(format!("Opened citation instructions: {CITING_URL}").into())
                }
                Err(e) => ui.set_status(
                    format!("Could not open browser: {e}. Citation instructions: {CITING_URL}")
                        .into(),
                ),
            }
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
    // Switch which acquisition channel is displayed and analysed.
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_channel_changed(move |idx| {
            let ui = ui_weak.unwrap();
            state.borrow_mut().set_view_channel(idx as usize);
            // A full refresh, not just the image: the frame selector lists this
            // channel's exposures, and the traces are measured from it.
            view::refresh(&ui, &state.borrow());
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
            if let Some(label) = st.hover_size_label(x, y) {
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
    edit_cb!(on_add_lane, |s: &mut AppState, _nx: f64, _ny: f64| s
        .add_lane(None));
    // Add band targets the currently selected lane (not the last image click).
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_add_band(move || {
            let ui = ui_weak.unwrap();
            let msg = state.borrow_mut().add_band_to_selected();
            ui.set_status(msg.into());
            view::refresh(&ui, &state.borrow());
        });
    }
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

    // Gel tree: select a row, and delete the selection.
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_select_tree(move |kind, id| {
            let ui = ui_weak.unwrap();
            state.borrow_mut().select_tree(kind, id as u32);
            view::refresh(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_delete_selected(move || {
            let ui = ui_weak.unwrap();
            let msg = state.borrow_mut().delete_selected();
            ui.set_status(msg.into());
            view::refresh(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_open_weight_dialog(move || {
            let ui = ui_weak.unwrap();
            let (val, unit) = state.borrow().weight_dialog_prefill();
            ui.set_weight_dialog_value(val.into());
            ui.set_weight_dialog_unit(unit.into());
            ui.set_weight_dialog_open(true);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_apply_weight_dialog(move || {
            let ui = ui_weak.unwrap();
            ui.set_weight_dialog_open(false);
            if let Ok(size) = ui.get_weight_dialog_value().parse::<f64>() {
                let msg = state.borrow_mut().set_selected_band_weight(size);
                ui.set_status(msg.into());
                view::refresh(&ui, &state.borrow());
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        ui.on_cancel_weight_dialog(move || {
            ui_weak.unwrap().set_weight_dialog_open(false);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_open_rename_dialog(move || {
            let ui = ui_weak.unwrap();
            ui.set_rename_dialog_value(state.borrow().rename_dialog_prefill().into());
            ui.set_rename_dialog_open(true);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_apply_rename_dialog(move || {
            let ui = ui_weak.unwrap();
            ui.set_rename_dialog_open(false);
            let id = ui.get_selected_lane_id();
            if id >= 0 {
                let name = ui.get_rename_dialog_value().to_string();
                let msg = state.borrow_mut().set_lane_label(id as u32, &name);
                ui.set_status(msg.into());
                view::refresh(&ui, &state.borrow());
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        ui.on_cancel_rename_dialog(move || {
            ui_weak.unwrap().set_rename_dialog_open(false);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_open_add_lane_dialog(move || {
            let ui = ui_weak.unwrap();
            ui.set_add_lane_dialog_value(state.borrow().add_lane_dialog_prefill().into());
            ui.set_add_lane_dialog_open(true);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_apply_add_lane_dialog(move || {
            let ui = ui_weak.unwrap();
            ui.set_add_lane_dialog_open(false);
            // Placement is automatic (left-to-right, no overlap) — the name is
            // all we take from the dialog.
            let name = ui.get_add_lane_dialog_value().to_string();
            let msg = state.borrow_mut().add_lane(Some(name));
            ui.set_status(msg.into());
            view::refresh(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        ui.on_cancel_add_lane_dialog(move || {
            ui_weak.unwrap().set_add_lane_dialog_open(false);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_set_ratio_a(move || {
            let ui = ui_weak.unwrap();
            let msg = state.borrow_mut().set_ratio_a();
            ui.set_status(msg.into());
            view::refresh(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_set_ratio_b(move || {
            let ui = ui_weak.unwrap();
            let msg = state.borrow_mut().set_ratio_b();
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
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_select_all_lanes(move || {
            let ui = ui_weak.unwrap();
            state.borrow_mut().select_all_lanes();
            view::refresh_trace(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_select_none_lanes(move || {
            let ui = ui_weak.unwrap();
            state.borrow_mut().select_no_lanes();
            view::refresh_trace(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_trace_set_zoom(move |z| {
            let ui = ui_weak.unwrap();
            state.borrow_mut().set_trace_zoom(z as f64);
            view::refresh_trace(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_trace_zoom_at(move |factor, focus| {
            let ui = ui_weak.unwrap();
            state
                .borrow_mut()
                .trace_zoom_by(factor as f64, focus as f64);
            view::refresh_trace(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_trace_pan(move |dx| {
            let ui = ui_weak.unwrap();
            state.borrow_mut().trace_pan_by(dx as f64);
            view::refresh_trace(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_trace_reset_view(move || {
            let ui = ui_weak.unwrap();
            state.borrow_mut().trace_reset_view();
            view::refresh_trace(&ui, &state.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_trace_hover(move |f| {
            let ui = ui_weak.unwrap();
            if let Some(label) = state.borrow().trace_hover_bp_label(f as f64) {
                ui.set_status(label.into());
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_export_trace_pdf(move || {
            let ui = ui_weak.unwrap();
            let default_name = state.borrow().trace_pdf_filename();
            if let Some(path) = rfd::FileDialog::new()
                .set_file_name(default_name)
                .add_filter("PDF", &["pdf"])
                .save_file()
            {
                let msg = match state.borrow().export_trace_pdf(&path) {
                    Ok(()) => format!("Exported trace to {}", path.display()),
                    Err(e) => format!("PDF export failed: {e}"),
                };
                ui.set_status(msg.into());
            }
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

    // ---- Gel Doc EZ tab ----
    // Instrument I/O runs on the instrument worker (see the event pump); these
    // callbacks only mutate state and enqueue commands.
    {
        // Protocols are the unit of reproducibility, so every edit is persisted
        // immediately — losing a protocol to a crash would lose the ability to
        // repeat an experiment.
        let persist = {
            let state = state.clone();
            let app_config = app_config.clone();
            Rc::new(move || {
                let mut cfg = app_config.borrow_mut();
                cfg.geldoc_protocols = Some(state.borrow().geldoc.library.clone());
                if let Err(e) = config::save_config(&cfg) {
                    eprintln!("saving protocols failed: {e}");
                }
            })
        };

        {
            let ui_weak = ui.as_weak();
            let state = state.clone();
            ui.on_gd_rescan(move || {
                let ui = ui_weak.unwrap();
                if let Some(inst) = &state.borrow().geldoc.inst {
                    inst.list();
                }
                view::refresh_geldoc(&ui, &state.borrow());
            });
        }
        {
            let state = state.clone();
            ui.on_gd_instrument_selected(move |idx| {
                state.borrow_mut().geldoc.selected_instrument = idx.max(0) as usize;
            });
        }
        {
            let ui_weak = ui.as_weak();
            let state = state.clone();
            ui.on_gd_connect(move || {
                let ui = ui_weak.unwrap();
                {
                    let mut st = state.borrow_mut();
                    st.geldoc.message = "Connecting…".into();
                    let index = st.geldoc.selected_instrument;
                    if let Some(inst) = &st.geldoc.inst {
                        inst.connect(index);
                    }
                }
                view::refresh_geldoc(&ui, &state.borrow());
            });
        }
        {
            let ui_weak = ui.as_weak();
            let state = state.clone();
            ui.on_gd_disconnect(move || {
                let ui = ui_weak.unwrap();
                if let Some(inst) = &state.borrow().geldoc.inst {
                    inst.disconnect();
                }
                view::refresh_geldoc(&ui, &state.borrow());
            });
        }
        {
            let ui_weak = ui.as_weak();
            let state = state.clone();
            ui.on_gd_clear_faults(move || {
                let ui = ui_weak.unwrap();
                if let Some(inst) = &state.borrow().geldoc.inst {
                    inst.clear_faults();
                }
                view::refresh_geldoc(&ui, &state.borrow());
            });
        }
        {
            let state = state.clone();
            ui.on_gd_watch_button_changed(move |watch| {
                let mut st = state.borrow_mut();
                st.geldoc.watch_run_button = watch;
                if let Some(inst) = &st.geldoc.inst {
                    inst.watch_run_button(watch);
                }
            });
        }
        {
            let ui_weak = ui.as_weak();
            let state = state.clone();
            ui.on_gd_protocol_selected(move |idx| {
                let ui = ui_weak.unwrap();
                state.borrow_mut().geldoc.select_protocol(idx.max(0) as usize);
                view::refresh_geldoc(&ui, &state.borrow());
            });
        }
        {
            let ui_weak = ui.as_weak();
            let state = state.clone();
            let persist = persist.clone();
            ui.on_gd_protocol_new(move || {
                let ui = ui_weak.unwrap();
                let name = state.borrow_mut().geldoc.new_protocol();
                persist();
                ui.set_status(format!("Created protocol “{name}”.").into());
                view::refresh_geldoc(&ui, &state.borrow());
            });
        }
        {
            let ui_weak = ui.as_weak();
            let state = state.clone();
            let persist = persist.clone();
            ui.on_gd_protocol_delete(move || {
                let ui = ui_weak.unwrap();
                state.borrow_mut().geldoc.delete_selected_protocol();
                // Force the name field to re-sync with the new selection.
                state.borrow().geldoc.name_field_for.set(usize::MAX);
                persist();
                view::refresh_geldoc(&ui, &state.borrow());
            });
        }
        {
            let ui_weak = ui.as_weak();
            let state = state.clone();
            let persist = persist.clone();
            ui.on_gd_protocol_make_default(move || {
                let ui = ui_weak.unwrap();
                let ok = state.borrow_mut().geldoc.make_selected_default();
                persist();
                ui.set_status(
                    if ok {
                        "This protocol now runs when the instrument's Run button is pressed with \
                         its tray inserted."
                    } else {
                        "This protocol has no tray, so it cannot be a default."
                    }
                    .into(),
                );
                view::refresh_geldoc(&ui, &state.borrow());
            });
        }
        {
            let state = state.clone();
            let persist = persist.clone();
            ui.on_gd_protocol_renamed(move |name| {
                state.borrow_mut().geldoc.rename_selected_protocol(&name);
                // The field already shows what the user typed; keep the view
                // from rewriting it underneath them.
                state
                    .borrow()
                    .geldoc
                    .name_field_for
                    .set(state.borrow().geldoc.selected_protocol);
                persist();
            });
        }
        {
            let ui_weak = ui.as_weak();
            let state = state.clone();
            ui.on_gd_step_selected(move |idx| {
                let ui = ui_weak.unwrap();
                let step = opengel::instrument::protocol::ProtocolStep::ALL
                    [(idx.max(0) as usize).min(3)];
                state.borrow_mut().geldoc.selected_step = step;
                view::refresh_geldoc(&ui, &state.borrow());
            });
        }
        {
            let ui_weak = ui.as_weak();
            let state = state.clone();
            let persist = persist.clone();
            ui.on_gd_step_toggled(move |idx, enabled| {
                let ui = ui_weak.unwrap();
                let step = opengel::instrument::protocol::ProtocolStep::ALL
                    [(idx.max(0) as usize).min(3)];
                state.borrow_mut().geldoc.set_step_enabled(step, enabled);
                persist();
                view::refresh_geldoc(&ui, &state.borrow());
            });
        }
        {
            let ui_weak = ui.as_weak();
            let state = state.clone();
            let persist = persist.clone();
            ui.on_gd_application_selected(move |idx| {
                let ui = ui_weak.unwrap();
                if let Some(app) =
                    opengel::instrument::application::APPLICATIONS.get(idx.max(0) as usize)
                {
                    state.borrow_mut().geldoc.set_application(app.id);
                }
                persist();
                view::refresh_geldoc(&ui, &state.borrow());
            });
        }
        {
            let ui_weak = ui.as_weak();
            let state = state.clone();
            let persist = persist.clone();
            ui.on_gd_exposure_mode_changed(move |idx| {
                use opengel::instrument::protocol::ExposureMode;
                let ui = ui_weak.unwrap();
                let mode = match idx {
                    1 => ExposureMode::AutoFaint,
                    2 => ExposureMode::Manual,
                    _ => ExposureMode::AutoIntense,
                };
                state.borrow_mut().geldoc.set_exposure_mode(mode);
                persist();
                view::refresh_geldoc(&ui, &state.borrow());
            });
        }
        {
            let ui_weak = ui.as_weak();
            let state = state.clone();
            let persist = persist.clone();
            ui.on_gd_exposure_changed(move |f| {
                let ui = ui_weak.unwrap();
                let seconds = geldoc::GelDocState::exposure_from_slider(f);
                state.borrow_mut().geldoc.set_manual_exposure_s(seconds);
                persist();
                view::refresh_geldoc(&ui, &state.borrow());
            });
        }
        {
            let state = state.clone();
            let persist = persist.clone();
            ui.on_gd_activation_changed(move |text| {
                // Ignore unparseable input rather than resetting the field: the
                // user is mid-edit, and "4" on the way to "45" is not an error.
                if let Ok(seconds) = text.trim().parse::<f64>() {
                    state.borrow_mut().geldoc.set_activation_s(seconds);
                    persist();
                }
            });
        }
        {
            let state = state.clone();
            let persist = persist.clone();
            ui.on_gd_highlight_saturated_changed(move |on| {
                if let Some(p) = state.borrow_mut().geldoc.protocol_mut() {
                    p.highlight_saturated = on;
                }
                persist();
            });
        }
        {
            let ui_weak = ui.as_weak();
            let state = state.clone();
            ui.on_gd_run(move || {
                let ui = ui_weak.unwrap();
                let msg = state.borrow_mut().geldoc_run();
                ui.set_status(msg.into());
                view::refresh_geldoc(&ui, &state.borrow());
            });
        }
        {
            let ui_weak = ui.as_weak();
            let state = state.clone();
            ui.on_gd_abort(move || {
                let ui = ui_weak.unwrap();
                {
                    let mut st = state.borrow_mut();
                    st.cancel_capture();
                    st.geldoc.abort_run("Run aborted.");
                }
                ui.set_status("Run aborted; the lamps were switched off.".into());
                view::refresh_geldoc(&ui, &state.borrow());
            });
        }
        // --- simulated-instrument bench controls ---
        {
            let state = state.clone();
            ui.on_gd_sim_tray(move |idx| {
                use opengel::instrument::TrayType;
                let tray = match idx {
                    1 => Some(TrayType::Uv),
                    2 => Some(TrayType::White),
                    3 => Some(TrayType::Blue),
                    4 => Some(TrayType::StainFree),
                    _ => None,
                };
                if let Some(inst) = &state.borrow().geldoc.inst {
                    inst.sim_set_tray(tray);
                }
            });
        }
        {
            let state = state.clone();
            ui.on_gd_sim_door(move |closed| {
                if let Some(inst) = &state.borrow().geldoc.inst {
                    inst.sim_set_door(closed);
                }
            });
        }
        {
            let state = state.clone();
            ui.on_gd_sim_fault(move |bit, on| {
                let mut st = state.borrow_mut();
                let mask = 1u16 << bit.clamp(0, 15);
                let faults = if on {
                    st.geldoc.faults.0 | mask
                } else {
                    st.geldoc.faults.0 & !mask
                };
                st.geldoc.faults = opengel::instrument::Faults(faults);
                if let Some(inst) = &st.geldoc.inst {
                    inst.sim_set_faults(faults);
                }
            });
        }
        {
            let state = state.clone();
            ui.on_gd_sim_press_button(move || {
                if let Some(inst) = &state.borrow().geldoc.inst {
                    inst.sim_press_button();
                }
            });
        }
    }

    ui.run()?;
    Ok(())
}
