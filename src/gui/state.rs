//! Application state and the operations behind each UI callback.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use opengel::core::model::{
    Analysis, Band, Calibration, CaptureMeta, GelType, LadderAssignment, LadderTemplate, Lane,
    Quantification, TargetKind,
};
use opengel::core::quant::{compare, mass_ng_to_nmol, nmol_to_molar};
use opengel::core::warp::GelWarp;
use opengel::core::{ladders, GelDocument, GrayF32};
use opengel::detect::detector::DetectParams;
use opengel::detect::ladder_match::best_template;

use crate::camera_worker::CameraHandle;

/// What the lane trace plots on its y-axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceMode {
    Intensity,
    Ng,
    Molarity,
}

impl TraceMode {
    pub fn from_index(i: usize) -> Self {
        match i {
            1 => TraceMode::Ng,
            2 => TraceMode::Molarity,
            _ => TraceMode::Intensity,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            TraceMode::Intensity => "intensity",
            TraceMode::Ng => "ng",
            TraceMode::Molarity => "molarity (nmol)",
        }
    }
}

/// The currently selected annotation (for highlight + drag).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    Lane(u32),
    Band(u32),
}

/// A flattened row of the lane/band tree (see [`AppState::tree_rows`]).
pub struct TreeRow {
    /// 0 = lane header, 1 = band child.
    pub kind: i32,
    pub lane_id: u32,
    pub band_id: i32,
    pub expanded: bool,
    pub is_ladder: bool,
    pub name: String,
    pub rf: String,
    pub size: String,
    pub density: String,
    pub ng: String,
    pub nmol: String,
}

/// One lane's densitometry trace (values along migration, top→bottom).
pub struct LaneTrace {
    pub label: String,
    pub ladder: bool,
    pub values: Vec<f64>,
}

pub struct AppState {
    pub doc: Option<GelDocument>,
    pub work: Option<GrayF32>,
    pub gel_type: GelType,
    pub source_path: Option<PathBuf>,
    /// Rotation correction applied before analysis/display (degrees).
    pub rotation_deg: f64,
    /// Display window (contrast/brightness) as fractions of the current image's
    /// `[min, max]` range: pixels at or below `disp_lo` render black, at or
    /// above `disp_hi` render white.
    pub disp_lo: f32,
    pub disp_hi: f32,
    /// Invert the displayed image (dark-on-light ↔ light-on-dark).
    pub invert: bool,
    /// Show the dewarped (rectified) gel: a fixed, straightened view.
    pub show_unwarped: bool,
    /// Fit the gel warp by optical flow (band twist) when running detection.
    pub optical_flow: bool,
    /// Optical-flow smoothness for model fitting.
    pub flow_smoothness: f64,
    /// Extra v-axis control rows beyond observed ladder/front rows.
    pub extra_vertical_edges: usize,
    /// NURBS control-point pull toward the prior grid during refinement.
    pub warp_regularization: f64,
    /// Weight for keeping adjacent v control rows uniformly spaced.
    pub row_spacing_weight: f64,
    /// Overlay the fitted NURBS warp grid on the image.
    pub show_warp: bool,
    /// When dragging a top/bottom warp knot, redistribute inner v knots in that
    /// column so migration spacing stays uniform.
    pub normalize_inner_knots: bool,
    /// Highlight over-exposed (clipped-high) pixels in red.
    pub show_overexposed: bool,
    /// Opacity of the lane/band annotation overlay (0 = hidden, 1 = solid).
    pub annotation_alpha: f32,
    /// Last hover position over the gel viewport, normalized `[0,1]`; used to
    /// draw the migration-alignment line. `< 0` = no hover.
    pub hover_x: f32,
    pub hover_y: f32,
    /// Which captured frame to display: `None` = the merged HDR working image,
    /// `Some(i)` = raw frame `i`. Analysis always uses the merged image.
    pub view_frame: Option<usize>,
    /// Trace-tab state: y-axis mode and which lanes are plotted.
    pub trace_mode: TraceMode,
    pub selected_lanes: std::collections::BTreeSet<u32>,
    /// Per-ladder-lane loaded **volume** (µL) and **concentration** (ng/µL). The
    /// loaded mass used for calibration is `volume × concentration`. Missing
    /// entries fall back to the defaults below.
    pub ladder_volume: std::collections::BTreeMap<u32, f64>,
    pub ladder_conc: std::collections::BTreeMap<u32, f64>,
    /// Lanes collapsed in the tree list (absent = expanded).
    pub collapsed_lanes: std::collections::BTreeSet<u32>,
    /// The selected annotation (highlighted; draggable).
    pub selected: Option<Selection>,
    /// True while a drag is in progress on the selected annotation.
    pub dragging: bool,
    /// Manual edit of the warp control lattice, paired with the `doc_gen` it was
    /// made against so it auto-invalidates when the document/analysis changes.
    pub warp_edit: Option<(u64, opengel::core::warp::GelWarp)>,
    /// The warp control point `(iu, iv)` currently being dragged, if any.
    pub dragging_knot: Option<(usize, usize)>,
    /// Bumped whenever the document/working image changes; invalidates the
    /// view's cached base image + histogram.
    pub doc_gen: u64,
    bracket_counter: u32,

    // ---- Live capture ----
    /// Handle to the camera worker thread (all camera I/O runs off the UI
    /// thread). `None` in headless/CLI contexts that never start the worker.
    pub cam: Option<CameraHandle>,
    /// Available camera names (from the worker) and the selected index.
    pub cameras: Vec<String>,
    pub selected_camera: usize,
    pub camera_name: String,
    pub live_running: bool,
    /// Whether the open camera supports manual exposure. When false the exposure
    /// slider and HDR capture are disabled (HDR needs varying exposure).
    pub exposure_supported: bool,
    /// True while a capture is in flight (drives the modal progress dialog).
    pub capturing: bool,
    /// Human-readable progress line for the capture dialog.
    pub capture_status: String,
    /// Lower / upper exposure time (seconds) of the HDR bracket.
    pub hdr_min_s: f64,
    pub hdr_max_s: f64,
    /// Number of exposures in the HDR bracket. `1` = a single (auto) frame.
    pub hdr_steps: usize,
    /// Current exposure (seconds): drives the live preview and single capture,
    /// and is the value fed to the "Set lower/upper HDR time" buttons.
    pub live_exposure_s: f64,
    /// Most recent live preview frame.
    pub preview: Option<GrayF32>,
}

/// Exposure-time slider range (seconds), log-mapped. Covers sub-millisecond to
/// multi-second, spanning the useful range for dim fluorescence to bright fields.
pub const EXPOSURE_MIN_S: f64 = 0.001;
pub const EXPOSURE_MAX_S: f64 = 4.0;
/// Step-count options offered in the HDR "Steps" dropdown (`1` = single/auto).
pub const HDR_STEP_OPTIONS: [usize; 5] = [1, 2, 3, 5, 7];

/// Default ladder load assumed when a ladder lane has no explicit value:
/// 10 µL × 50 ng/µL = 500 ng.
pub const DEFAULT_LADDER_VOLUME_UL: f64 = 10.0;
pub const DEFAULT_LADDER_CONC_NG_UL: f64 = 50.0;

impl AppState {
    pub fn new() -> Self {
        AppState {
            doc: None,
            work: None,
            gel_type: GelType::Dna,
            source_path: None,
            rotation_deg: 0.0,
            disp_lo: 0.0,
            disp_hi: 1.0,
            invert: false,
            show_unwarped: false,
            optical_flow: false,
            flow_smoothness: 8.0,
            extra_vertical_edges: 2,
            warp_regularization: 1e-2,
            row_spacing_weight: 10.0,
            show_warp: false,
            normalize_inner_knots: true,
            show_overexposed: false,
            annotation_alpha: 0.5,
            hover_x: -1.0,
            hover_y: -1.0,
            view_frame: None,
            trace_mode: TraceMode::Intensity,
            selected_lanes: std::collections::BTreeSet::new(),
            ladder_volume: std::collections::BTreeMap::new(),
            ladder_conc: std::collections::BTreeMap::new(),
            collapsed_lanes: std::collections::BTreeSet::new(),
            selected: None,
            dragging: false,
            warp_edit: None,
            dragging_knot: None,
            doc_gen: 0,
            bracket_counter: 0,
            cam: None,
            cameras: Vec::new(),
            selected_camera: 0,
            camera_name: "—".to_string(),
            live_running: false,
            exposure_supported: true,
            capturing: false,
            capture_status: String::new(),
            hdr_min_s: 0.01,
            hdr_max_s: 1.0,
            hdr_steps: 3,
            live_exposure_s: 0.1,
            preview: None,
        }
    }

    /// Loaded volume (µL) for a ladder lane.
    pub fn ladder_volume(&self, lane_id: u32) -> f64 {
        self.ladder_volume
            .get(&lane_id)
            .copied()
            .unwrap_or(DEFAULT_LADDER_VOLUME_UL)
    }

    /// Loaded concentration (ng/µL) for a ladder lane.
    pub fn ladder_conc(&self, lane_id: u32) -> f64 {
        self.ladder_conc
            .get(&lane_id)
            .copied()
            .unwrap_or(DEFAULT_LADDER_CONC_NG_UL)
    }

    /// The loaded ladder mass (ng) for a lane = volume × concentration.
    pub fn ladder_load(&self, lane_id: u32) -> f64 {
        self.ladder_volume(lane_id) * self.ladder_conc(lane_id)
    }

    /// Set a lane's loaded volume + concentration.
    pub fn set_ladder_amounts(&mut self, lane_id: u32, volume_ul: f64, conc_ng_ul: f64) {
        self.ladder_volume.insert(lane_id, volume_ul);
        self.ladder_conc.insert(lane_id, conc_ng_ul);
    }

    // ---- lane/band tree list ----

    /// Flatten the analysis into tree rows: each lane, followed by its bands
    /// (top→bottom) when expanded.
    pub fn tree_rows(&self) -> Vec<TreeRow> {
        let unit = self.gel_type.size_unit();
        let Some(a) = self.analysis() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for lane in &a.lanes {
            let expanded = !self.collapsed_lanes.contains(&lane.id);
            let name = lane
                .label
                .clone()
                .unwrap_or_else(|| format!("Lane {}", lane.id));
            out.push(TreeRow {
                kind: 0,
                lane_id: lane.id,
                band_id: -1,
                expanded,
                is_ladder: lane.is_ladder,
                name,
                rf: String::new(),
                size: String::new(),
                density: String::new(),
                ng: String::new(),
                nmol: String::new(),
            });
            if !expanded {
                continue;
            }
            let mut bands: Vec<&Band> = a.bands.iter().filter(|b| b.lane_id == lane.id).collect();
            bands.sort_by(|x, y| x.v_center.partial_cmp(&y.v_center).unwrap());
            for b in bands {
                let quant = a.quantifications.iter().find(|q| q.target_id == b.id);
                let ng = quant
                    .and_then(|q| q.mass_ng)
                    .map(|m| format!("{m:.1}"))
                    .unwrap_or_else(|| "-".into());
                let nmol = quant
                    .and_then(|q| q.molarity_nmol)
                    .map(|m| format!("{m:.3}"))
                    .unwrap_or_else(|| "-".into());
                let name = match b.known_size {
                    Some(s) => format!("{s:.0} {unit}"),
                    None => "band".to_string(),
                };
                out.push(TreeRow {
                    kind: 1,
                    lane_id: lane.id,
                    band_id: b.id as i32,
                    expanded: false,
                    is_ladder: lane.is_ladder,
                    name,
                    rf: format!("{:.2}", b.rf()),
                    size: b
                        .size
                        .map(|s| format!("{s:.0} {unit}"))
                        .unwrap_or_else(|| "-".into()),
                    density: format!("{:.1}", b.integrated_density),
                    ng,
                    nmol,
                });
            }
        }
        out
    }

    /// Expand/collapse a lane in the tree.
    pub fn toggle_expanded(&mut self, lane_id: u32) {
        if !self.collapsed_lanes.remove(&lane_id) {
            self.collapsed_lanes.insert(lane_id);
        }
    }

    /// Rename a lane (empty string clears the custom label).
    pub fn set_lane_label(&mut self, lane_id: u32, name: &str) -> String {
        let Some(doc) = self.doc.as_mut() else {
            return "No document.".into();
        };
        if let Some(lane) = doc
            .project
            .analysis
            .lanes
            .iter_mut()
            .find(|l| l.id == lane_id)
        {
            lane.label = if name.trim().is_empty() {
                None
            } else {
                Some(name.trim().to_string())
            };
            format!("Renamed lane {lane_id}.")
        } else {
            "No such lane.".into()
        }
    }

    /// Delete a lane and everything attached to it.
    pub fn delete_lane(&mut self, lane_id: u32) -> String {
        let Some(doc) = self.doc.as_mut() else {
            return "No document.".into();
        };
        let a = &mut doc.project.analysis;
        let before = a.lanes.len();
        a.lanes.retain(|l| l.id != lane_id);
        a.bands.retain(|b| b.lane_id != lane_id);
        a.ladder_assignments.retain(|la| la.lane_id != lane_id);
        self.ladder_volume.remove(&lane_id);
        self.ladder_conc.remove(&lane_id);
        self.selected_lanes.remove(&lane_id);
        if a.lanes.len() < before {
            format!("Deleted lane {lane_id}.")
        } else {
            "No such lane.".into()
        }
    }

    /// Mark or unmark a lane as a ladder. Unmarking clears its assignment and
    /// the known sizes on its bands.
    pub fn set_lane_is_ladder(&mut self, lane_id: u32, on: bool) -> String {
        let Some(doc) = self.doc.as_mut() else {
            return "No document.".into();
        };
        let a = &mut doc.project.analysis;
        let Some(lane) = a.lanes.iter_mut().find(|l| l.id == lane_id) else {
            return "No such lane.".into();
        };
        lane.is_ladder = on;
        if !on {
            a.ladder_assignments.retain(|la| la.lane_id != lane_id);
            for b in a.bands.iter_mut().filter(|b| b.lane_id == lane_id) {
                b.known_size = None;
            }
            resize_sample_lanes(a);
        }
        format!("Lane {lane_id} ladder = {on}.")
    }

    /// Re-apply the ladder assigned to a lane (re-match rungs and re-size all
    /// sample lanes). Used by the tree's "Set" action.
    pub fn reapply_lane_ladder(&mut self, lane_id: u32) -> String {
        let name = match self.analysis() {
            Some(a) => a
                .ladder_assignments
                .iter()
                .find(|la| la.lane_id == lane_id)
                .map(|la| la.template_name.clone()),
            None => None,
        };
        let Some(name) = name else {
            return "Assign a ladder to this lane first.".into();
        };
        let idx = self.ladder_names().iter().position(|n| *n == name);
        match idx {
            Some(i) => self.set_lane_ladder(lane_id, i),
            None => format!("Ladder {name} not available for this gel type."),
        }
    }

    // ---- display: frame selection, contrast window, histogram ----

    /// Labels for the frame selector: index 0 is the merged HDR image, then one
    /// entry per captured frame with its exposure time.
    pub fn frame_labels(&self) -> Vec<String> {
        let mut out = vec!["HDR (merged)".to_string()];
        if let Some(doc) = self.doc.as_ref() {
            for (i, img) in doc.project.images.iter().enumerate() {
                let t = img.meta.exposure_seconds;
                if t > 0.0 {
                    out.push(format!("Frame {i} — {t:.3} s"));
                } else {
                    out.push(format!("Frame {i}"));
                }
            }
        }
        out
    }

    /// Set the displayed frame from a selector index (0 = merged HDR).
    pub fn set_view_frame(&mut self, sel: usize) {
        self.view_frame = if sel == 0 { None } else { Some(sel - 1) };
    }

    /// The selector index matching the current `view_frame`.
    pub fn view_frame_index(&self) -> usize {
        self.view_frame.map_or(0, |i| i + 1)
    }

    /// The grayscale image currently shown (selected raw frame, or the merged
    /// working image). Shares dimensions with [`Self::view_image`] so overlays
    /// stay aligned.
    pub fn display_gray(&self) -> Option<GrayF32> {
        match self.view_frame {
            Some(i) => self
                .doc
                .as_ref()
                .and_then(|d| d.frames.get(i))
                .map(GrayF32::from_dynamic),
            None => self.work.clone(),
        }
    }

    /// A histogram (counts per bin) of the currently displayed image over its
    /// own `[min, max]` range. Used to draw the contrast control.
    pub fn histogram(&self, bins: usize) -> Vec<u32> {
        let bins = bins.max(1);
        let mut h = vec![0u32; bins];
        let Some(img) = self.display_gray() else {
            return h;
        };
        let (lo, hi) = img.min_max();
        let span = (hi - lo).max(1e-9);
        for &v in img.data.iter() {
            let f = ((v - lo) / span).clamp(0.0, 0.999_999);
            h[(f * bins as f32) as usize] += 1;
        }
        h
    }

    pub fn set_display_window(&mut self, lo: f32, hi: f32) {
        // Keep at least a sliver of range so the image never collapses.
        self.disp_lo = lo.clamp(0.0, 1.0);
        self.disp_hi = hi.clamp(0.0, 1.0);
        if self.disp_hi <= self.disp_lo {
            self.disp_hi = (self.disp_lo + 0.02).min(1.0);
        }
    }

    pub fn set_invert(&mut self, on: bool) {
        self.invert = on;
    }

    pub fn set_show_unwarped(&mut self, on: bool) {
        self.show_unwarped = on;
    }

    /// The dewarped (rectified) working image: resample through the fitted warp
    /// so lanes are vertical and bands horizontal. `None` when no image.
    pub fn unwarped_view(&self) -> Option<GrayF32> {
        let work = self.work.as_ref()?;
        let warp = self
            .analysis()
            .and_then(|a| a.warp.clone())
            .unwrap_or_else(|| GelWarp::identity(work.width() as u32, work.height() as u32));
        Some(warp.rectify(work, work.width(), work.height()))
    }

    pub fn set_show_warp(&mut self, on: bool) {
        self.show_warp = on;
    }

    pub fn set_normalize_inner_knots(&mut self, on: bool) {
        self.normalize_inner_knots = on;
    }

    pub fn set_show_overexposed(&mut self, on: bool) {
        self.show_overexposed = on;
    }

    pub fn set_annotation_alpha(&mut self, a: f32) {
        self.annotation_alpha = a.clamp(0.0, 1.0);
    }

    pub fn set_hover(&mut self, x: f32, y: f32) {
        self.hover_x = x;
        self.hover_y = y;
    }

    /// Fit a NURBS [`GelWarp`](opengel::core::warp::GelWarp) to the current analysis:
    /// anchors come from every band (u = lane order across the gel, v = its Rf),
    /// mapped to the band's image position. Regularized so it is well-posed even
    /// with a single lane. Returns `None` if there is nothing to fit.
    pub fn fit_warp(&self) -> Option<opengel::core::warp::GelWarp> {
        // A manual knot edit (still matching the current document) wins over the
        // auto-fit, so dragged adjustments persist across re-renders.
        if let Some((gen, w)) = &self.warp_edit {
            if *gen == self.doc_gen {
                return Some(w.clone());
            }
        }
        self.auto_fit_warp()
    }

    /// The warp used for the grid overlay: the analysis pipeline already fits and
    /// stores the gel warp, so return that (or the identity when none has been
    /// computed yet).
    fn auto_fit_warp(&self) -> Option<opengel::core::warp::GelWarp> {
        let a = self.analysis()?;
        let img = self.work.as_ref()?;
        Some(
            a.warp
                .clone()
                .unwrap_or_else(|| GelWarp::identity(img.width() as u32, img.height() as u32)),
        )
    }

    /// Control points of the effective warp, as normalized `[0,1]²` positions
    /// (for drawing draggable knot handles). Empty when the grid is hidden or no
    /// warp exists.
    pub fn warp_knots(&self) -> Vec<(f32, f32)> {
        if !self.show_warp {
            return Vec::new();
        }
        let (Some(warp), Some(img)) = (self.fit_warp(), self.work.as_ref()) else {
            return Vec::new();
        };
        let (w, h) = (img.width() as f64, img.height() as f64);
        let (nu, nv) = warp.grid_size();
        let mut out = Vec::with_capacity(nu * nv);
        for iv in 0..nv {
            for iu in 0..nu {
                let (cx, cy) = warp.control_point(iu, iv);
                out.push(((cx / w.max(1.0)) as f32, (cy / h.max(1.0)) as f32));
            }
        }
        out
    }

    /// Try to grab a warp control point near normalized `(nx, ny)` — only when
    /// the warp grid is visible. Seeds the editable warp from the current fit on
    /// the first grab. Returns true if a knot was grabbed (so it can be dragged).
    pub fn press_warp_knot(&mut self, nx: f64, ny: f64) -> bool {
        if !self.show_warp {
            return false;
        }
        let Some((w, h)) = self
            .work
            .as_ref()
            .map(|i| (i.width() as f64, i.height() as f64))
        else {
            return false;
        };
        let Some(warp) = self.fit_warp() else {
            return false;
        };
        let (nu, nv) = warp.grid_size();
        let mut best: Option<((usize, usize), f64)> = None;
        for iv in 0..nv {
            for iu in 0..nu {
                let (cx, cy) = warp.control_point(iu, iv);
                let dx = nx - cx / w.max(1.0);
                let dy = ny - cy / h.max(1.0);
                let d2 = dx * dx + dy * dy;
                if best.is_none_or(|(_, bd)| d2 < bd) {
                    best = Some(((iu, iv), d2));
                }
            }
        }
        // Grab radius as a fraction of the image (knot handles are ~this size).
        const GRAB_R: f64 = 0.035;
        if let Some((k, d2)) = best {
            if d2 <= GRAB_R * GRAB_R {
                self.warp_edit = Some((self.doc_gen, warp));
                self.dragging_knot = Some(k);
                return true;
            }
        }
        false
    }

    /// Move the grabbed knot to normalized `(nx, ny)`.
    pub fn drag_warp_knot(&mut self, nx: f64, ny: f64) {
        let Some((iu, iv)) = self.dragging_knot else {
            return;
        };
        let Some((w, h)) = self
            .work
            .as_ref()
            .map(|i| (i.width() as f64, i.height() as f64))
        else {
            return;
        };
        if let Some((gen, warp)) = self.warp_edit.as_mut() {
            if *gen == self.doc_gen {
                warp.set_control_point(iu, iv, nx.clamp(0.0, 1.0) * w, ny.clamp(0.0, 1.0) * h);
                let (_, nv) = warp.grid_size();
                if self.normalize_inner_knots && nv > 2 && (iv == 0 || iv + 1 == nv) {
                    let (top_x, top_y) = warp.control_point(iu, 0);
                    let (bottom_x, bottom_y) = warp.control_point(iu, nv - 1);
                    for inner_iv in 1..(nv - 1) {
                        let t = inner_iv as f64 / (nv - 1) as f64;
                        warp.set_control_point(
                            iu,
                            inner_iv,
                            top_x + (bottom_x - top_x) * t,
                            top_y + (bottom_y - top_y) * t,
                        );
                    }
                }
            }
        }
    }

    /// End a knot drag.
    pub fn release_warp_knot(&mut self) {
        self.dragging_knot = None;
    }

    pub fn is_dragging_knot(&self) -> bool {
        self.dragging_knot.is_some()
    }

    /// Reset the contrast window to full range.
    pub fn reset_display_window(&mut self) {
        self.disp_lo = 0.0;
        self.disp_hi = 1.0;
    }

    // ---- Live capture ----

    /// Map a log-scale slider position `[0,1]` to an exposure time (seconds)
    /// across [`EXPOSURE_MIN_S`, `EXPOSURE_MAX_S`], and back. Exposure spans
    /// several decades, so a linear slider would be unusable at the low end.
    pub fn exposure_from_slider(f: f32) -> f64 {
        let f = f.clamp(0.0, 1.0) as f64;
        EXPOSURE_MIN_S * (EXPOSURE_MAX_S / EXPOSURE_MIN_S).powf(f)
    }
    pub fn slider_from_exposure(t: f64) -> f32 {
        let t = t.clamp(EXPOSURE_MIN_S, EXPOSURE_MAX_S);
        ((t / EXPOSURE_MIN_S).ln() / (EXPOSURE_MAX_S / EXPOSURE_MIN_S).ln()) as f32
    }

    /// Set the current exposure (drives preview + single capture) from a
    /// log-scale slider position, and apply it to the camera worker.
    pub fn set_live_exposure_slider(&mut self, f: f32) {
        self.live_exposure_s = Self::exposure_from_slider(f);
        if let Some(cam) = &self.cam {
            cam.set_exposure(self.live_exposure_s);
        }
    }

    /// Slider position for the current exposure (to initialize the UI).
    pub fn live_exposure_slider(&self) -> f32 {
        Self::slider_from_exposure(self.live_exposure_s)
    }

    /// Adopt the current exposure as the lower / upper HDR bound.
    pub fn set_hdr_lower_from_current(&mut self) {
        self.hdr_min_s = self.live_exposure_s.min(self.hdr_max_s);
    }
    pub fn set_hdr_upper_from_current(&mut self) {
        self.hdr_max_s = self.live_exposure_s.max(self.hdr_min_s);
    }

    /// Number of HDR steps from a dropdown index into [`HDR_STEP_OPTIONS`].
    pub fn set_hdr_steps_idx(&mut self, idx: usize) {
        self.hdr_steps = HDR_STEP_OPTIONS.get(idx).copied().unwrap_or(3);
    }
    pub fn hdr_steps_idx(&self) -> usize {
        HDR_STEP_OPTIONS
            .iter()
            .position(|&n| n == self.hdr_steps)
            .unwrap_or(2)
    }

    /// The HDR bracket's exposure times (seconds), geometric (log-even) between
    /// the min and max bounds. `steps == 1` yields a single frame — the caller
    /// treats that as a non-HDR auto/single capture.
    pub fn hdr_exposures(&self) -> Vec<f64> {
        let n = self.hdr_steps.max(1);
        let lo = self.hdr_min_s.max(1e-4);
        let hi = self.hdr_max_s.max(lo);
        if n == 1 {
            return vec![self.live_exposure_s.max(1e-4)];
        }
        (0..n)
            .map(|i| {
                let f = i as f64 / (n - 1) as f64;
                lo * (hi / lo).powf(f)
            })
            .collect()
    }

    /// Dynamic range covered by the current bracket, in EV (stops). Shown in the
    /// UI so the user can judge whether the step count fits the range.
    pub fn hdr_range_ev(&self) -> f64 {
        (self.hdr_max_s.max(1e-9) / self.hdr_min_s.max(1e-9)).log2()
    }

    /// Set the list of available cameras (reported by the worker).
    pub fn set_cameras(&mut self, names: Vec<String>) {
        if self.selected_camera >= names.len() {
            self.selected_camera = 0;
        }
        self.cameras = names;
    }

    /// Ask the worker to (re)enumerate cameras.
    pub fn refresh_cameras(&self) {
        if let Some(cam) = &self.cam {
            cam.list_cameras();
        }
    }

    /// Select a camera by index; reopens it (keeping exposure + preview state).
    pub fn select_camera(&mut self, idx: usize) {
        self.selected_camera = idx;
        if let Some(cam) = &self.cam {
            cam.open(idx);
            cam.set_exposure(self.live_exposure_s.max(1e-4));
            if self.live_running {
                cam.start_preview();
            }
        }
    }

    /// Open the selected camera and begin live preview (on the worker thread).
    pub fn live_start(&mut self) -> String {
        self.live_running = true;
        if let Some(cam) = &self.cam {
            cam.open(self.selected_camera);
            cam.set_exposure(self.live_exposure_s.max(1e-4));
            cam.start_preview();
            "Starting live preview…".into()
        } else {
            self.live_running = false;
            "Camera unavailable.".into()
        }
    }

    pub fn live_stop(&mut self) {
        self.live_running = false;
        if let Some(cam) = &self.cam {
            cam.stop_preview();
        }
    }

    pub fn preview_image(&self) -> Option<&GrayF32> {
        self.preview.as_ref()
    }

    /// Histogram (counts per bin) of the live preview frame, for the exposure
    /// aid beneath the preview image. Empty if there is no preview yet.
    pub fn preview_histogram(&self, bins: usize) -> Vec<u32> {
        let bins = bins.max(1);
        let mut h = vec![0u32; bins];
        let Some(img) = self.preview.as_ref() else {
            return h;
        };
        for &v in img.data.iter() {
            let f = v.clamp(0.0, 0.999_999);
            h[(f * bins as f32) as usize] += 1;
        }
        h
    }

    /// Make a captured set of frames the current gel document. Called from the
    /// UI event loop when the worker reports a finished capture.
    pub fn adopt_capture(&mut self, imgs: Vec<image::DynamicImage>, metas: Vec<CaptureMeta>) {
        let doc = GelDocument::from_frames(self.gel_type, imgs, metas);
        self.work = doc.working_image();
        self.doc = Some(doc);
        self.source_path = None;
        self.view_frame = None;
        self.reset_display_window();
        self.clear_selection();
        self.doc_gen = self.doc_gen.wrapping_add(1);
    }

    /// Start an HDR capture on the worker (non-blocking). The result arrives via
    /// a [`CamEvent`] and is applied by the UI event loop. `steps == 1` is a
    /// single (auto) frame, which the document builder leaves un-merged.
    pub fn live_capture(&mut self) {
        let group = self.bracket_counter;
        self.bracket_counter += 1;
        let exposures = self.hdr_exposures();
        self.capturing = true;
        self.capture_status = if exposures.len() == 1 {
            "Capturing…".into()
        } else {
            format!("Capturing HDR bracket (0/{})…", exposures.len())
        };
        if let Some(cam) = &self.cam {
            cam.capture_hdr(exposures, group);
        }
    }

    /// Start a non-HDR single capture on the worker (non-blocking).
    pub fn capture_single(&mut self) {
        let expo = self.live_exposure_s.max(1e-4);
        self.capturing = true;
        self.capture_status = "Capturing single frame…".into();
        if let Some(cam) = &self.cam {
            cam.capture_single(expo);
        }
    }

    /// Request cancellation of an in-progress capture.
    pub fn cancel_capture(&mut self) {
        if let Some(cam) = &self.cam {
            cam.cancel();
        }
        self.capture_status = "Cancelling…".into();
    }

    // ---- Trace tab ----

    pub fn set_trace_mode(&mut self, idx: usize) {
        self.trace_mode = TraceMode::from_index(idx);
    }

    /// Toggle whether a lane is included in the trace plot.
    pub fn toggle_lane(&mut self, id: u32) {
        if !self.selected_lanes.remove(&id) {
            self.selected_lanes.insert(id);
        }
    }

    /// Lanes for the checklist: `(id, label, is_ladder, selected)`.
    pub fn lane_items(&self) -> Vec<(u32, String, bool, bool)> {
        let Some(a) = self.analysis() else {
            return Vec::new();
        };
        a.lanes
            .iter()
            .map(|l| {
                let label = l.label.clone().unwrap_or_else(|| format!("Lane {}", l.id));
                (
                    l.id,
                    label,
                    l.is_ladder,
                    self.selected_lanes.contains(&l.id),
                )
            })
            .collect()
    }

    /// Semi-log sizing model from the identified ladder lane, if any.
    fn sizing_fit(&self) -> Option<opengel::core::quant::SizingFit> {
        let a = self.analysis()?;
        let assign = a.ladder_assignments.first()?;
        let pts: Vec<(f64, f64)> = a
            .bands
            .iter()
            .filter(|b| b.lane_id == assign.lane_id)
            .filter_map(|b| b.known_size.map(|s| (b.v_center, s)))
            .collect();
        opengel::core::quant::SizingFit::fit(&pts)
    }

    /// A human-readable fragment-size estimate at a normalized vertical hover
    /// position (e.g. `"≈ 1230 bp"`), from the identified ladder's semi-log
    /// sizing model. Returns `None` unless a ladder has been fitted (so a
    /// sizing grid exists) and the position is over the image.
    pub fn hover_size_label(&self, ny: f32) -> Option<String> {
        if !(0.0..=1.0).contains(&ny) {
            return None;
        }
        let fit = self.sizing_fit()?;
        // Migration coordinate v ≡ the normalized hover position.
        let size = fit.size_at(ny as f64);
        if !size.is_finite() || size <= 0.0 {
            return None;
        }
        // Round to a sensible precision for a live readout (no false digits).
        let rounded = if size >= 1000.0 {
            (size / 10.0).round() * 10.0
        } else {
            size.round()
        };
        Some(format!("≈ {rounded:.0} {}", self.gel_type.size_unit()))
    }

    /// Calibration slope (ng per integrated-density unit), or 1.0 if none.
    fn cal_slope(&self) -> f64 {
        match self.analysis().and_then(|a| a.calibration.clone()) {
            Some(Calibration::Linear { slope }) => slope,
            Some(Calibration::Affine { a, .. }) => a,
            _ => 1.0,
        }
    }

    /// Compute the densitometry trace for each selected lane, scaled per the
    /// current [`TraceMode`]. Intensity is the background-subtracted lane
    /// profile; ng multiplies by the calibration slope; molarity converts each
    /// row's ng to nmol using the ladder's size-at-position.
    pub fn compute_traces(&self) -> Vec<LaneTrace> {
        use opengel::detect::classical::lane_row_profile;
        use opengel::detect::signal::subtract_baseline;

        let (Some(a), Some(img)) = (self.analysis(), self.work.as_ref()) else {
            return Vec::new();
        };
        let slope = self.cal_slope();
        let fit = self.sizing_fit();
        let (w, h) = (img.width(), img.height());
        let warp = a.warp_or_identity(w as u32, h as u32);
        let rect = warp.rectify(img, w, h);
        let h = h as f64;
        let w = w as f64;
        let mut out = Vec::new();
        for lane in &a.lanes {
            if !self.selected_lanes.contains(&lane.id) {
                continue;
            }
            let x0 = (lane.u_min * w).clamp(0.0, w - 1.0) as usize;
            let x1 = ((lane.u_max * w).ceil() as usize).clamp(x0 + 1, w as usize);
            let inten = subtract_baseline(&lane_row_profile(&rect, x0, x1), 25);
            let values: Vec<f64> = inten
                .iter()
                .enumerate()
                .map(|(y, &v)| match self.trace_mode {
                    TraceMode::Intensity => v,
                    TraceMode::Ng => v * slope,
                    TraceMode::Molarity => {
                        let ng = v * slope;
                        match fit.map(|f| f.size_at(y as f64 / h)) {
                            Some(size) if size > 0.0 => {
                                mass_ng_to_nmol(ng, size, self.gel_type).unwrap_or(0.0)
                            }
                            _ => ng,
                        }
                    }
                })
                .collect();
            out.push(LaneTrace {
                label: lane
                    .label
                    .clone()
                    .unwrap_or_else(|| format!("Lane {}", lane.id)),
                ladder: lane.is_ladder,
                values,
            });
        }
        out
    }

    /// The raw working image, used for display and region measurement. Display
    /// rotation is applied live by the UI (Slint), so it is NOT baked here —
    /// annotations stay in raw image coordinates and rotate with the view.
    pub fn view_image(&self) -> Option<GrayF32> {
        self.work.clone()
    }

    /// The image used for detection (rotation baked in). Detection is currently
    /// deferred, but this keeps the straighten path ready for a plugged-in
    /// algorithm.
    pub fn display_image(&self) -> Option<GrayF32> {
        self.work.as_ref().map(|w| {
            if self.rotation_deg.abs() < 1e-3 {
                w.clone()
            } else {
                w.rotated(self.rotation_deg)
            }
        })
    }

    /// Measure every annotated band region from the image: integrate the
    /// background-subtracted lane densitometry trace over each band's y-extent.
    /// This is the core gel-region measurement, independent of how the regions
    /// were produced (demo, manual editing, or a future detector).
    pub fn measure_regions(&mut self) -> String {
        use opengel::detect::classical::lane_row_profile;
        use opengel::detect::signal::subtract_baseline;
        use std::collections::HashMap;

        let Some(img) = self.work.clone() else {
            return "No image.".into();
        };
        let Some(doc) = self.doc.as_mut() else {
            return "No annotation.".into();
        };
        let a = &mut doc.project.analysis;
        if a.lanes.is_empty() {
            return "No regions to measure — add lanes/bands or use Demo annotation.".into();
        }
        // Background-subtracted densitometry trace per lane.
        let (w, h) = (img.width(), img.height());
        let warp = a.warp_or_identity(w as u32, h as u32);
        let rect = warp.rectify(&img, w, h);
        let h = h as f64;
        let w = w as f64;
        let mut prof: HashMap<u32, Vec<f64>> = HashMap::new();
        for lane in &a.lanes {
            let x0 = (lane.u_min * w).clamp(0.0, w - 1.0) as usize;
            let x1 = ((lane.u_max * w).ceil() as usize).clamp(x0 + 1, w as usize);
            let raw = lane_row_profile(&rect, x0, x1);
            prof.insert(lane.id, subtract_baseline(&raw, 25));
        }
        let mut n = 0;
        for b in &mut a.bands {
            if let Some(p) = prof.get(&b.lane_id) {
                let y0 = ((b.v_center - b.v_half_width) * h).max(0.0) as usize;
                let y1 = (((b.v_center + b.v_half_width) * h) as usize + 1).min(p.len());
                if y0 < y1 {
                    b.integrated_density = p[y0..y1].iter().sum();
                    n += 1;
                }
            }
        }
        format!("Measured {n} region(s) by densitometry.")
    }

    pub fn set_rotation(&mut self, deg: f64) {
        self.rotation_deg = deg;
    }

    // ---- selection + drag of annotations ----

    pub fn selected_lane_id(&self) -> Option<u32> {
        match self.selected {
            Some(Selection::Lane(id)) => Some(id),
            _ => None,
        }
    }
    pub fn selected_band_id(&self) -> Option<u32> {
        match self.selected {
            Some(Selection::Band(id)) => Some(id),
            _ => None,
        }
    }

    /// Hit-test at image-normalized `(nx, ny)`: select the band under the
    /// pointer if any, else the lane column. Returns `true` when something
    /// draggable was selected (so the UI enters drag mode).
    pub fn press_annotation(&mut self, nx: f64, ny: f64) -> bool {
        let (Some(a), Some(img)) = (self.analysis(), self.work.as_ref()) else {
            self.selected = None;
            self.dragging = false;
            return false;
        };
        let (w, h) = (img.width() as f64, img.height() as f64);
        let (px, py) = (nx.clamp(0.0, 1.0) * w, ny.clamp(0.0, 1.0) * h);
        let warp = a.warp_or_identity(w as u32, h as u32);
        // Lane whose x-range contains px (else nearest by center).
        let lane_at = |x: f64| -> Option<u32> {
            a.lanes
                .iter()
                .find(|l| {
                    let (x0, x1) = l.px_x_bounds(&warp);
                    x >= x0 as f64 && x <= x1 as f64
                })
                .map(|l| l.id)
        };
        // Band under the pointer: inside its lane's x-range and near its center.
        let mut best_band: Option<(u32, f64)> = None;
        for b in &a.bands {
            let Some(lane) = a.lanes.iter().find(|l| l.id == b.lane_id) else {
                continue;
            };
            let (x0, x1) = lane.px_x_bounds(&warp);
            if px < x0 as f64 || px > x1 as f64 {
                continue;
            }
            let dy = (py - b.v_center * h).abs();
            let tol = (b.v_half_width * h * 2.0).max(8.0);
            if dy <= tol && best_band.is_none_or(|(_, bd)| dy < bd) {
                best_band = Some((b.id, dy));
            }
        }
        if let Some((id, _)) = best_band {
            self.selected = Some(Selection::Band(id));
            self.dragging = true;
            true
        } else if let Some(id) = lane_at(px) {
            self.selected = Some(Selection::Lane(id));
            self.dragging = true;
            true
        } else {
            self.selected = None;
            self.dragging = false;
            false
        }
    }

    /// Drag the selected annotation to image-normalized `(nx, ny)`. Lanes move
    /// horizontally (keeping width); bands move to that y and re-home to the
    /// lane under the pointer.
    pub fn drag_selection(&mut self, nx: f64, ny: f64) {
        if !self.dragging {
            return;
        }
        let sel = self.selected;
        let Some(img) = self.work.clone() else { return };
        let (w, h) = (img.width() as f64, img.height() as f64);
        let (px, py) = (nx.clamp(0.0, 1.0) * w, ny.clamp(0.0, 1.0) * h);
        let Some(doc) = self.doc.as_mut() else { return };
        let a = &mut doc.project.analysis;
        let warp = a.warp_or_identity(w as u32, h as u32);
        match sel {
            Some(Selection::Lane(id)) => {
                if let Some(lane) = a.lanes.iter_mut().find(|l| l.id == id) {
                    // Move the lane's cross-lane center to the pointer, keeping
                    // its u-width. u ← image-x via the warp inverse.
                    let half_u = (lane.u_max - lane.u_min) / 2.0;
                    let (uc, _) = warp.invert(px, 0.0);
                    let uc = uc.clamp(half_u, 1.0 - half_u);
                    lane.u_min = uc - half_u;
                    lane.u_max = uc + half_u;
                }
            }
            Some(Selection::Band(id)) => {
                // Which lane is the pointer over (for re-homing)?
                let target_lane = a
                    .lanes
                    .iter()
                    .find(|l| {
                        let (x0, x1) = l.px_x_bounds(&warp);
                        px >= x0 as f64 && px <= x1 as f64
                    })
                    .map(|l| l.id);
                if let Some(b) = a.bands.iter_mut().find(|b| b.id == id) {
                    b.v_center = (py / h).clamp(0.0, 1.0);
                    if let Some(lid) = target_lane {
                        b.lane_id = lid;
                    }
                }
            }
            None => {}
        }
    }

    /// End a drag; re-measure so densities reflect the new positions.
    pub fn release_selection(&mut self) {
        if self.dragging {
            self.dragging = false;
            self.measure_regions();
        }
    }

    pub fn clear_selection(&mut self) {
        self.selected = None;
        self.dragging = false;
    }

    /// Estimate the gel's skew and set the rotation to straighten it.
    pub fn auto_straighten(&mut self) -> String {
        let Some(w) = self.work.as_ref() else {
            return "No image loaded.".into();
        };
        let est = opengel::detect::orient::estimate_rotation(w, 50.0, true);
        self.rotation_deg = -est;
        format!("Auto-straighten applied {:.1}°.", self.rotation_deg)
    }

    /// Ladder template names applicable to the current gel type.
    pub fn ladder_names(&self) -> Vec<String> {
        ladders::for_gel_type(self.gel_type)
            .iter()
            .map(|t| t.name.clone())
            .collect()
    }

    // ---- ladder lanes (any number, individually tunable) ----

    /// Prefill values for the "Use as ladder" dialog for a lane:
    /// `(lane_name, template_index, volume_ul, conc_ng_ul)`. `template_index`
    /// is the lane's currently assigned template (or 0 if none).
    pub fn ladder_dialog_prefill(&self, lane_id: u32) -> (String, i32, f64, f64) {
        let names = self.ladder_names();
        let name = self
            .analysis()
            .and_then(|a| a.lanes.iter().find(|l| l.id == lane_id))
            .and_then(|l| l.label.clone())
            .unwrap_or_else(|| format!("Lane {lane_id}"));
        let tidx = self
            .analysis()
            .and_then(|a| a.ladder_assignments.iter().find(|la| la.lane_id == lane_id))
            .and_then(|la| names.iter().position(|n| *n == la.template_name))
            .map(|p| p as i32)
            .unwrap_or(0);
        (
            name,
            tidx,
            self.ladder_volume(lane_id),
            self.ladder_conc(lane_id),
        )
    }

    /// Apply the "Use as ladder" dialog: store the lane's volume + concentration
    /// and assign the chosen ladder template (which marks it as a ladder).
    pub fn apply_ladder_dialog(
        &mut self,
        lane_id: u32,
        template_idx: usize,
        volume_ul: f64,
        conc_ng_ul: f64,
    ) -> String {
        self.set_ladder_amounts(lane_id, volume_ul, conc_ng_ul);
        let msg = self.set_lane_ladder(lane_id, template_idx);
        let load = self.ladder_load(lane_id);
        format!("{msg} Load {load:.0} ng ({volume_ul:.1} µL × {conc_ng_ul:.1} ng/µL).")
    }

    /// Delete a single band by id.
    pub fn delete_band_by_id(&mut self, band_id: u32) -> String {
        let Some(doc) = self.doc.as_mut() else {
            return "No document.".into();
        };
        let a = &mut doc.project.analysis;
        let before = a.bands.len();
        a.bands.retain(|b| b.id != band_id);
        a.quantifications.retain(|q| q.target_id != band_id);
        for la in &mut a.ladder_assignments {
            for slot in &mut la.rung_to_band {
                if *slot == Some(band_id) {
                    *slot = None;
                }
            }
        }
        if a.bands.len() < before {
            format!("Deleted band {band_id}.")
        } else {
            "No such band.".into()
        }
    }

    /// Assign the ladder template at `template_idx` to a specific ladder lane:
    /// match its bands to the template's rungs, record the assignment, and
    /// re-derive sizes for the sample lanes. Marks the lane as a ladder.
    pub fn set_lane_ladder(&mut self, lane_id: u32, template_idx: usize) -> String {
        let names = self.ladder_names();
        let Some(name) = names.get(template_idx).cloned() else {
            return "No ladder selected.".into();
        };
        let Some(template) = ladders::by_name(&name) else {
            return format!("Unknown ladder {name}.");
        };
        let Some(doc) = self.doc.as_mut() else {
            return "No document.".into();
        };
        let a = &mut doc.project.analysis;
        if let Some(lane) = a.lanes.iter_mut().find(|l| l.id == lane_id) {
            lane.is_ladder = true;
        } else {
            return "No such lane.".into();
        }
        match apply_ladder_to_lane(a, lane_id, template) {
            Some(n) => {
                resize_sample_lanes(a);
                format!("Lane {lane_id} = {name} ({n} rungs matched).")
            }
            None => format!("Lane {lane_id}: could not match {name} to its bands."),
        }
    }

    /// Replace the current project with the synthetic demo gel (8 lanes, three
    /// NEB 1 kb ladders) with its aligned annotation, then measure the bands.
    pub fn load_demo(&mut self) -> String {
        let doc = opengel::core::demo::demo_document_annotated();
        self.gel_type = doc.project.gel_type;
        self.work = doc.working_image();
        self.doc = Some(doc);
        self.source_path = None;
        self.view_frame = None;
        self.reset_display_window();
        self.clear_selection();
        self.doc_gen = self.doc_gen.wrapping_add(1);
        let msg = self.measure_regions();
        format!("Loaded demo gel (8 lanes, 3 ladders). {msg}")
    }

    pub fn open_path(&mut self, path: &Path) -> Result<()> {
        let doc = GelDocument::load(path).with_context(|| format!("loading {}", path.display()))?;
        self.gel_type = doc.project.gel_type;
        self.work = doc.working_image();
        self.doc = Some(doc);
        self.source_path = Some(path.to_path_buf());
        self.view_frame = None;
        self.reset_display_window();
        self.clear_selection();
        self.doc_gen = self.doc_gen.wrapping_add(1);
        Ok(())
    }

    pub fn save_path(&self, path: &Path) -> Result<()> {
        let doc = self
            .doc
            .as_ref()
            .ok_or_else(|| anyhow!("nothing to save"))?;
        doc.save(path)?;
        Ok(())
    }

    pub fn analysis(&self) -> Option<&Analysis> {
        self.doc.as_ref().map(|d| &d.project.analysis)
    }

    pub fn capture(&mut self) -> Result<String> {
        let group = self.bracket_counter;
        self.bracket_counter += 1;
        let (source, frames) = crate::camera_glue::capture_bracket_frames(group)?;
        let n = frames.len();
        let (imgs, metas): (Vec<_>, Vec<_>) = frames.into_iter().unzip();
        let doc = GelDocument::from_frames(self.gel_type, imgs, metas);
        self.work = doc.working_image();
        self.doc = Some(doc);
        self.source_path = None;
        self.view_frame = None;
        self.reset_display_window();
        self.clear_selection();
        self.doc_gen = self.doc_gen.wrapping_add(1);
        Ok(format!("Captured {n}-frame HDR bracket from {source}."))
    }

    /// Run detection + ladder ID + sizing. If `force_template` is set, only that
    /// template is considered (min_r2 relaxed so the user's choice wins).
    pub fn analyze(&mut self, force_template: Option<&str>) -> Result<String> {
        let work = self
            .display_image()
            .ok_or_else(|| anyhow!("no image loaded"))?;
        let work = &work;
        // Honor the "Optical-flow dewarp" toggle: without it the warp comes from
        // ladder-rung smile fitting (needs a matched ladder across lanes), which
        // falls back to a near-identity coarse warp when no smile is recovered.
        let params = DetectParams {
            optical_flow_warp: self.optical_flow,
            flow_smoothness: self.flow_smoothness.max(0.0),
            extra_vertical_edges: self.extra_vertical_edges.max(2),
            warp_regularization: self.warp_regularization.max(0.0),
            row_spacing_weight: self.row_spacing_weight.max(0.0),
            ..DetectParams::default()
        };

        let (candidates, min_r2): (Vec<&opengel::core::LadderTemplate>, f64) = match force_template
        {
            Some(name) => {
                let t = ladders::by_name(name).ok_or_else(|| anyhow!("unknown ladder {name}"))?;
                (vec![t], 0.0)
            }
            None => (Vec::new(), 0.9),
        };
        let analysis = opengel::detect::analyze(work, self.gel_type, &params, &candidates, min_r2);
        let n_lanes = analysis.lanes.len();
        let n_bands = analysis.bands.len();
        let ladder = analysis
            .ladder_assignments
            .first()
            .map(|a| format!(", ladder: {} (lane {})", a.template_name, a.lane_id))
            .unwrap_or_default();

        let doc = self.doc.as_mut().ok_or_else(|| anyhow!("no document"))?;
        doc.project.analysis = analysis;
        Ok(format!(
            "Detected {n_lanes} lanes, {n_bands} bands{ladder}."
        ))
    }

    // ---- interactive editing (coordinates are normalized [0,1] over the
    // displayed/rotated image) ----

    fn with_analysis_mut<F: FnOnce(&mut Analysis, &GrayF32) -> String>(&mut self, f: F) -> String {
        let Some(img) = self.display_image() else {
            return "No image loaded.".into();
        };
        let Some(doc) = self.doc.as_mut() else {
            return "No document.".into();
        };
        f(&mut doc.project.analysis, &img)
    }

    pub fn add_lane_at(&mut self, nx: f64) -> String {
        self.with_analysis_mut(|a, img| {
            let (w, h) = (img.width() as u32, img.height() as u32);
            let warp = a.warp_or_identity(w, h);
            let cx = nx.clamp(0.0, 1.0) * w as f64;
            let half = (0.04 * w as f64).max(4.0);
            let (u_lo, _) = warp.invert((cx - half).max(0.0), 0.0);
            let (u_hi, _) = warp.invert((cx + half).min(w as f64), 0.0);
            let id = a.lanes.iter().map(|l| l.id).max().map_or(0, |m| m + 1);
            a.lanes.push(Lane {
                id,
                u_min: u_lo.min(u_hi),
                u_max: u_lo.max(u_hi),
                label: None,
                is_ladder: false,
            });
            format!("Added lane {id}.")
        })
    }

    pub fn add_band_at(&mut self, nx: f64, ny: f64) -> String {
        self.with_analysis_mut(|a, img| {
            let (w, h) = (img.width() as u32, img.height() as u32);
            let warp = a.warp_or_identity(w, h);
            let x = nx.clamp(0.0, 1.0) * w as f64;
            let yc = ny.clamp(0.0, 1.0) * h as f64;
            let Some(pos) = nearest_lane(a, &warp, x) else {
                return "Add a lane first.".into();
            };
            let lane = &a.lanes[pos];
            let half = 5.0;
            let (x0, x1) = lane.px_x_bounds(&warp);
            let density = window_density(img, x0, x1, yc, half);
            let id = a.bands.iter().map(|b| b.id).max().map_or(0, |m| m + 1);
            let lane_id = lane.id;
            a.bands.push(Band {
                id,
                lane_id,
                v_center: (yc / h as f64).clamp(0.0, 1.0),
                v_half_width: half / h as f64,
                integrated_density: density,
                size: None,
                known_size: None,
            });
            format!("Added band {id} to lane {lane_id}.")
        })
    }

    /// Fit an intensity→mass calibration from *every* ladder lane, each using
    /// its own loaded mass (ng), then quantify every band (ng + nmol).
    ///
    /// For each ladder lane, its load is distributed across its rungs in
    /// proportion to fragment size; the resulting (density, mass) points from
    /// all ladder lanes are pooled into a single linear fit. This supports any
    /// number of ladders — including different ladders at different loads.
    pub fn calibrate(&mut self, volume_ul: f64) -> String {
        let gel_type = self.gel_type;
        // Snapshot per-lane loads so the closure can borrow `a` mutably.
        let loads: std::collections::BTreeMap<u32, f64> = self
            .analysis()
            .map(|a| {
                a.ladder_assignments
                    .iter()
                    .map(|la| (la.lane_id, self.ladder_load(la.lane_id)))
                    .collect()
            })
            .unwrap_or_default();

        self.with_analysis_mut(move |a, _img| {
            if a.ladder_assignments.is_empty() {
                return "Assign a ladder to at least one lane first.".into();
            }
            // Pool (density, mass) points across all ladder lanes.
            let mut points: Vec<(f64, f64)> = Vec::new();
            let mut lanes_used = 0;
            for assign in &a.ladder_assignments {
                let load = loads
                    .get(&assign.lane_id)
                    .copied()
                    .unwrap_or(DEFAULT_LADDER_VOLUME_UL * DEFAULT_LADDER_CONC_NG_UL);
                let bands: Vec<(f64, f64)> = a
                    .bands
                    .iter()
                    .filter(|b| b.lane_id == assign.lane_id && b.known_size.is_some())
                    .map(|b| (b.integrated_density, b.known_size.unwrap()))
                    .collect();
                if bands.len() < 2 {
                    continue;
                }
                let size_sum: f64 = bands.iter().map(|(_, s)| s).sum();
                if size_sum <= 0.0 {
                    continue;
                }
                for (d, s) in &bands {
                    points.push((*d, load * s / size_sum));
                }
                lanes_used += 1;
            }
            if points.len() < 2 {
                return "Not enough ladder bands to calibrate.".into();
            }
            let Some(cal) = Calibration::fit_linear(&points) else {
                return "Calibration fit failed.".into();
            };

            // Quantify every band.
            let mut quants = Vec::new();
            for b in &a.bands {
                let mass = cal.mass_ng(b.integrated_density);
                let nmol = b.size.and_then(|s| mass_ng_to_nmol(mass, s, gel_type));
                let _molar = nmol.and_then(|n| nmol_to_molar(n, volume_ul));
                quants.push(Quantification {
                    target_id: b.id,
                    target_kind: TargetKind::Band,
                    mass_ng: Some(mass),
                    molarity_nmol: nmol,
                    size: b.size,
                });
            }
            a.calibration = Some(cal);
            a.quantifications = quants;
            format!(
                "Calibrated from {} ladder lane(s), {} bands.",
                lanes_used,
                points.len()
            )
        })
    }

    /// Compare the first two detected bands (relative mass & molarity). This is
    /// a minimal demonstration of the two-blob comparison; interactive
    /// two-band selection is a UI refinement.
    pub fn compare_first_two(&self, volume_ul: f64) -> String {
        let Some(a) = self.analysis() else {
            return "Analyze first.".into();
        };
        if a.bands.len() < 2 {
            return "Need at least two bands to compare.".into();
        }
        let b0 = &a.bands[0];
        let b1 = &a.bands[1];
        let Some(rel) = compare(
            b0.integrated_density,
            b0.size,
            b1.integrated_density,
            b1.size,
        ) else {
            return "Comparison unavailable.".into();
        };
        let molar = rel
            .molar_ratio
            .map(|m| format!(", molar ratio {m:.3}"))
            .unwrap_or_default();
        let _ = (volume_ul, mass_ng_to_nmol(0.0, 1.0, self.gel_type)); // reserved for absolute mode
        format!(
            "Band 0 vs 1: density ratio {:.3}{molar} (sizes {} / {} {})",
            rel.density_ratio,
            fmt_size(b0.size),
            fmt_size(b1.size),
            self.gel_type.size_unit(),
        )
    }
}

fn fmt_size(s: Option<f64>) -> String {
    match s {
        Some(v) => format!("{v:.0}"),
        None => "?".into(),
    }
}

/// Match `template` against a lane's bands (top→bottom), assign `known_size`/
/// `size` to the matched bands, and record/replace the [`LadderAssignment`].
/// Returns the number of rungs matched, or `None` if no acceptable match.
fn apply_ladder_to_lane(
    a: &mut Analysis,
    lane_id: u32,
    template: &LadderTemplate,
) -> Option<usize> {
    // Band indices for this lane, in detection (y-ascending) order.
    let idxs: Vec<usize> = a
        .bands
        .iter()
        .enumerate()
        .filter(|(_, b)| b.lane_id == lane_id)
        .map(|(i, _)| i)
        .collect();
    let positions: Vec<f64> = idxs.iter().map(|&i| a.bands[i].v_center).collect();
    // The user picked this template explicitly, so accept any fit (min_r2 = 0).
    let m = best_template(&positions, std::iter::once(template), 0.0)?;

    // Clear previous known sizes on this lane, then apply the new mapping.
    for &i in &idxs {
        a.bands[i].known_size = None;
    }
    let mut rung_to_band = Vec::with_capacity(m.pairs.len());
    for pair in &m.pairs {
        if let Some(&bi) = idxs.get(pair.band_index) {
            a.bands[bi].known_size = Some(pair.size);
            a.bands[bi].size = Some(pair.size);
            rung_to_band.push(Some(a.bands[bi].id));
        } else {
            rung_to_band.push(None);
        }
    }
    a.ladder_assignments.retain(|la| la.lane_id != lane_id);
    a.ladder_assignments.push(LadderAssignment {
        lane_id,
        template_name: template.name.clone(),
        rung_to_band,
    });
    Some(m.pairs.len())
}

/// Re-size every non-ladder band from the first ladder assignment's semi-log fit.
fn resize_sample_lanes(a: &mut Analysis) {
    let Some(assign) = a.ladder_assignments.first().cloned() else {
        return;
    };
    let pts: Vec<(f64, f64)> = a
        .bands
        .iter()
        .filter(|b| b.lane_id == assign.lane_id)
        .filter_map(|b| b.known_size.map(|s| (b.v_center, s)))
        .collect();
    let Some(fit) = opengel::core::quant::SizingFit::fit(&pts) else {
        return;
    };
    let ladder_ids: std::collections::BTreeSet<u32> = a
        .lanes
        .iter()
        .filter(|l| l.is_ladder)
        .map(|l| l.id)
        .collect();
    for b in &mut a.bands {
        if !ladder_ids.contains(&b.lane_id) {
            b.size = Some(fit.size_at(b.v_center));
        }
    }
}

/// Index of the lane whose pixel x-center is nearest `x`.
fn nearest_lane(a: &Analysis, warp: &GelWarp, x: f64) -> Option<usize> {
    a.lanes
        .iter()
        .enumerate()
        .min_by(|(_, l), (_, m)| {
            let dl = (l.px_x_center(warp) - x).abs();
            let dm = (m.px_x_center(warp) - x).abs();
            dl.partial_cmp(&dm).unwrap()
        })
        .map(|(i, _)| i)
}

/// Sum intensity over `[x0,x1) × [yc-half, yc+half)`.
fn window_density(img: &GrayF32, x0: usize, x1: usize, yc: f64, half: f64) -> f64 {
    let y0 = (yc - half).max(0.0) as usize;
    let y1 = ((yc + half) as usize).min(img.height());
    let x1 = x1.min(img.width());
    let mut s = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            s += img.get(x, y) as f64;
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_analyze_compare_headless() {
        // Uses the mock camera backend — no display or hardware required.
        let mut st = AppState::new();
        let msg = st.capture().expect("capture");
        assert!(msg.contains("bracket"));
        assert!(st.work.is_some());

        let msg = st.analyze(None).expect("analyze");
        assert!(msg.contains("lanes"));
        let a = st.analysis().unwrap();
        assert!(!a.bands.is_empty());
        // The mock ladder lane reproduces the NEB 1 kb ladder, so it should be
        // identified against the built-in database.
        assert!(a
            .ladder_assignments
            .iter()
            .any(|x| x.template_name.contains("1 kb")));

        let cmp = st.compare_first_two(10.0);
        assert!(cmp.contains("ratio"));
    }

    #[test]
    fn edit_add_delete_and_calibrate() {
        let mut st = AppState::new();
        st.capture().unwrap();
        st.analyze(None).unwrap();
        let before = st.analysis().unwrap().bands.len();

        // Add a lane then a band in it.
        st.add_lane_at(0.5);
        let lanes_after = st.analysis().unwrap().lanes.len();
        let new_lane_id = st
            .analysis()
            .unwrap()
            .lanes
            .iter()
            .map(|l| l.id)
            .max()
            .unwrap();
        st.add_band_at(0.5, 0.5);
        assert!(st.analysis().unwrap().bands.len() > before);

        // Delete the band we just added back out (by id).
        let new_band_id = st
            .analysis()
            .unwrap()
            .bands
            .iter()
            .map(|b| b.id)
            .max()
            .unwrap();
        st.delete_band_by_id(new_band_id);

        // Mark as ladder then delete the lane operate without panicking.
        st.set_lane_is_ladder(new_lane_id, true);
        st.delete_lane(new_lane_id);
        assert!(st.analysis().unwrap().lanes.len() < lanes_after);

        // Absolute calibration from the identified ladder (default 500 ng load).
        let msg = st.calibrate(10.0);
        assert!(msg.contains("Calibrated"), "got: {msg}");
        let a = st.analysis().unwrap();
        assert!(a.calibration.is_some());
        assert!(a
            .quantifications
            .iter()
            .any(|q| q.mass_ng.unwrap_or(0.0) > 0.0));
        // Sized DNA bands should also get a molarity.
        assert!(a.quantifications.iter().any(|q| q.molarity_nmol.is_some()));
    }

    #[test]
    fn rotation_and_straighten() {
        let mut st = AppState::new();
        st.capture().unwrap();
        st.set_rotation(12.0);
        assert!(st.display_image().is_some());
        let msg = st.auto_straighten();
        assert!(msg.contains("Auto-straighten"));
        // Mock capture is upright, so straightening should settle near 0°.
        assert!(st.rotation_deg.abs() < 6.0, "rotation {}", st.rotation_deg);
    }

    #[test]
    fn demo_loads_and_measures_regions() {
        // load_demo builds the 8-lane / 3-ladder demo with an aligned annotation
        // and measures each band region from the pixels.
        let mut st = AppState::new();
        let msg = st.load_demo();
        assert!(msg.contains("Loaded demo gel"), "got: {msg}");
        let a = st.analysis().unwrap();
        assert_eq!(a.lanes.len(), 8);
        assert_eq!(a.lanes.iter().filter(|l| l.is_ladder).count(), 3);
        assert!(!a.bands.is_empty());
        // Measurement produced positive integrated densities for bands sitting
        // on the demo gel's bright bands.
        assert!(
            a.bands
                .iter()
                .filter(|b| b.integrated_density > 0.0)
                .count()
                >= 3,
            "expected several measured regions"
        );

        // Re-measuring is idempotent-ish (still positive).
        let msg2 = st.measure_regions();
        assert!(msg2.contains("Measured"), "got: {msg2}");

        // No ladder identified yet → no size readout on hover.
        assert!(st.hover_size_label(0.5).is_none());

        // Identify one of the demo's ladder lanes, which fits the semi-log
        // sizing model.
        let ladder_lane = st
            .analysis()
            .unwrap()
            .lanes
            .iter()
            .find(|l| l.is_ladder)
            .unwrap()
            .id;
        st.set_lane_ladder(ladder_lane, 0);

        // With a fitted ladder, hovering yields a size readout that decreases
        // as the cursor moves down the gel (larger fragments migrate less).
        let top = st.hover_size_label(0.2).expect("bp readout near top");
        let bot = st.hover_size_label(0.8).expect("bp readout near bottom");
        assert!(top.contains("bp"), "got: {top}");
        let num = |s: &str| -> f64 {
            s.trim_start_matches("≈ ")
                .split_whitespace()
                .next()
                .unwrap()
                .parse()
                .unwrap()
        };
        assert!(
            num(&top) > num(&bot),
            "top {top} should be larger than bottom {bot}"
        );
        // Off-image positions produce no readout.
        assert!(st.hover_size_label(-0.1).is_none());
        assert!(st.hover_size_label(1.5).is_none());
    }

    #[test]
    fn hdr_exposures_are_geometric() {
        let mut st = AppState::new();
        st.hdr_min_s = 0.01;
        st.hdr_max_s = 1.0;
        st.set_hdr_steps_idx(2); // → 3 steps
        let e = st.hdr_exposures();
        assert_eq!(e.len(), 3);
        assert!((e[0] - 0.01).abs() < 1e-9);
        assert!((e[1] - 0.1).abs() < 1e-9, "mid = {}", e[1]); // geometric mean
        assert!((e[2] - 1.0).abs() < 1e-9);
        // Log-even: constant ratio between consecutive exposures.
        assert!(((e[1] / e[0]) - (e[2] / e[1])).abs() < 1e-9);

        // 1 step = a single frame (auto/non-HDR) at the current exposure.
        st.set_hdr_steps_idx(0); // → 1
        st.live_exposure_s = 0.25;
        let e1 = st.hdr_exposures();
        assert_eq!(e1, vec![0.25]);

        // Covered dynamic range in EV.
        assert!((st.hdr_range_ev() - 100.0_f64.log2()).abs() < 1e-9);
    }

    #[test]
    fn exposure_slider_round_trips_and_hdr_bounds() {
        // Log slider maps monotonically and inverts.
        for &f in &[0.0f32, 0.25, 0.5, 0.75, 1.0] {
            let t = AppState::exposure_from_slider(f);
            let back = AppState::slider_from_exposure(t);
            assert!((back - f).abs() < 1e-4, "f={f} back={back}");
        }
        // Adopting the current exposure as the HDR bounds.
        let mut st = AppState::new();
        st.live_exposure_s = 0.05;
        st.set_hdr_lower_from_current();
        assert!((st.hdr_min_s - 0.05).abs() < 1e-9);
        st.live_exposure_s = 1.5;
        st.set_hdr_upper_from_current();
        assert!((st.hdr_max_s - 1.5).abs() < 1e-9);
        // Bounds never cross.
        assert!(st.hdr_min_s <= st.hdr_max_s);
    }

    #[test]
    fn warp_knots_only_when_shown_and_edit_persists() {
        let mut st = AppState::new();
        st.load_demo();

        // Hidden grid → no knots and nothing grabbable.
        assert!(st.warp_knots().is_empty());
        let some_pos = (0.5, 0.5);
        assert!(!st.press_warp_knot(some_pos.0, some_pos.1));

        // Shown grid → control-point handles appear.
        st.set_show_warp(true);
        let knots = st.warp_knots();
        assert!(!knots.is_empty(), "expected warp control points");

        // Grab the first knot (iu=0, iv=0), drag it, and release.
        let (nx, ny) = (knots[0].0 as f64, knots[0].1 as f64);
        assert!(
            st.press_warp_knot(nx, ny),
            "should grab the knot under the cursor"
        );
        assert!(st.is_dragging_knot());
        let target = (0.30, 0.40);
        st.drag_warp_knot(target.0, target.1);
        st.release_warp_knot();
        assert!(!st.is_dragging_knot());

        // The edit moved that knot and persists across fit_warp() calls
        // (it does not revert to the auto-fit).
        let moved = st.warp_knots()[0];
        assert!(
            (moved.0 as f64 - target.0).abs() < 5e-3 && (moved.1 as f64 - target.1).abs() < 5e-3,
            "knot at {moved:?} not at target {target:?}"
        );
        let a = st.warp_knots();
        let b = st.warp_knots();
        assert_eq!(a, b, "edited warp must be stable across renders");

        // On a taller grid, dragging a top/bottom edge knot redistributes that
        // column's inner v rows linearly between the two edge knots.
        let img = st.work.as_ref().unwrap();
        let (w, h) = (img.width() as f64, img.height() as f64);
        st.warp_edit = Some((st.doc_gen, GelWarp::identity_grid(w, h, 4, 5)));
        st.normalize_inner_knots = true;
        let knots = st.warp_knots();
        assert!(st.press_warp_knot(knots[0].0 as f64, knots[0].1 as f64));
        let target = (0.12, 0.18);
        st.drag_warp_knot(target.0, target.1);
        st.release_warp_knot();
        let warp = st.fit_warp().unwrap();
        let (nu, nv) = warp.grid_size();
        let top = warp.control_point(0, 0);
        let bottom = warp.control_point(0, nv - 1);
        for iv in 1..(nv - 1) {
            let t = iv as f64 / (nv - 1) as f64;
            let p = warp.control_point(0, iv);
            assert!(
                (p.0 - (top.0 + (bottom.0 - top.0) * t)).abs() < 1e-6
                    && (p.1 - (top.1 + (bottom.1 - top.1) * t)).abs() < 1e-6,
                "inner row {iv}/{nv} was not redistributed uniformly in column 0"
            );
        }
        assert_eq!(nu, 4);

        st.warp_edit = Some((st.doc_gen, GelWarp::identity_grid(w, h, 4, 5)));
        st.normalize_inner_knots = false;
        let knots = st.warp_knots();
        assert!(st.press_warp_knot(knots[0].0 as f64, knots[0].1 as f64));
        st.drag_warp_knot(0.22, 0.28);
        st.release_warp_knot();
        let warp = st.fit_warp().unwrap();
        let unchanged_inner = warp.control_point(0, 1);
        assert!(
            (unchanged_inner.0 - 0.0).abs() < 1e-6 && (unchanged_inner.1 - h / 4.0).abs() < 1e-6,
            "inner knot moved despite Normalize inner knots being off"
        );
    }

    #[test]
    fn no_bp_readout_without_ladder() {
        // A fresh capture has no identified ladder → no size readout.
        let mut st = AppState::new();
        st.capture().unwrap();
        assert!(st.hover_size_label(0.5).is_none());
    }

    #[test]
    fn trace_selection_and_compute() {
        let mut st = AppState::new();
        st.load_demo();
        // Nothing selected → no traces.
        assert!(st.compute_traces().is_empty());

        // Select the ladder lane (0) and one sample lane (1).
        st.toggle_lane(0);
        st.toggle_lane(1);
        let traces = st.compute_traces();
        assert_eq!(traces.len(), 2);
        assert!(traces.iter().any(|t| t.ladder));
        assert!(traces[0].values.len() > 10);

        // lane_items reflects the selection.
        let sel = st
            .lane_items()
            .into_iter()
            .filter(|(_, _, _, s)| *s)
            .count();
        assert_eq!(sel, 2);

        // Switching to ng mode still yields traces (scaled by calibration).
        st.set_trace_mode(1);
        assert_eq!(st.trace_mode, TraceMode::Ng);
        assert_eq!(st.compute_traces().len(), 2);

        // Toggling a lane off removes it.
        st.toggle_lane(1);
        assert_eq!(st.compute_traces().len(), 1);
    }

    #[test]
    fn ladder_names_match_gel_type() {
        let st = AppState::new();
        assert!(!st.ladder_names().is_empty());
        assert!(st
            .ladder_names()
            .iter()
            .all(|n| opengel::core::ladders::by_name(n).is_some()));
    }
}
