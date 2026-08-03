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

pub struct OpenDocumentState {
    pub doc: GelDocument,
    pub work: Option<GrayF32>,
    pub gel_type: GelType,
    pub source_path: Option<PathBuf>,
    pub display_name: String,
    pub dirty: bool,
    pub rotation_deg: f64,
    pub disp_lo: f32,
    pub disp_hi: f32,
    pub invert: bool,
    pub show_unwarped: bool,
    pub hdr_bias_subtraction: bool,
    pub hdr_align: bool,
    pub hdr_deghost: bool,
    pub view_frame: Option<usize>,
    /// Which acquisition channel is displayed and analysed.
    pub view_channel: u32,
    pub trace_mode: TraceMode,
    pub trace_zoom: f64,
    pub trace_pan: f64,
    pub trace_view: std::cell::Cell<(f64, f64, usize)>,
    pub selected_lanes: std::collections::BTreeSet<u32>,
    pub ladder_volume: std::collections::BTreeMap<u32, f64>,
    pub ladder_conc: std::collections::BTreeMap<u32, f64>,
    pub collapsed_lanes: std::collections::BTreeSet<u32>,
    pub selected: Option<Selection>,
    pub ratio_a: Option<u32>,
    pub ratio_b: Option<u32>,
    pub dragging: bool,
    pub warp_edit: Option<(u64, opengel::core::warp::GelWarp)>,
    pub dragging_knot: Option<(usize, usize)>,
    pub warp_drag_base: Option<opengel::core::warp::GelWarp>,
    pub doc_gen: u64,
    pub show_warp: bool,
    pub normalize_inner_knots: bool,
    pub show_overexposed: bool,
    pub annotation_alpha: f32,
    pub hover_x: f32,
    pub hover_y: f32,
}

pub struct AppState {
    pub documents: Vec<Option<OpenDocumentState>>,
    pub active_document: Option<usize>,
    next_doc_gen: u64,
    pub doc: Option<GelDocument>,
    pub work: Option<GrayF32>,
    pub gel_type: GelType,
    pub source_path: Option<PathBuf>,
    pub current_label: String,
    pub dirty: bool,
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
    /// Use GelGenie ML segmentation instead of the classical detector.
    pub use_gelgenie_ml: bool,
    /// 0 = CPU, 1 = WGPU.
    pub gelgenie_runtime_index: i32,
    /// Optical-flow smoothness for model fitting.
    pub flow_smoothness: f64,
    /// Extra v-axis (migration) control rows beyond observed ladder/front rows.
    pub extra_vertical_edges: usize,
    /// Extra u-axis (cross-lane) control columns beyond gel edges + one per lane.
    pub extra_horizontal_edges: usize,
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
    /// Which acquisition channel is displayed and analysed.
    pub view_channel: u32,
    /// Optional stages for the HDR "Recompute" action (bias / align / de-ghost).
    pub hdr_bias_subtraction: bool,
    pub hdr_align: bool,
    pub hdr_deghost: bool,
    /// Trace-tab state: y-axis mode and which lanes are plotted.
    pub trace_mode: TraceMode,
    /// Trace-plot horizontal zoom (>=1; 1 = the auto-cropped signal range).
    pub trace_zoom: f64,
    /// Trace-plot horizontal pan: center of the visible window as a fraction
    /// `[0,1]` of the auto-cropped range (0.5 = centered).
    pub trace_pan: f64,
    /// Cached visible trace window `(k0, k1, n)` (migration rows) from the last
    /// render, so a hover can map the cursor to a size without recomputing traces.
    pub trace_view: std::cell::Cell<(f64, f64, usize)>,
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
    /// The two bands chosen (via "Set A"/"Set B") for the density-ratio readout.
    pub ratio_a: Option<u32>,
    pub ratio_b: Option<u32>,
    /// Most recently used built-in ladder templates, newest first. These are
    /// surfaced first in the ladder picker, then persisted by the GUI.
    pub recent_ladders: Vec<String>,
    /// True while a drag is in progress on the selected annotation.
    pub dragging: bool,
    /// Manual edit of the warp control lattice, paired with the `doc_gen` it was
    /// made against so it auto-invalidates when the document/analysis changes.
    pub warp_edit: Option<(u64, opengel::core::warp::GelWarp)>,
    /// The warp control point `(iu, iv)` currently being dragged, if any.
    pub dragging_knot: Option<(usize, usize)>,
    /// Snapshot of the warp when the current knot drag began. Inner-knot
    /// normalization deforms this fitted shape affinely by the edge's movement,
    /// so the fitted smile is preserved rather than flattened to a line.
    pub warp_drag_base: Option<opengel::core::warp::GelWarp>,
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
    /// Set when the user cancels: the UI is released immediately, and whatever
    /// frame the (uninterruptible) in-flight grab eventually returns is discarded
    /// rather than adopted.
    pub cancel_requested: bool,
    /// Human-readable progress line for the capture dialog.
    pub capture_status: String,
    /// Set by `--sim`: connect the simulated enclosure as soon as the
    /// instrument list arrives, then light it and start the preview.
    pub autostart_sim: bool,
    /// True when the capture in flight is an exposure *measurement* (the "Auto"
    /// button): its frame configures the channel instead of becoming a document.
    pub auto_metering: bool,
    /// Most recent live preview frame.
    pub preview: Option<GrayF32>,

    // ---- Gel Doc EZ tab ----
    /// The imaging enclosure, the acquisition plan and the run state machine.
    /// Separate from the live-capture fields above because an enclosure run is
    /// a sequenced instrument operation, not a frame grab.
    ///
    /// The plan lives here rather than on the Live tab because exposure settings
    /// belong to a *channel* (a light source), and both tabs shoot channels: the
    /// Live tab frames one by hand, this tab runs a list of them.
    pub geldoc: crate::geldoc::GelDocState,
}

/// Exposure-time slider range (seconds), log-mapped. Covers sub-millisecond to
/// multi-second, spanning the useful range for dim fluorescence to bright
/// fields. Shared with the instrument so a time set on one tab means the same
/// thing on the other.
pub use opengel::instrument::acquisition::{
    EXPOSURE_MAX_S, EXPOSURE_MIN_S, HDR_STEP_OPTIONS,
};

/// Default ladder load assumed when a ladder lane has no explicit value:
/// 10 µL × 50 ng/µL = 500 ng.
pub const DEFAULT_LADDER_VOLUME_UL: f64 = 10.0;
pub const DEFAULT_LADDER_CONC_NG_UL: f64 = 50.0;

impl AppState {
    pub fn new() -> Self {
        AppState {
            documents: Vec::new(),
            active_document: None,
            next_doc_gen: 1,
            doc: None,
            work: None,
            gel_type: GelType::Dna,
            source_path: None,
            current_label: "Untitled gel".to_string(),
            dirty: false,
            hdr_bias_subtraction: false,
            hdr_align: false,
            hdr_deghost: false,
            rotation_deg: 0.0,
            disp_lo: 0.0,
            disp_hi: 1.0,
            invert: false,
            show_unwarped: false,
            optical_flow: false,
            use_gelgenie_ml: false,
            gelgenie_runtime_index: 0,
            flow_smoothness: 8.0,
            extra_vertical_edges: 2,
            extra_horizontal_edges: 0,
            warp_regularization: 1e-2,
            row_spacing_weight: 10.0,
            show_warp: false,
            normalize_inner_knots: true,
            show_overexposed: false,
            annotation_alpha: 0.25,
            hover_x: -1.0,
            hover_y: -1.0,
            view_frame: None,
            view_channel: 0,
            trace_mode: TraceMode::Intensity,
            trace_zoom: 1.0,
            trace_pan: 0.5,
            trace_view: std::cell::Cell::new((0.0, 0.0, 0)),
            selected_lanes: std::collections::BTreeSet::new(),
            ladder_volume: std::collections::BTreeMap::new(),
            ladder_conc: std::collections::BTreeMap::new(),
            collapsed_lanes: std::collections::BTreeSet::new(),
            selected: None,
            ratio_a: None,
            ratio_b: None,
            recent_ladders: Vec::new(),
            dragging: false,
            warp_edit: None,
            dragging_knot: None,
            warp_drag_base: None,
            doc_gen: 0,
            bracket_counter: 0,
            cam: None,
            cameras: Vec::new(),
            selected_camera: 0,
            camera_name: "—".to_string(),
            live_running: false,
            exposure_supported: true,
            capturing: false,
            cancel_requested: false,
            capture_status: String::new(),
            autostart_sim: false,
            auto_metering: false,
            preview: None,
            geldoc: crate::geldoc::GelDocState::new(),
        }
    }

    fn display_name_for(path: Option<&Path>, fallback: &str) -> String {
        path.and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .filter(|n| !n.trim().is_empty())
            .unwrap_or(fallback)
            .to_string()
    }

    fn current_display_name(&self) -> String {
        self.source_path.as_deref().map_or_else(
            || self.current_label.clone(),
            |path| Self::display_name_for(Some(path), "Untitled gel"),
        )
    }

    fn snapshot_current_document(&mut self) -> Option<OpenDocumentState> {
        let display_name = self.current_display_name();
        let doc = self.doc.take()?;
        Some(OpenDocumentState {
            doc,
            work: self.work.take(),
            gel_type: self.gel_type,
            source_path: self.source_path.take(),
            display_name,
            dirty: self.dirty,
            rotation_deg: self.rotation_deg,
            disp_lo: self.disp_lo,
            disp_hi: self.disp_hi,
            invert: self.invert,
            show_unwarped: self.show_unwarped,
            hdr_bias_subtraction: self.hdr_bias_subtraction,
            hdr_align: self.hdr_align,
            hdr_deghost: self.hdr_deghost,
            view_frame: self.view_frame,
            view_channel: self.view_channel,
            trace_mode: self.trace_mode,
            trace_zoom: self.trace_zoom,
            trace_pan: self.trace_pan,
            trace_view: std::cell::Cell::new(self.trace_view.get()),
            selected_lanes: std::mem::take(&mut self.selected_lanes),
            ladder_volume: std::mem::take(&mut self.ladder_volume),
            ladder_conc: std::mem::take(&mut self.ladder_conc),
            collapsed_lanes: std::mem::take(&mut self.collapsed_lanes),
            selected: self.selected.take(),
            ratio_a: self.ratio_a.take(),
            ratio_b: self.ratio_b.take(),
            dragging: self.dragging,
            warp_edit: self.warp_edit.take(),
            dragging_knot: self.dragging_knot.take(),
            warp_drag_base: self.warp_drag_base.take(),
            doc_gen: self.doc_gen,
            show_warp: self.show_warp,
            normalize_inner_knots: self.normalize_inner_knots,
            show_overexposed: self.show_overexposed,
            annotation_alpha: self.annotation_alpha,
            hover_x: self.hover_x,
            hover_y: self.hover_y,
        })
    }

    fn restore_document(&mut self, mut open: OpenDocumentState) {
        self.gel_type = open.gel_type;
        self.work = open.work.take();
        self.doc = Some(open.doc);
        self.source_path = open.source_path.take();
        self.current_label = open.display_name;
        self.dirty = open.dirty;
        self.rotation_deg = open.rotation_deg;
        self.disp_lo = open.disp_lo;
        self.disp_hi = open.disp_hi;
        self.invert = open.invert;
        self.show_unwarped = open.show_unwarped;
        self.hdr_bias_subtraction = open.hdr_bias_subtraction;
        self.hdr_align = open.hdr_align;
        self.hdr_deghost = open.hdr_deghost;
        self.view_frame = open.view_frame;
        self.view_channel = open.view_channel;
        self.trace_mode = open.trace_mode;
        self.trace_zoom = open.trace_zoom;
        self.trace_pan = open.trace_pan;
        self.trace_view = std::cell::Cell::new(open.trace_view.get());
        self.selected_lanes = open.selected_lanes;
        self.ladder_volume = open.ladder_volume;
        self.ladder_conc = open.ladder_conc;
        self.collapsed_lanes = open.collapsed_lanes;
        self.selected = open.selected;
        self.ratio_a = open.ratio_a;
        self.ratio_b = open.ratio_b;
        self.dragging = open.dragging;
        self.warp_edit = open.warp_edit;
        self.dragging_knot = open.dragging_knot;
        self.warp_drag_base = open.warp_drag_base;
        self.doc_gen = open.doc_gen;
        self.show_warp = open.show_warp;
        self.normalize_inner_knots = open.normalize_inner_knots;
        self.show_overexposed = open.show_overexposed;
        self.annotation_alpha = open.annotation_alpha;
        self.hover_x = open.hover_x;
        self.hover_y = open.hover_y;
    }

    fn clear_current_document(&mut self) {
        self.doc = None;
        self.work = None;
        self.source_path = None;
        self.current_label = "Untitled gel".to_string();
        self.dirty = false;
        self.gel_type = GelType::Dna;
        self.rotation_deg = 0.0;
        self.disp_lo = 0.0;
        self.disp_hi = 1.0;
        self.invert = false;
        self.show_unwarped = false;
        self.hdr_bias_subtraction = false;
        self.hdr_align = false;
        self.hdr_deghost = false;
        self.view_frame = None;
        self.view_channel = 0;
        self.trace_mode = TraceMode::Intensity;
        self.trace_zoom = 1.0;
        self.trace_pan = 0.5;
        self.trace_view = std::cell::Cell::new((0.0, 0.0, 0));
        self.selected_lanes.clear();
        self.ladder_volume.clear();
        self.ladder_conc.clear();
        self.collapsed_lanes.clear();
        self.clear_selection();
        self.warp_edit = None;
        self.dragging_knot = None;
        self.warp_drag_base = None;
        self.bump_doc_gen();
        self.show_warp = false;
        self.normalize_inner_knots = true;
        self.show_overexposed = false;
        self.annotation_alpha = 0.25;
        self.hover_x = -1.0;
        self.hover_y = -1.0;
    }

    fn save_active_slot(&mut self) {
        let Some(idx) = self.active_document else {
            return;
        };
        if idx >= self.documents.len() || self.documents[idx].is_some() {
            return;
        }
        if let Some(snapshot) = self.snapshot_current_document() {
            self.documents[idx] = Some(snapshot);
        }
    }

    fn replace_active_with_document(
        &mut self,
        doc: GelDocument,
        source_path: Option<PathBuf>,
        fallback_name: &str,
    ) {
        self.save_active_slot();
        let idx = self.documents.len();
        self.documents.push(None);
        self.active_document = Some(idx);
        self.gel_type = doc.project.gel_type;
        self.work = doc.working_image();
        self.doc = Some(doc);
        let dirty = source_path.is_none();
        self.current_label = Self::display_name_for(source_path.as_deref(), fallback_name);
        self.dirty = dirty;
        self.source_path = source_path;
        self.rotation_deg = 0.0;
        self.invert = false;
        self.show_unwarped = false;
        self.view_frame = None;
        self.view_channel = 0;
        self.hdr_bias_subtraction = false;
        self.hdr_align = false;
        self.hdr_deghost = false;
        self.trace_mode = TraceMode::Intensity;
        self.trace_zoom = 1.0;
        self.trace_pan = 0.5;
        self.trace_view = std::cell::Cell::new((0.0, 0.0, 0));
        self.selected_lanes.clear();
        self.ladder_volume.clear();
        self.ladder_conc.clear();
        self.collapsed_lanes.clear();
        self.show_warp = false;
        self.normalize_inner_knots = true;
        self.show_overexposed = false;
        self.annotation_alpha = 0.25;
        self.hover_x = -1.0;
        self.hover_y = -1.0;
        self.reset_display_window();
        self.clear_selection();
        self.warp_edit = None;
        self.dragging_knot = None;
        self.warp_drag_base = None;
        self.bump_doc_gen();
    }

    fn bump_doc_gen(&mut self) {
        self.doc_gen = self.next_doc_gen;
        self.next_doc_gen = self.next_doc_gen.wrapping_add(1).max(1);
    }

    fn mark_dirty(&mut self) {
        if self.doc.is_some() {
            self.dirty = true;
        }
    }

    fn paths_same(a: &Path, b: &Path) -> bool {
        match (a.canonicalize(), b.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => a == b,
        }
    }

    pub fn document_labels(&self) -> Vec<String> {
        self.documents
            .iter()
            .enumerate()
            .map(|(i, doc)| {
                if Some(i) == self.active_document && doc.is_none() {
                    if self.dirty {
                        format!("{} *", self.current_display_name())
                    } else {
                        self.current_display_name()
                    }
                } else {
                    doc.as_ref().map_or_else(
                        || "Untitled gel".to_string(),
                        |d| {
                            if d.dirty {
                                format!("{} *", d.display_name)
                            } else {
                                d.display_name.clone()
                            }
                        },
                    )
                }
            })
            .collect()
    }

    pub fn active_document_index(&self) -> i32 {
        self.active_document.map_or(-1, |i| i as i32)
    }

    pub fn has_open_file(&self) -> bool {
        self.doc.is_some()
    }

    pub fn active_document_dirty(&self) -> bool {
        self.dirty
    }

    pub fn set_active_document(&mut self, idx: usize) -> String {
        if Some(idx) == self.active_document {
            return self.current_display_name();
        }
        if idx >= self.documents.len() {
            return "No such open file.".into();
        }
        self.save_active_slot();
        match self.documents[idx].take() {
            Some(open) => {
                self.active_document = Some(idx);
                let label = open.display_name.clone();
                self.restore_document(open);
                format!("Switched to {label}.")
            }
            None => "No such open file.".into(),
        }
    }

    pub fn close_active_document(&mut self) -> String {
        let Some(idx) = self.active_document else {
            return "No file open.".into();
        };
        let label = self.current_display_name();
        self.doc = None;
        self.work = None;
        self.documents.remove(idx);
        if self.documents.is_empty() {
            self.active_document = None;
            self.clear_current_document();
        } else {
            let next = idx.min(self.documents.len() - 1);
            self.active_document = Some(next);
            if let Some(open) = self.documents[next].take() {
                self.restore_document(open);
            }
        }
        format!("Closed {label}.")
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
        self.mark_dirty();
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
                    Some(s) => merged_size_label(s, &b.merged_sizes, unit),
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
            self.mark_dirty();
            format!("Renamed lane {lane_id}.")
        } else {
            "No such lane.".into()
        }
    }

    /// Delete a lane and everything attached to it.
    /// Select a lane (`kind == 0`) or band (`kind == 1`) from the tree list.
    pub fn select_tree(&mut self, kind: i32, id: u32) {
        self.selected = Some(if kind == 0 {
            Selection::Lane(id)
        } else {
            Selection::Band(id)
        });
    }

    /// Set band A (or B) of the density ratio to the currently selected band.
    pub fn set_ratio_a(&mut self) -> String {
        match self.selected {
            Some(Selection::Band(id)) => {
                self.ratio_a = Some(id);
                "Set A.".into()
            }
            _ => "Select a band first.".into(),
        }
    }
    pub fn set_ratio_b(&mut self) -> String {
        match self.selected {
            Some(Selection::Band(id)) => {
                self.ratio_b = Some(id);
                "Set B.".into()
            }
            _ => "Select a band first.".into(),
        }
    }

    /// Short name of a band (its size, else `band #id`).
    fn band_name(&self, id: u32) -> Option<String> {
        let a = self.analysis()?;
        let b = a.bands.iter().find(|b| b.id == id)?;
        let unit = self.gel_type.size_unit();
        Some(match b.known_size {
            Some(s) => merged_size_label(s, &b.merged_sizes, unit),
            None => match b.size {
                Some(s) => format!("{s:.0} {unit}"),
                None => format!("band {id}"),
            },
        })
    }

    /// Density-ratio readout for the chosen A and B bands.
    pub fn ratio_label(&self) -> String {
        let a_txt = self.ratio_a.and_then(|id| self.band_name(id));
        let b_txt = self.ratio_b.and_then(|id| self.band_name(id));
        let (Some(a_name), Some(b_name)) = (&a_txt, &b_txt) else {
            return format!(
                "Ratio: A = {}, B = {} (select a band, then Set A / Set B)",
                a_txt.as_deref().unwrap_or("—"),
                b_txt.as_deref().unwrap_or("—"),
            );
        };
        let analysis = self.analysis();
        let band = |id: u32| analysis.and_then(|a| a.bands.iter().find(|b| b.id == id));
        let (Some(ba), Some(bb)) = (band(self.ratio_a.unwrap()), band(self.ratio_b.unwrap()))
        else {
            return "Ratio: (bands not found)".into();
        };
        match compare(
            ba.integrated_density,
            ba.size,
            bb.integrated_density,
            bb.size,
        ) {
            Some(rel) => {
                let molar = rel
                    .molar_ratio
                    .map(|m| format!(", molar {m:.3}"))
                    .unwrap_or_default();
                format!(
                    "Ratio A/B ({a_name} / {b_name}): {:.3}{molar}",
                    rel.density_ratio
                )
            }
            None => "Ratio: unavailable".into(),
        }
    }

    /// `(selected_lane_id, selected_band_id, selected_lane_is_ladder)` for the UI
    /// (a selected band reports its parent lane). `-1` means none.
    pub fn selection_info(&self) -> (i32, i32, bool) {
        let a = self.analysis();
        let is_ladder =
            |lid: u32| a.is_some_and(|a| a.lanes.iter().any(|l| l.id == lid && l.is_ladder));
        match self.selected {
            Some(Selection::Lane(id)) => (id as i32, -1, is_ladder(id)),
            Some(Selection::Band(bid)) => {
                let lane = a.and_then(|a| a.bands.iter().find(|b| b.id == bid).map(|b| b.lane_id));
                (
                    lane.map_or(-1, |l| l as i32),
                    bid as i32,
                    lane.is_some_and(is_ladder),
                )
            }
            None => (-1, -1, false),
        }
    }

    /// Current label of the selected lane, for the rename dialog.
    pub fn rename_dialog_prefill(&self) -> String {
        match self.selected {
            Some(Selection::Lane(id)) => self
                .analysis()
                .and_then(|a| a.lanes.iter().find(|l| l.id == id))
                .and_then(|l| l.label.clone())
                .unwrap_or_else(|| format!("Lane {id}")),
            _ => String::new(),
        }
    }

    /// Delete whatever annotation is currently selected on the gel (a lane and
    /// its bands, or a single band). `None`-selection returns a hint.
    pub fn delete_selected(&mut self) -> String {
        match self.selected.take() {
            Some(Selection::Lane(id)) => self.delete_lane(id),
            Some(Selection::Band(id)) => self.delete_band_by_id(id),
            None => "Select a lane or band on the gel first, then Delete.".into(),
        }
    }

    pub fn delete_lane(&mut self, lane_id: u32) -> String {
        let Some(doc) = self.doc.as_mut() else {
            return "No document.".into();
        };
        let a = &mut doc.project.analysis;
        let before = a.lanes.len();
        let deleted_bands: std::collections::BTreeSet<u32> = a
            .bands
            .iter()
            .filter(|b| b.lane_id == lane_id)
            .map(|b| b.id)
            .collect();
        a.lanes.retain(|l| l.id != lane_id);
        a.bands.retain(|b| b.lane_id != lane_id);
        a.ladder_assignments.retain(|la| la.lane_id != lane_id);
        a.quantifications
            .retain(|q| !deleted_bands.contains(&q.target_id));
        for la in &mut a.ladder_assignments {
            for slot in &mut la.rung_to_band {
                if slot.is_some_and(|id| deleted_bands.contains(&id)) {
                    *slot = None;
                }
            }
        }
        self.ladder_volume.remove(&lane_id);
        self.ladder_conc.remove(&lane_id);
        self.selected_lanes.remove(&lane_id);
        self.collapsed_lanes.remove(&lane_id);
        if a.lanes.len() < before {
            self.selected = match self.selected {
                Some(Selection::Lane(id)) if id == lane_id => None,
                Some(Selection::Band(id)) if deleted_bands.contains(&id) => None,
                other => other,
            };
            if self.ratio_a.is_some_and(|id| deleted_bands.contains(&id)) {
                self.ratio_a = None;
            }
            if self.ratio_b.is_some_and(|id| deleted_bands.contains(&id)) {
                self.ratio_b = None;
            }
            self.mark_dirty();
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
        self.mark_dirty();
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
        self.set_lane_ladder_by_name(lane_id, &name)
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

    /// Labels for the channel selector, e.g. "DyLight 549 (Green)".
    pub fn channel_labels(&self) -> Vec<String> {
        self.doc
            .as_ref()
            .map(|d| {
                d.project
                    .channels
                    .iter()
                    .map(|c| match c.color {
                        opengel::core::ChannelColor::Gray => c.name.clone(),
                        color => format!("{} ({})", c.name, color.label()),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// True when the open document has more than one channel, which is the only
    /// case where the channel selector is worth showing.
    pub fn is_multichannel(&self) -> bool {
        self.doc
            .as_ref()
            .is_some_and(|d| d.project.is_multichannel())
    }

    /// Show and analyse a different channel.
    ///
    /// The working image is recomputed from that channel alone. Lanes, bands
    /// and the warp are deliberately left untouched: channels are the same gel
    /// under different light, so the geometry found in one is the geometry of
    /// all of them.
    pub fn set_view_channel(&mut self, sel: usize) {
        let Some(doc) = &self.doc else { return };
        let Some(channel) = doc.project.channels.get(sel) else {
            return;
        };
        self.view_channel = channel.id;
        // A saved HDR merge belongs to whichever channel produced it, so it
        // cannot stand in for another one.
        self.work = if doc.merged.is_some() {
            let id = channel.id;
            doc.project
                .image_indices_for_channel(id)
                .first()
                .and_then(|&i| doc.frames.get(i))
                .map(GrayF32::from_dynamic)
        } else {
            doc.working_image_for_channel(channel.id)
        };
        self.view_frame = None;
        self.bump_doc_gen();
    }

    /// The selector index matching the current `view_channel`.
    pub fn view_channel_index(&self) -> usize {
        self.doc
            .as_ref()
            .and_then(|d| {
                d.project
                    .channels
                    .iter()
                    .position(|c| c.id == self.view_channel)
            })
            .unwrap_or(0)
    }

    /// Every metadata row to show in the Metadata tab, as
    /// `(section, name, value)`.
    ///
    /// Sections are just headings — nothing here is interpreted, because the
    /// instruments disagree about what they report and the useful thing is to
    /// show whatever was recorded rather than the subset we anticipated.
    pub fn metadata_rows(&self) -> Vec<(String, String, String)> {
        let Some(doc) = &self.doc else {
            return Vec::new();
        };
        let p = &doc.project;
        let mut rows = Vec::new();

        rows.push((
            "Document".to_string(),
            "Gel type".to_string(),
            format!("{:?}", p.gel_type),
        ));
        if let Some(path) = &self.source_path {
            rows.push((
                "Document".to_string(),
                "File".to_string(),
                path.display().to_string(),
            ));
        }
        for a in &p.metadata {
            rows.push(("Document".to_string(), a.name.clone(), a.value.clone()));
        }

        for channel in &p.channels {
            let section = if p.is_multichannel() {
                format!("Channel {} — {}", channel.id + 1, channel.name)
            } else {
                "Acquisition".to_string()
            };
            if p.is_multichannel() {
                rows.push((
                    section.clone(),
                    "Display colour".to_string(),
                    channel.color.label().to_string(),
                ));
            }
            for &i in &p.image_indices_for_channel(channel.id) {
                let img = &p.images[i];
                rows.push((
                    section.clone(),
                    "Size".to_string(),
                    format!("{} × {} px", img.width, img.height),
                ));
                if img.meta.acquisition.is_empty() {
                    // Our own captures have no vendor record; say so rather
                    // than showing an empty section.
                    rows.push((
                        section.clone(),
                        "Exposure".to_string(),
                        format!("{:.3} s", img.meta.exposure_seconds),
                    ));
                    if let Some(cam) = &img.meta.camera_name {
                        rows.push((section.clone(), "Camera".to_string(), cam.clone()));
                    }
                }
                for a in &img.meta.acquisition {
                    rows.push((section.clone(), a.name.clone(), a.value.clone()));
                }
            }
        }
        rows
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
        // Invert the *displayed* NURBS (including any manual knot edits): rectify
        // resamples out[u,v] = img(eval(u,v)), i.e. the inverse of the shown grid.
        let warp = self
            .fit_warp()
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

    /// Warp control points as `(nx, ny, active)` in normalized image coords, for
    /// drawing draggable handles as a **separate Slint overlay** (not composited
    /// into the gel image, so edge knots aren't clipped). `active` marks the knot
    /// currently being dragged. Empty when the grid is hidden.
    pub fn warp_knot_items(&self) -> Vec<(f32, f32, bool)> {
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
                let active = self.dragging_knot == Some((iu, iv));
                out.push(((cx / w.max(1.0)) as f32, (cy / h.max(1.0)) as f32, active));
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
                self.warp_drag_base = Some(warp.clone());
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
        let base = self.warp_drag_base.clone();
        if let Some((gen, warp)) = self.warp_edit.as_mut() {
            if *gen == self.doc_gen {
                // No clamp to [0,1]: a control point may sit outside the image
                // (the surface region can be pinned from a knot beyond the edge).
                warp.set_control_point(iu, iv, nx * w, ny * h);
                let (_, nv) = warp.grid_size();
                // Normalize inner rows when an edge knot moves — but preserve the
                // fitted smile: displace each inner knot from its *base* (pre-drag)
                // position by the affine blend of the two edges' displacements,
                // rather than snapping the column to a straight line. With one edge
                // held fixed this keeps the fitted curvature, just sheared to the
                // moved edge (consistent with the initial fit).
                if self.normalize_inner_knots && nv > 2 && (iv == 0 || iv + 1 == nv) {
                    if let Some(base) = base.as_ref() {
                        let (bt_x, bt_y) = base.control_point(iu, 0);
                        let (bb_x, bb_y) = base.control_point(iu, nv - 1);
                        let (t_x, t_y) = warp.control_point(iu, 0);
                        let (b_x, b_y) = warp.control_point(iu, nv - 1);
                        let (dtx, dty) = (t_x - bt_x, t_y - bt_y); // top displacement
                        let (dbx, dby) = (b_x - bb_x, b_y - bb_y); // bottom displacement
                        for inner_iv in 1..(nv - 1) {
                            let t = inner_iv as f64 / (nv - 1) as f64;
                            let (ox, oy) = base.control_point(iu, inner_iv);
                            warp.set_control_point(
                                iu,
                                inner_iv,
                                ox + (1.0 - t) * dtx + t * dbx,
                                oy + (1.0 - t) * dty + t * dby,
                            );
                        }
                    }
                }
            }
        }
    }

    /// End a knot drag.
    pub fn release_warp_knot(&mut self) {
        self.dragging_knot = None;
        self.warp_drag_base = None;
        self.mark_dirty();
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

    /// The channel the live view is framing: the one whose settings the Live tab
    /// edits and whose exposure the preview runs at.
    pub fn live_channel(&self) -> &opengel::instrument::ChannelPlan {
        self.geldoc.channel()
    }

    /// The exposure the preview and a single capture run at, i.e. the current
    /// channel's single-frame time.
    pub fn live_exposure_s(&self) -> f64 {
        self.geldoc.channel().exposure_s
    }

    /// Point the live view (and the settings controls) at another channel, and
    /// re-expose the camera for it.
    pub fn set_live_channel(&mut self, index: usize) {
        self.geldoc.select_channel(index);
        self.apply_live_exposure();
        // Selecting a channel is what lights it.
        self.geldoc_sync_lamps();
    }

    /// Set the current channel's exposure from a log-scale slider position, and
    /// apply it to the camera worker.
    pub fn set_live_exposure_slider(&mut self, f: f32) {
        let seconds = Self::exposure_from_slider(f);
        self.geldoc.set_exposure_s(seconds);
        self.apply_live_exposure();
    }

    /// Push the current channel's exposure to the camera.
    pub fn apply_live_exposure(&self) {
        if let Some(cam) = &self.cam {
            cam.set_exposure(self.live_exposure_s().max(EXPOSURE_MIN_S));
        }
    }

    /// Slider position for the current exposure (to initialize the UI).
    pub fn live_exposure_slider(&self) -> f32 {
        Self::slider_from_exposure(self.live_exposure_s())
    }

    /// Adopt the current exposure as this channel's lower / upper HDR bound.
    pub fn set_hdr_lower_from_current(&mut self) {
        let t = self.live_exposure_s();
        self.geldoc.set_hdr_min_s(t);
    }
    pub fn set_hdr_upper_from_current(&mut self) {
        let t = self.live_exposure_s();
        self.geldoc.set_hdr_max_s(t);
    }

    /// Number of HDR steps from a dropdown index into [`HDR_STEP_OPTIONS`].
    pub fn set_hdr_steps_idx(&mut self, idx: usize) {
        let steps = HDR_STEP_OPTIONS.get(idx).copied().unwrap_or(3);
        self.geldoc.set_hdr_steps(steps);
    }
    pub fn hdr_steps_idx(&self) -> usize {
        HDR_STEP_OPTIONS
            .iter()
            .position(|&n| n == self.geldoc.channel().hdr_steps)
            .unwrap_or(1)
    }

    /// Single frame or HDR bracket for the current channel.
    pub fn set_capture_mode_idx(&mut self, idx: usize) {
        self.geldoc
            .set_capture_mode(opengel::instrument::CaptureMode::from_index(idx));
    }

    /// Dynamic range covered by the current bracket, in EV (stops). Shown in the
    /// UI so the user can judge whether the step count fits the range.
    pub fn hdr_range_ev(&self) -> f64 {
        self.geldoc.channel().hdr_range_ev()
    }

    /// Set the list of available cameras (reported by the worker).
    pub fn set_cameras(&mut self, names: Vec<String>) {
        if self.selected_camera >= names.len() {
            self.selected_camera = 0;
        }
        self.cameras = names;
    }

    /// Ask the workers to look for hardware again — cameras *and* enclosures.
    ///
    /// Plugging the imager in is how the Capture tab grows its channels and its
    /// interlocks, so the button that looks for hardware has to look for all of
    /// it, not just the camera.
    pub fn refresh_cameras(&self) {
        if let Some(cam) = &self.cam {
            cam.list_cameras();
        }
        if let Some(inst) = &self.geldoc.inst {
            inst.list();
        }
    }

    /// Select a camera by index; reopens it (keeping exposure + preview state).
    pub fn select_camera(&mut self, idx: usize) {
        self.selected_camera = idx;
        let exposure = self.live_exposure_s().max(EXPOSURE_MIN_S);
        if let Some(cam) = &self.cam {
            cam.open(idx);
            cam.set_exposure(exposure);
            if self.live_running {
                cam.start_preview();
            }
        }
    }

    /// Open the selected camera and begin live preview (on the worker thread).
    pub fn live_start(&mut self) -> String {
        self.live_running = true;
        let exposure = self.live_exposure_s().max(EXPOSURE_MIN_S);
        if let Some(cam) = &self.cam {
            cam.open(self.selected_camera);
            cam.set_exposure(exposure);
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
        self.replace_active_with_document(doc, None, "Captured gel");
    }

    /// Make a captured multi-channel acquisition the current gel document.
    ///
    /// One document, one channel per light source: they are separate
    /// acquisitions of the same gel, sharing its geometry, so they belong in the
    /// same document rather than in one document each.
    pub fn adopt_capture_channels(&mut self, channels: Vec<opengel::core::CapturedChannel>) {
        let doc = GelDocument::from_channels(self.gel_type, channels);
        self.replace_active_with_document(doc, None, "Captured gel");
    }

    /// A fresh bracket-group id, so frames of one bracket are recognisable as
    /// belonging together (and frames of different ones never merge).
    pub fn next_bracket_group(&mut self) -> u32 {
        let group = self.bracket_counter;
        self.bracket_counter += 1;
        group
    }

    /// Capture the current channel on the worker (non-blocking): one frame, or
    /// the channel's HDR bracket. The result arrives via a [`CamEvent`] and is
    /// applied by the UI event loop.
    pub fn live_capture(&mut self) {
        let group = self.next_bracket_group();
        let exposures = self.geldoc.channel().exposures();
        self.capturing = true;
        self.cancel_requested = false;
        self.auto_metering = false;
        self.capture_status = if exposures.len() == 1 {
            "Capturing…".into()
        } else {
            format!("Capturing HDR bracket (0/{})…", exposures.len())
        };
        if let Some(cam) = &self.cam {
            cam.capture_hdr(exposures, group);
        }
    }

    /// Meter the scene and write what it settles on into the current channel's
    /// settings — the Live tab's "Auto". No document is produced: this sets up
    /// the exposure, it does not take the picture.
    pub fn live_auto_expose(&mut self) {
        self.capturing = true;
        self.cancel_requested = false;
        self.auto_metering = true;
        self.capture_status = format!("Metering {}…", self.geldoc.channel().label());
        if let Some(cam) = &self.cam {
            cam.capture_auto(
                crate::camera_worker::AutoExposureMode::IntenseBands,
                EXPOSURE_MIN_S,
                EXPOSURE_MAX_S,
            );
        }
    }

    /// Apply a metering result to the current channel. Returns the line to show.
    pub fn apply_auto_exposure(&mut self, metered_s: f64) -> String {
        self.auto_metering = false;
        self.geldoc.channel_mut().apply_metered(metered_s);
        self.apply_live_exposure();
        format!(
            "Auto — {}: {}",
            self.geldoc.channel().label(),
            self.geldoc.channel().summary()
        )
    }

    /// Request cancellation of an in-progress capture. The camera's frame grab
    /// can't be interrupted mid-exposure, so instead of blocking the UI on
    /// "Cancelling…" until it returns, release the dialog immediately and mark
    /// the pending result to be discarded when it finally arrives.
    pub fn cancel_capture(&mut self) {
        if let Some(cam) = &self.cam {
            cam.cancel();
        }
        if self.capturing {
            self.cancel_requested = true;
        }
        self.capturing = false;
        self.capture_status = "Capture cancelled.".into();
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

    /// Select every lane for plotting.
    pub fn select_all_lanes(&mut self) {
        if let Some(a) = self.analysis() {
            self.selected_lanes = a.lanes.iter().map(|l| l.id).collect();
        }
    }

    /// Clear the trace-plot selection.
    pub fn select_no_lanes(&mut self) {
        self.selected_lanes.clear();
    }

    /// Reset the trace plot to the default (auto-cropped) view.
    pub fn trace_reset_view(&mut self) {
        self.trace_zoom = 1.0;
        self.trace_pan = 0.5;
    }

    /// Set the trace zoom directly (from the slider), keeping the pan clamped so
    /// the visible window stays inside the cropped range.
    pub fn set_trace_zoom(&mut self, zoom: f64) {
        self.trace_zoom = zoom.clamp(1.0, 40.0);
        self.clamp_trace_pan();
    }

    /// Zoom by a multiplicative factor about a normalized focus `x` (0..1 across
    /// the *visible* window), keeping that point under the cursor.
    pub fn trace_zoom_by(&mut self, factor: f64, focus: f64) {
        let old = self.trace_zoom;
        let new = (old * factor).clamp(1.0, 40.0);
        if (new - old).abs() < 1e-9 {
            return;
        }
        // The visible half-width (in pan-fraction units) shrinks/grows with zoom;
        // keep the focused fraction of the window fixed as it does.
        let half_old = 0.5 / old;
        let half_new = 0.5 / new;
        let focus_frac = self.trace_pan - half_old + focus * (2.0 * half_old);
        self.trace_pan = focus_frac - (focus * 2.0 - 1.0) * half_new;
        self.trace_zoom = new;
        self.clamp_trace_pan();
    }

    /// Pan by a fraction of the visible window (drag): `dx` is the pointer motion
    /// as a fraction of the plot width.
    pub fn trace_pan_by(&mut self, dx: f64) {
        // Dragging right moves the content right → the window center moves left.
        self.trace_pan -= dx / self.trace_zoom;
        self.clamp_trace_pan();
    }

    fn clamp_trace_pan(&mut self) {
        let half = 0.5 / self.trace_zoom;
        self.trace_pan = self.trace_pan.clamp(half, 1.0 - half);
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
    pub fn sizing_fit(&self) -> Option<opengel::core::quant::SizingFit> {
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

    /// Round a size for a live readout (no false precision).
    fn round_size(size: f64) -> f64 {
        if size >= 1000.0 {
            (size / 10.0).round() * 10.0
        } else {
            size.round()
        }
    }

    /// A human-readable fragment-size estimate at a normalized hover position
    /// (e.g. `"Lane 3: ≈ 1230 bp"`), from the identified ladder's semi-log sizing
    /// model, prefixed with the lane under the cursor when there is one. `None`
    /// unless a ladder has been fitted and the position is over the image.
    pub fn hover_size_label(&self, nx: f32, ny: f32) -> Option<String> {
        if !(0.0..=1.0).contains(&ny) {
            return None;
        }
        let fit = self.sizing_fit()?;
        // Migration coordinate v ≡ the normalized vertical hover position.
        let size = fit.size_at(ny as f64);
        if !size.is_finite() || size <= 0.0 {
            return None;
        }
        let bp = format!(
            "≈ {:.0} {}",
            Self::round_size(size),
            self.gel_type.size_unit()
        );
        // Prefix the lane under the cursor (x-warp is near-linear, so u ≈ nx).
        let lane = self.analysis().and_then(|a| {
            a.lanes
                .iter()
                .find(|l| (l.u_min..=l.u_max).contains(&(nx as f64)))
                .map(|l| l.label.clone().unwrap_or_else(|| format!("Lane {}", l.id)))
        });
        Some(match lane {
            Some(name) => format!("{name}: {bp}"),
            None => bp,
        })
    }

    /// Size (bp/nt/Da) readout for a hover at horizontal fraction `f` (0..1)
    /// across the current trace plot's visible window. `None` without a ladder.
    pub fn trace_hover_bp_label(&self, f: f64) -> Option<String> {
        let (k0, k1, n) = self.trace_view.get();
        if n == 0 || k1 <= k0 {
            return None;
        }
        let fit = self.sizing_fit()?;
        let k = k0 + f.clamp(0.0, 1.0) * (k1 - k0);
        let size = fit.size_at((k / n as f64).clamp(0.0, 1.0));
        if !size.is_finite() || size <= 0.0 {
            return None;
        }
        Some(format!(
            "≈ {:.0} {} (migration {:.0} px)",
            Self::round_size(size),
            self.gel_type.size_unit(),
            k
        ))
    }

    /// Export the current trace plot (selected lanes, current zoom window, axes,
    /// bp scale and legend) to a vector PDF at `path`.
    pub fn export_trace_pdf(&self, path: &std::path::Path) -> anyhow::Result<()> {
        use printpdf::*;
        let mm = |v: f64| Mm(v as f32); // printpdf uses f32 millimetres
        let traces = self.compute_traces();
        if traces.is_empty() {
            anyhow::bail!("no lanes selected to plot");
        }
        let n = traces.iter().map(|t| t.values.len()).max().unwrap_or(0);
        let (mut k0, mut k1, _) = self.trace_view.get();
        if !(k1 > k0) {
            (k0, k1) = (0.0, n as f64);
        }
        let span = (k1 - k0).max(1e-6);
        let vmax = traces
            .iter()
            .flat_map(|t| t.values.iter().copied())
            .fold(0.0f64, f64::max)
            .max(1e-9);

        let (pw, ph) = (250.0f64, 160.0f64);
        let (doc, page, layer) = PdfDocument::new("OpenGel trace", mm(pw), mm(ph), "traces");
        let lyr = doc.get_page(page).get_layer(layer);
        let font = doc.add_builtin_font(BuiltinFont::Helvetica)?;

        // Plot rectangle (mm), origin bottom-left (PDF y grows upward).
        let (ml, mr, mt, mb) = (24.0, 44.0, 18.0, 20.0);
        let (px0, px1, py0, py1) = (ml, pw - mr, mb, ph - mt);
        let (pwmm, phmm) = (px1 - px0, py1 - py0);
        let seg = |ax: f64, ay: f64, bx: f64, by: f64| Line {
            points: vec![
                (Point::new(mm(ax), mm(ay)), false),
                (Point::new(mm(bx), mm(by)), false),
            ],
            is_closed: false,
        };

        lyr.set_outline_color(Color::Rgb(Rgb::new(0.2, 0.2, 0.2, None)));
        lyr.set_outline_thickness(0.6);
        lyr.add_line(seg(px0, py0, px1, py0));
        lyr.add_line(seg(px0, py0, px0, py1));
        lyr.add_line(seg(px0, py1, px1, py1)); // top axis (bp)

        // Traces.
        const PAL: [(f32, f32, f32); 6] = [
            (0.129, 0.463, 0.839),
            (0.839, 0.176, 0.176),
            (0.133, 0.627, 0.267),
            (0.588, 0.235, 0.784),
            (0.0, 0.588, 0.588),
            (0.839, 0.471, 0.078),
        ];
        const LAD: (f32, f32, f32) = (0.769, 0.549, 0.0);
        let mut si = 0usize;
        for t in &traces {
            let (r, g, b) = if t.ladder {
                LAD
            } else {
                let c = PAL[si % PAL.len()];
                si += 1;
                c
            };
            let pts: Vec<(Point, bool)> = t
                .values
                .iter()
                .enumerate()
                .map(|(k, &v)| {
                    let xf = ((k as f64 - k0) / span).clamp(0.0, 1.0);
                    let yf = (v / vmax).clamp(0.0, 1.0);
                    (Point::new(mm(px0 + xf * pwmm), mm(py0 + yf * phmm)), false)
                })
                .collect();
            lyr.set_outline_color(Color::Rgb(Rgb::new(r, g, b, None)));
            lyr.set_outline_thickness(0.4);
            lyr.add_line(Line {
                points: pts,
                is_closed: false,
            });
        }

        // Text: y-title, x-title/ticks, bp top ticks, legend.
        let black = Color::Rgb(Rgb::new(0.1, 0.1, 0.1, None));
        lyr.set_fill_color(black.clone());
        lyr.use_text(self.trace_mode.label(), 10.0, mm(px0), mm(py1 + 4.0), &font);
        lyr.use_text(
            "Migration (px)",
            10.0,
            mm(px0 + pwmm / 2.0 - 14.0),
            mm(4.0),
            &font,
        );
        for i in 0..5 {
            let x = px0 + pwmm * i as f64 / 4.0;
            let kf = k0 + span * i as f64 / 4.0;
            lyr.use_text(format!("{kf:.0}"), 8.0, mm(x - 4.0), mm(py0 - 5.0), &font);
        }
        if let Some(fit) = self.sizing_fit() {
            for i in 0..5 {
                let x = px0 + pwmm * i as f64 / 4.0;
                let bp = fit.size_at((k0 + span * i as f64 / 4.0) / n.max(1) as f64);
                lyr.use_text(format!("{bp:.0}"), 8.0, mm(x - 5.0), mm(py1 + 2.0), &font);
            }
            lyr.use_text(
                format!("Size ({})", self.gel_type.size_unit()),
                9.0,
                mm(px1 + 3.0),
                mm(py1 + 2.0),
                &font,
            );
        }
        let mut ly = py1 - 4.0;
        let mut si2 = 0usize;
        for t in &traces {
            let (r, g, b) = if t.ladder {
                LAD
            } else {
                let c = PAL[si2 % PAL.len()];
                si2 += 1;
                c
            };
            lyr.set_fill_color(Color::Rgb(Rgb::new(r, g, b, None)));
            lyr.use_text(t.label.clone(), 9.0, mm(px1 + 3.0), mm(ly), &font);
            ly -= 6.0;
        }

        doc.save(&mut std::io::BufWriter::new(std::fs::File::create(path)?))?;
        Ok(())
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
        // Use the displayed NURBS (respecting knot edits) — the same transform as
        // the gel view and the unwarp — to rectify before profiling.
        let warp = self
            .fit_warp()
            .unwrap_or_else(|| a.warp_or_identity(w as u32, h as u32));
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

    /// The current working image basis. Before detection, the UI rotation is a
    /// live preview; when detection runs, that orientation is committed into
    /// this image so migration coordinates are fitted top-to-bottom.
    pub fn view_image(&self) -> Option<GrayF32> {
        self.work.clone()
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
        if n > 0 {
            self.mark_dirty();
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
        // When annotations overlap under the cursor, prefer the one already
        // selected in the list.
        let (prev_band, prev_lane) = match self.selected {
            Some(Selection::Band(id)) => (Some(id), None),
            Some(Selection::Lane(id)) => (None, Some(id)),
            None => (None, None),
        };
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
        // If the already-selected band is under the pointer it wins, even when a
        // different band sits closer (overlapping bands).
        let mut sel_band_hit: Option<(u32, f64)> = None;
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
            if dy <= tol {
                if Some(b.id) == prev_band {
                    sel_band_hit = Some((b.id, dy));
                }
                if best_band.is_none_or(|(_, bd)| dy < bd) {
                    best_band = Some((b.id, dy));
                }
            }
        }
        let best_band = sel_band_hit.or(best_band);
        // Lane fallback: prefer the selected lane if the pointer is still inside
        // it (overlapping lanes), otherwise the lane under the pointer.
        let lane_hit = prev_lane
            .filter(|&id| {
                a.lanes.iter().any(|l| {
                    l.id == id && {
                        let (x0, x1) = l.px_x_bounds(&warp);
                        px >= x0 as f64 && px <= x1 as f64
                    }
                })
            })
            .or_else(|| lane_at(px));
        if let Some((id, _)) = best_band {
            self.selected = Some(Selection::Band(id));
            self.dragging = true;
            true
        } else if let Some(id) = lane_hit {
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
        // Real gels are only slightly tilted; a wide search invites false maxima.
        let est = opengel::detect::orient::estimate_rotation(w, 15.0, true);
        self.rotation_deg = -est;
        format!("Auto-straighten applied {:.1}°.", self.rotation_deg)
    }

    /// Ladder template names applicable to the current gel type.
    /// Ladder options for the dialog. The built-in templates first, then "Custom"
    /// at the end — Custom marks the lane as a ladder without assigning any rungs,
    /// so the user sets each band's weight manually (via "Set weight…").
    pub fn ladder_names(&self) -> Vec<String> {
        self.ladder_names_for_vendor(self.gel_type, None)
    }

    /// Vendors offering ladders for `gel_type`, with "Recent" first (when the
    /// user has used any of that type) and "Custom" last.
    ///
    /// Takes the type explicitly rather than reading `self.gel_type`: the ladder
    /// dialog lets the user browse a *different* category than the document is
    /// currently flagged as — picking a protein ladder is how a mislabelled
    /// document gets corrected to protein.
    pub fn ladder_vendor_names(&self, gel_type: GelType) -> Vec<String> {
        let mut vendors = Vec::new();
        let recent = self.ladder_names_for_vendor(gel_type, Some("Recent"));
        if !recent.is_empty() {
            vendors.push("Recent".to_string());
        }
        for template in ladders::for_gel_type(gel_type) {
            let vendor = template.vendor.as_deref().unwrap_or("Other");
            if !vendors.iter().any(|v| v == vendor) {
                vendors.push(vendor.to_string());
            }
        }
        vendors.push("Custom".to_string());
        vendors
    }

    pub fn ladder_names_for_vendor(&self, gel_type: GelType, vendor: Option<&str>) -> Vec<String> {
        let all: Vec<String> = ladders::for_gel_type(gel_type)
            .iter()
            .filter(|t| match vendor {
                Some("Recent") => self.recent_ladders.iter().any(|n| n == &t.name),
                Some("Custom") => false,
                Some(vendor) => t.vendor.as_deref().unwrap_or("Other") == vendor,
                None => true,
            })
            .map(|t| t.name.clone())
            .collect();
        let mut v = Vec::with_capacity(all.len() + usize::from(vendor.is_none()));
        if vendor.is_none() || vendor == Some("Recent") {
            for name in &self.recent_ladders {
                if all.iter().any(|n| n == name) && !v.iter().any(|n| n == name) {
                    v.push(name.clone());
                }
            }
        }
        for name in all {
            if !v.iter().any(|n| n == &name) {
                v.push(name);
            }
        }
        if vendor.is_none() || vendor == Some("Custom") {
            v.push("Custom (set weights manually)".to_string());
        }
        v
    }

    pub fn ladder_dialog_options_for_vendor_index(
        &self,
        gel_type: GelType,
        vendor_index: usize,
    ) -> (Vec<String>, Vec<String>) {
        let vendors = self.ladder_vendor_names(gel_type);
        let names = vendors
            .get(vendor_index)
            .map(|vendor| self.ladder_names_for_vendor(gel_type, Some(vendor)))
            .unwrap_or_else(|| self.ladder_names_for_vendor(gel_type, None));
        (vendors, names)
    }

    /// Gel-type names for the document and ladder-type selectors.
    pub fn gel_type_names() -> Vec<String> {
        GelType::ALL.iter().map(|t| t.label().to_string()).collect()
    }

    /// Flag the open document as DNA / RNA / protein.
    ///
    /// The type drives the size unit, the molarity conversion and which ladders
    /// are offered, so a ladder fitted under the old type no longer means
    /// anything: assignments to templates of the wrong type are dropped and the
    /// sample lanes re-fitted. Band weights the user entered by hand are kept —
    /// only their unit changes.
    pub fn set_gel_type(&mut self, gel_type: GelType) -> String {
        if self.gel_type == gel_type
            && self.doc.as_ref().map(|d| d.project.gel_type) == Some(gel_type)
        {
            return format!("Gel type is already {}.", gel_type.label());
        }
        self.gel_type = gel_type;
        let Some(doc) = self.doc.as_mut() else {
            return format!("Gel type: {}.", gel_type.label());
        };
        doc.project.gel_type = gel_type;
        let a = &mut doc.project.analysis;
        let before = a.ladder_assignments.len();
        a.ladder_assignments.retain(|la| {
            ladders::by_name(&la.template_name).is_none_or(|t| t.gel_type == gel_type)
        });
        let dropped = before - a.ladder_assignments.len();
        if dropped > 0 {
            resize_sample_lanes(a);
        }
        self.mark_dirty();
        match dropped {
            0 => format!("Gel type: {} ({}).", gel_type.label(), gel_type.size_unit()),
            1 => format!(
                "Gel type: {} ({}). Dropped 1 ladder assignment of the previous type.",
                gel_type.label(),
                gel_type.size_unit()
            ),
            n => format!(
                "Gel type: {} ({}). Dropped {n} ladder assignments of the previous type.",
                gel_type.label(),
                gel_type.size_unit()
            ),
        }
    }

    pub fn set_recent_ladders(&mut self, names: Vec<String>) {
        self.recent_ladders = crate::config::sanitize_recent_ladders(names);
    }

    pub fn remember_ladder(&mut self, name: &str) {
        crate::config::remember_ladder(&mut self.recent_ladders, name);
    }

    /// Prefill `(value, unit)` for the set-band-weight dialog from the selected
    /// band's current known size.
    pub fn weight_dialog_prefill(&self) -> (String, String) {
        let unit = self.gel_type.size_unit().to_string();
        let value = match self.selected {
            Some(Selection::Band(id)) => self
                .analysis()
                .and_then(|a| a.bands.iter().find(|b| b.id == id))
                .and_then(|b| b.known_size)
                .map(|s| format!("{s:.0}"))
                .unwrap_or_default(),
            _ => String::new(),
        };
        (value, unit)
    }

    /// Set the selected band's known size (weight), then re-fit sizing so the
    /// sample lanes update.
    pub fn set_selected_band_weight(&mut self, size: f64) -> String {
        let Some(Selection::Band(id)) = self.selected else {
            return "Select a band first.".into();
        };
        let Some(doc) = self.doc.as_mut() else {
            return "No document.".into();
        };
        let a = &mut doc.project.analysis;
        let Some(band) = a.bands.iter_mut().find(|b| b.id == id) else {
            return "No such band.".into();
        };
        band.known_size = Some(size);
        band.merged_sizes.clear(); // an explicit weight overrides any merge label
        resize_sample_lanes(a);
        self.mark_dirty();
        format!(
            "Set band weight to {size:.0} {}.",
            self.gel_type.size_unit()
        )
    }

    // ---- ladder lanes (any number, individually tunable) ----

    /// Prefill values for the "Use as ladder" dialog for a lane:
    /// `(lane_name, type_index, vendor_index, template_index, volume_ul,
    /// conc_ng_ul)`. `template_index` is the lane's currently assigned template
    /// (or 0 if none); `type_index` is that template's category, falling back to
    /// the document's own gel type.
    pub fn ladder_dialog_prefill(&self, lane_id: u32) -> (String, i32, i32, i32, f64, f64) {
        let name = self
            .analysis()
            .and_then(|a| a.lanes.iter().find(|l| l.id == lane_id))
            .and_then(|l| l.label.clone())
            .unwrap_or_else(|| format!("Lane {lane_id}"));
        let template_name = self
            .analysis()
            .and_then(|a| a.ladder_assignments.iter().find(|la| la.lane_id == lane_id))
            .map(|la| la.template_name.clone());
        let template = template_name.as_deref().and_then(ladders::by_name);
        let gel_type = template.map(|t| t.gel_type).unwrap_or(self.gel_type);
        let vendors = self.ladder_vendor_names(gel_type);
        let mut vidx = 0usize;
        let mut tidx = 0usize;
        if let Some(template_name) = template_name {
            if let Some(template) = template {
                let vendor = template.vendor.as_deref().unwrap_or("Other");
                if let Some(pos) = vendors.iter().position(|v| v == vendor) {
                    vidx = pos;
                }
            }
            let names = vendors
                .get(vidx)
                .map(|vendor| self.ladder_names_for_vendor(gel_type, Some(vendor)))
                .unwrap_or_else(|| self.ladder_names_for_vendor(gel_type, None));
            if let Some(pos) = names.iter().position(|n| *n == template_name) {
                tidx = pos;
            }
        }
        (
            name,
            gel_type.index() as i32,
            vidx as i32,
            tidx as i32,
            self.ladder_volume(lane_id),
            self.ladder_conc(lane_id),
        )
    }

    /// Apply the dialog's selection to a lane.
    ///
    /// `gel_type` is the category the dialog was browsing. It wins over the
    /// document's current flag: choosing a protein ladder on a gel that opened
    /// as DNA means the document was mislabelled, so the document is re-flagged
    /// before the ladder is applied.
    pub fn apply_ladder_dialog_by_name(
        &mut self,
        lane_id: u32,
        gel_type: GelType,
        template_name: &str,
        volume_ul: f64,
        conc_ng_ul: f64,
    ) -> String {
        let retype = if self.gel_type == gel_type {
            String::new()
        } else {
            format!("{} ", self.set_gel_type(gel_type))
        };
        format!(
            "{retype}{}",
            self.apply_ladder_to_lane_by_name(lane_id, template_name, volume_ul, conc_ng_ul)
        )
    }

    fn apply_ladder_to_lane_by_name(
        &mut self,
        lane_id: u32,
        template_name: &str,
        volume_ul: f64,
        conc_ng_ul: f64,
    ) -> String {
        let valid_lane = self
            .analysis()
            .is_some_and(|a| a.lanes.iter().any(|l| l.id == lane_id));
        if !valid_lane {
            return "No such lane.".into();
        }
        if !template_name.starts_with("Custom") && ladders::by_name(template_name).is_none() {
            return format!("Unknown ladder {template_name}.");
        }
        self.set_ladder_amounts(lane_id, volume_ul, conc_ng_ul);
        let msg = self.set_lane_ladder_by_name(lane_id, template_name);
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
            self.selected = match self.selected {
                Some(Selection::Band(id)) if id == band_id => None,
                other => other,
            };
            if self.ratio_a == Some(band_id) {
                self.ratio_a = None;
            }
            if self.ratio_b == Some(band_id) {
                self.ratio_b = None;
            }
            self.mark_dirty();
            format!("Deleted band {band_id}.")
        } else {
            "No such band.".into()
        }
    }

    pub fn set_lane_ladder_by_name(&mut self, lane_id: u32, name: &str) -> String {
        // "Custom": mark as a ladder but assign no rungs (the user sets each
        // band's weight manually).
        if name.starts_with("Custom") {
            let Some(doc) = self.doc.as_mut() else {
                return "No document.".into();
            };
            match doc
                .project
                .analysis
                .lanes
                .iter_mut()
                .find(|l| l.id == lane_id)
            {
                Some(lane) => {
                    lane.is_ladder = true;
                    self.mark_dirty();
                    return format!("Lane {lane_id} = custom ladder (set band weights manually).");
                }
                None => return "No such lane.".into(),
            }
        }
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
                self.remember_ladder(name);
                self.mark_dirty();
                format!("Lane {lane_id} = {name} ({n} rungs matched).")
            }
            None => format!("Lane {lane_id}: could not match {name} to its bands."),
        }
    }

    /// Replace the current project with the synthetic demo gel (8 lanes, three
    /// NEB 1 kb ladders) with its aligned annotation, then measure the bands.
    pub fn load_demo(&mut self) -> String {
        let doc = opengel::core::demo::demo_document_annotated();
        self.replace_active_with_document(doc, None, "Demo gel");
        let msg = self.measure_regions();
        format!("Loaded demo gel (8 lanes, 3 ladders). {msg}")
    }

    pub fn open_path(&mut self, path: &Path) -> Result<()> {
        if self
            .source_path
            .as_deref()
            .is_some_and(|p| Self::paths_same(p, path))
        {
            return Ok(());
        }
        for i in 0..self.documents.len() {
            let is_match = self.documents[i]
                .as_ref()
                .and_then(|d| d.source_path.as_deref())
                .is_some_and(|p| Self::paths_same(p, path));
            if is_match {
                let _ = self.set_active_document(i);
                return Ok(());
            }
        }
        let doc = if opengel::core::scn::has_scn_extension(path) {
            GelDocument::load_scn(path)
                .with_context(|| format!("reading Image Lab scan {}", path.display()))?
        } else {
            GelDocument::load(path).with_context(|| format!("loading {}", path.display()))?
        };
        // A source that asked for a white background gets one. This is the only
        // place the preference is applied: it is how the document should first
        // appear, not a lock — the user's own invert toggle still rules after.
        let display_inverted = doc.project.display_inverted;
        self.replace_active_with_document(doc, Some(path.to_path_buf()), "Untitled gel");
        if display_inverted {
            self.set_invert(true);
        }
        Ok(())
    }

    pub fn save_path(&mut self, path: &Path) -> Result<()> {
        let doc = self
            .doc
            .as_ref()
            .ok_or_else(|| anyhow!("nothing to save"))?;
        doc.save(path)?;
        self.dirty = false;
        Ok(())
    }

    /// Save the document to `path` and remember it as the current file, so later
    /// saves default to the same place.
    pub fn save_as(&mut self, path: &Path) -> Result<()> {
        self.save_path(path)?;
        self.source_path = Some(path.to_path_buf());
        self.current_label = Self::display_name_for(Some(path), "gel.gel.zip");
        self.dirty = false;
        if let Some(idx) = self.active_document {
            if let Some(Some(doc)) = self.documents.get_mut(idx) {
                doc.source_path = Some(path.to_path_buf());
                doc.display_name = Self::display_name_for(Some(path), "gel.gel.zip");
                doc.dirty = false;
            }
        }
        Ok(())
    }

    /// Default file name for the "Save as…" dialog: the current file's name, or a
    /// sensible fallback.
    pub fn save_dialog_filename(&self) -> String {
        self.source_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .filter(|n| !n.trim().is_empty())
            .unwrap_or("gel.gel.zip")
            .to_string()
    }

    /// Default filename for the trace-PDF export: derived from the loaded/saved
    /// gel file's base name (without the `.gel.zip`/`.zip` extension), else a
    /// generic name.
    pub fn trace_pdf_filename(&self) -> String {
        let base = self
            .source_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(|n| {
                n.strip_suffix(".gel.zip")
                    .or_else(|| n.strip_suffix(".zip"))
                    .unwrap_or(n)
            })
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("trace");
        format!("{base}_trace.pdf")
    }

    /// Recompute the HDR merge from the exposure bracket with the current option
    /// toggles ([`hdr_bias_subtraction`], [`hdr_align`], [`hdr_deghost`]). The
    /// result becomes the working image and is stored on the document so it is
    /// persisted to the `.gel.zip` on save. Returns a status message.
    pub fn recompute_hdr(&mut self) -> String {
        use opengel::core::hdr::{merge_hdr_with, HdrOptions};
        use opengel::core::model::HdrRecord;
        use std::collections::BTreeMap;

        let opts = HdrOptions {
            bias_subtraction: self.hdr_bias_subtraction,
            align: self.hdr_align,
            deghost: self.hdr_deghost,
        };
        let Some(doc) = self.doc.as_mut() else {
            return "No document.".into();
        };
        // Largest bracket group (shared bracket_group, >1 frame).
        let mut groups: BTreeMap<Option<u32>, Vec<usize>> = BTreeMap::new();
        for (i, img) in doc.project.images.iter().enumerate() {
            groups.entry(img.meta.bracket_group).or_default().push(i);
        }
        let bracket = groups
            .into_iter()
            .filter(|(k, v)| k.is_some() && v.len() > 1)
            .max_by_key(|(_, v)| v.len());
        let Some((_, idxs)) = bracket else {
            return "No exposure bracket to merge (need ≥2 bracketed frames).".into();
        };
        let frames: Vec<GrayF32> = idxs
            .iter()
            .map(|&i| GrayF32::from_dynamic(&doc.frames[i]))
            .collect();
        let exposures: Vec<f64> = idxs
            .iter()
            .map(|&i| doc.project.images[i].meta.exposure_seconds)
            .collect();
        if exposures.iter().any(|&t| t <= 0.0) {
            return "Bracket frames are missing exposure times.".into();
        }
        let merged = match merge_hdr_with(&frames, &exposures, &opts) {
            Ok(m) => m,
            Err(e) => return format!("HDR merge failed: {e}"),
        };
        // Radiance scale for persisting the merge as a normalized 16-bit PNG.
        let scale = (merged.data.iter().cloned().fold(0.0f32, f32::max) as f64).max(1e-6);
        doc.project.hdr = Some(HdrRecord {
            options: opts,
            scale,
        });
        doc.merged = Some(merged.clone());
        self.work = Some(merged);
        self.view_frame = None; // show the merged image
        self.bump_doc_gen();
        self.mark_dirty();

        let mut parts = Vec::new();
        if opts.bias_subtraction {
            parts.push("bias");
        }
        if opts.align {
            parts.push("align");
        }
        if opts.deghost {
            parts.push("de-ghost");
        }
        let applied = if parts.is_empty() {
            "plain".to_string()
        } else {
            parts.join("+")
        };
        format!("Recomputed HDR from {} frames ({applied}).", idxs.len())
    }

    pub fn email_attachment(&self) -> Result<(String, Vec<u8>)> {
        let doc = self
            .doc
            .as_ref()
            .ok_or_else(|| anyhow!("nothing to email"))?;
        let name = self
            .source_path
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("gel.gel.zip")
            .to_string();
        Ok((name, doc.to_bytes()?))
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
        self.replace_active_with_document(doc, None, "Captured gel");
        Ok(format!("Captured {n}-frame HDR bracket from {source}."))
    }

    /// Run detection + ladder ID + sizing. If `force_template` is set, only that
    /// template is considered (min_r2 relaxed so the user's choice wins).
    pub fn analyze(&mut self, force_template: Option<&str>) -> Result<String> {
        let source = self
            .work
            .clone()
            .ok_or_else(|| anyhow!("no image loaded"))?;
        let orientation_deg = self.rotation_deg;
        let work = if orientation_deg.abs() < 1e-3 {
            source.clone()
        } else {
            source.rotated(orientation_deg)
        };
        let work = &work;
        // Honor the "Optical-flow dewarp" toggle: without it the warp comes from
        // ladder-rung smile fitting (needs a matched ladder across lanes), which
        // falls back to a near-identity coarse warp when no smile is recovered.
        let params = DetectParams {
            optical_flow_warp: self.optical_flow,
            flow_smoothness: self.flow_smoothness.max(0.0),
            extra_vertical_edges: self.extra_vertical_edges,
            extra_horizontal_edges: self.extra_horizontal_edges,
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
        let mut analysis = if self.use_gelgenie_ml {
            #[cfg(feature = "gelgenie-ml")]
            {
                use opengel::detect::detector::GelDetector;

                let runtime =
                    opengel::detect::GelGenieRuntime::from_index(self.gelgenie_runtime_index);
                let detector = opengel::detect::GelGenieDetector::new(runtime)?;
                let det = detector.detect(work, &params);
                opengel::detect::analyze_detection(
                    det,
                    work,
                    self.gel_type,
                    &params,
                    &candidates,
                    min_r2,
                )
            }
            #[cfg(not(feature = "gelgenie-ml"))]
            {
                return Err(anyhow!(
                    "GelGenie ML support was not compiled in; rebuild with --features gelgenie-ml"
                ));
            }
        } else {
            opengel::detect::analyze(work, self.gel_type, &params, &candidates, min_r2)
        };
        if orientation_deg.abs() >= 1e-3 {
            transform_analysis_from_oriented(
                &mut analysis,
                orientation_deg,
                source.width(),
                source.height(),
            );
        }
        let n_lanes = analysis.lanes.len();
        let n_bands = analysis.bands.len();
        let ladder = analysis
            .ladder_assignments
            .first()
            .map(|a| format!(", ladder: {} (lane {})", a.template_name, a.lane_id))
            .unwrap_or_default();

        let doc = self.doc.as_mut().ok_or_else(|| anyhow!("no document"))?;
        doc.project.analysis = analysis;
        self.warp_edit = None;
        self.dragging_knot = None;
        self.warp_drag_base = None;
        self.bump_doc_gen();
        self.mark_dirty();
        let detector = if self.use_gelgenie_ml {
            #[cfg(feature = "gelgenie-ml")]
            {
                format!(
                    "GelGenie ML ({})",
                    opengel::detect::GelGenieRuntime::from_index(self.gelgenie_runtime_index)
                        .label()
                )
            }
            #[cfg(not(feature = "gelgenie-ml"))]
            {
                "GelGenie ML".to_string()
            }
        } else {
            "classical".to_string()
        };
        Ok(format!(
            "Detected {n_lanes} lanes, {n_bands} bands{ladder} using {detector}. Adjust the NURBS knots as needed."
        ))
    }

    // ---- interactive editing (coordinates are normalized [0,1] over the
    // displayed/rotated image) ----

    fn with_analysis_mut<F: FnOnce(&mut Analysis, &GrayF32) -> String>(&mut self, f: F) -> String {
        let Some(img) = self.work.clone() else {
            return "No image loaded.".into();
        };
        let Some(doc) = self.doc.as_mut() else {
            return "No document.".into();
        };
        let msg = f(&mut doc.project.analysis, &img);
        self.mark_dirty();
        msg
    }

    /// Default name for a new lane — "Lane N" where N is the next lane id.
    pub fn add_lane_dialog_prefill(&self) -> String {
        let next = self
            .analysis()
            .and_then(|a| a.lanes.iter().map(|l| l.id).max())
            .map_or(0, |m| m + 1);
        format!("Lane {next}")
    }

    /// Add a lane, placed **left-to-right** in the first horizontal slot that
    /// doesn't overlap an existing lane (fills gaps from the left, else appends
    /// to the right).
    pub fn add_lane(&mut self, label: Option<String>) -> String {
        self.with_analysis_mut(|a, img| {
            let (w, h) = (img.width() as u32, img.height() as u32);
            let warp = a.warp_or_identity(w, h);
            // Lane half-width in u, from a default pixel width at the gel centre.
            let halfpx = (0.04 * w as f64).max(4.0);
            let cxpx = w as f64 / 2.0;
            let (ul, _) = warp.invert((cxpx - halfpx).max(0.0), 0.0);
            let (ur, _) = warp.invert((cxpx + halfpx).min(w as f64), 0.0);
            let uhalf = ((ur - ul).abs() / 2.0).clamp(0.01, 0.45);
            let gap = uhalf * 0.3;
            // Sweep left→right, jumping past any lane the candidate would overlap.
            let mut lanes: Vec<(f64, f64)> = a.lanes.iter().map(|l| (l.u_min, l.u_max)).collect();
            lanes.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            let mut c = uhalf;
            for (lmin, lmax) in &lanes {
                if c + uhalf + gap > *lmin && c - uhalf - gap < *lmax {
                    c = lmax + gap + uhalf;
                }
            }
            c = c.clamp(uhalf, 1.0 - uhalf);
            let id = a.lanes.iter().map(|l| l.id).max().map_or(0, |m| m + 1);
            // A label that's just the default "Lane N" stays None so downstream
            // display keeps using the derived name.
            let label = label.filter(|s| !s.trim().is_empty() && s.trim() != format!("Lane {id}"));
            a.lanes.push(Lane {
                id,
                u_min: c - uhalf,
                u_max: c + uhalf,
                label,
                is_ladder: false,
            });
            format!("Added lane {id}.")
        })
    }

    /// Add a band to the currently selected lane (or the selected band's lane),
    /// at the lane centre, placed **top-to-bottom** in the first migration slot
    /// that doesn't overlap an existing band in that lane. Falls back to a hint
    /// if nothing is selected.
    pub fn add_band_to_selected(&mut self) -> String {
        let lane_id = match self.selected {
            Some(Selection::Lane(id)) => Some(id),
            Some(Selection::Band(bid)) => self
                .analysis()
                .and_then(|a| a.bands.iter().find(|b| b.id == bid).map(|b| b.lane_id)),
            None => None,
        };
        let Some(lane_id) = lane_id else {
            return "Select a lane first, then Add band.".into();
        };
        let Some((nx, v)) = self.analysis().and_then(|a| {
            let lane = a.lanes.iter().find(|l| l.id == lane_id)?;
            let nx = (lane.u_min + lane.u_max) / 2.0;
            // Existing bands in this lane, sorted by migration (top→bottom).
            let mut bands: Vec<(f64, f64)> = a
                .bands
                .iter()
                .filter(|b| b.lane_id == lane_id)
                .map(|b| (b.v_center, b.v_half_width))
                .collect();
            bands.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            // Sweep top→bottom, jumping past any band the candidate would overlap.
            let half = 0.02;
            let gap = 0.008;
            let mut v = half;
            for (bc, bh) in &bands {
                if v + half + gap > bc - bh && v - half - gap < bc + bh {
                    v = bc + bh + gap + half;
                }
            }
            Some((nx, v.clamp(half, 1.0 - half)))
        }) else {
            return "No such lane.".into();
        };
        self.add_band_at(nx, v)
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
                channel_density: Vec::new(),
                size: None,
                known_size: None,
                angle: 0.0,
                merged_sizes: Vec::new(),
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

/// Display label for a ladder band's size. A plain band shows "N unit"; a merged
/// blob (two rungs too close to resolve) shows all its rungs largest-first, e.g.
/// "10000 + 8000 bp".
fn merged_size_label(known: f64, merged: &[f64], unit: &str) -> String {
    if merged.is_empty() {
        return format!("{known:.0} {unit}");
    }
    let mut sizes: Vec<f64> = std::iter::once(known)
        .chain(merged.iter().copied())
        .collect();
    sizes.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let joined = sizes
        .iter()
        .map(|s| format!("{s:.0}"))
        .collect::<Vec<_>>()
        .join(" + ");
    format!("{joined} {unit}")
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

fn transform_analysis_from_oriented(
    analysis: &mut Analysis,
    orientation_deg: f64,
    width: usize,
    height: usize,
) {
    let (w, h) = (width as f64, height as f64);
    if let Some(warp) = analysis.warp.as_mut() {
        for ctrl in &mut warp.ctrl {
            let (x, y) = oriented_to_source(ctrl[0], ctrl[1], orientation_deg, w, h);
            *ctrl = [x, y];
        }
    }
    let angle_rad = orientation_deg.to_radians();
    for band in &mut analysis.bands {
        band.angle = normalize_angle_rad(band.angle - angle_rad);
    }
    for blob in &mut analysis.blobs {
        let corners = [
            (blob.x_min as f64, blob.y_min as f64),
            (blob.x_max as f64, blob.y_min as f64),
            (blob.x_max as f64, blob.y_max as f64),
            (blob.x_min as f64, blob.y_max as f64),
        ];
        let mut x_min = f64::INFINITY;
        let mut x_max = f64::NEG_INFINITY;
        let mut y_min = f64::INFINITY;
        let mut y_max = f64::NEG_INFINITY;
        for (x, y) in corners {
            let (sx, sy) = oriented_to_source(x, y, orientation_deg, w, h);
            x_min = x_min.min(sx);
            x_max = x_max.max(sx);
            y_min = y_min.min(sy);
            y_max = y_max.max(sy);
        }
        blob.x_min = x_min.floor().clamp(0.0, w) as u32;
        blob.x_max = x_max.ceil().clamp(0.0, w) as u32;
        blob.y_min = y_min.floor().clamp(0.0, h) as u32;
        blob.y_max = y_max.ceil().clamp(0.0, h) as u32;
    }
}

fn oriented_to_source(x: f64, y: f64, orientation_deg: f64, width: f64, height: f64) -> (f64, f64) {
    if width <= 0.0 || height <= 0.0 {
        return (x, y);
    }
    let cx = (width - 1.0) / 2.0;
    let cy = (height - 1.0) / 2.0;
    let rad = orientation_deg.to_radians();
    let (c, s) = (rad.cos(), rad.sin());
    let dx = x - cx;
    let dy = y - cy;
    (cx + dx * c + dy * s, cy - dx * s + dy * c)
}

fn normalize_angle_rad(angle: f64) -> f64 {
    let pi = std::f64::consts::PI;
    (angle + pi).rem_euclid(2.0 * pi) - pi
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
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_gel_path(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("opengel-{name}-{nonce}.gel.zip"))
    }

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
    fn multiple_open_files_switch_preserves_document_state() {
        let path_a = temp_gel_path("a");
        let path_b = temp_gel_path("b");

        let mut seed = AppState::new();
        seed.load_demo();
        seed.save_path(&path_a).unwrap();
        seed.capture().unwrap();
        seed.save_path(&path_b).unwrap();

        let mut st = AppState::new();
        st.open_path(&path_a).unwrap();
        assert_eq!(st.document_labels().len(), 1);
        assert_eq!(st.active_document_index(), 0);
        st.set_trace_mode(1);
        st.toggle_lane(0);
        st.set_invert(true);

        st.open_path(&path_b).unwrap();
        assert_eq!(st.document_labels().len(), 2);
        assert_eq!(st.active_document_index(), 1);
        assert_eq!(st.trace_mode, TraceMode::Intensity);
        assert!(!st.invert);
        st.set_trace_mode(2);

        let msg = st.set_active_document(0);
        assert!(msg.contains(path_a.file_name().unwrap().to_str().unwrap()));
        assert_eq!(st.source_path.as_deref(), Some(path_a.as_path()));
        assert_eq!(st.trace_mode, TraceMode::Ng);
        assert!(st.invert);
        assert!(st.selected_lanes.contains(&0));

        st.set_active_document(1);
        assert_eq!(st.source_path.as_deref(), Some(path_b.as_path()));
        assert_eq!(st.trace_mode, TraceMode::Molarity);
        assert!(!st.invert);

        st.open_path(&path_a).unwrap();
        assert_eq!(st.document_labels().len(), 2);
        assert_eq!(st.active_document_index(), 0);

        let msg = st.close_active_document();
        assert!(msg.contains("Closed"));
        assert_eq!(st.document_labels().len(), 1);
        assert_eq!(st.active_document_index(), 0);
        assert_eq!(st.source_path.as_deref(), Some(path_b.as_path()));

        st.close_active_document();
        assert!(st.document_labels().is_empty());
        assert_eq!(st.active_document_index(), -1);
        assert!(!st.has_open_file());

        let _ = std::fs::remove_file(path_a);
        let _ = std::fs::remove_file(path_b);
    }

    #[test]
    fn document_generations_stay_unique_after_switching() {
        let mut st = AppState::new();
        st.load_demo();
        let gen_a = st.doc_gen;
        st.capture().unwrap();
        let gen_b = st.doc_gen;
        assert_ne!(gen_a, gen_b);

        st.set_active_document(0);
        assert_eq!(st.doc_gen, gen_a);
        st.capture().unwrap();
        let gen_c = st.doc_gen;
        assert_ne!(gen_c, gen_a);
        assert_ne!(gen_c, gen_b);
    }

    #[test]
    fn dirty_flag_tracks_edits_and_successful_saves() {
        let path = temp_gel_path("dirty");
        let mut st = AppState::new();
        st.capture().unwrap();
        assert!(st.active_document_dirty());
        assert!(st.document_labels()[0].ends_with(" *"));

        st.save_as(&path).unwrap();
        assert!(!st.active_document_dirty());
        assert!(!st.document_labels()[0].ends_with(" *"));
        assert_eq!(st.source_path.as_deref(), Some(path.as_path()));

        st.add_lane(None);
        assert!(st.active_document_dirty());
        st.save_path(&path).unwrap();
        assert!(!st.active_document_dirty());

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn analyze_invalidates_manual_warp_and_delete_lane_cleans_dependents() {
        let mut st = AppState::new();
        st.capture().unwrap();
        st.analyze(None).unwrap();

        let img = st.work.as_ref().unwrap();
        let (w, h) = (img.width() as f64, img.height() as f64);
        st.warp_edit = Some((st.doc_gen, GelWarp::identity_grid(w, h, 4, 5)));
        let gen_before = st.doc_gen;
        st.analyze(None).unwrap();
        assert!(st.warp_edit.is_none());
        assert_ne!(st.doc_gen, gen_before);

        st.calibrate(10.0);
        let lane_id = st.analysis().unwrap().lanes[0].id;
        let deleted_bands: std::collections::BTreeSet<u32> = st
            .analysis()
            .unwrap()
            .bands
            .iter()
            .filter(|b| b.lane_id == lane_id)
            .map(|b| b.id)
            .collect();
        let band_id = *deleted_bands.iter().next().expect("band in deleted lane");
        st.selected = Some(Selection::Band(band_id));
        st.ratio_a = Some(band_id);
        st.ratio_b = Some(band_id);
        st.collapsed_lanes.insert(lane_id);

        st.delete_lane(lane_id);
        let a = st.analysis().unwrap();
        assert!(!a.lanes.iter().any(|l| l.id == lane_id));
        assert!(!a.bands.iter().any(|b| b.lane_id == lane_id));
        assert!(!a
            .quantifications
            .iter()
            .any(|q| deleted_bands.contains(&q.target_id)));
        assert!(st.selected.is_none());
        assert!(st.ratio_a.is_none());
        assert!(st.ratio_b.is_none());
        assert!(!st.collapsed_lanes.contains(&lane_id));
    }

    #[test]
    fn edit_add_delete_and_calibrate() {
        let mut st = AppState::new();
        st.capture().unwrap();
        st.analyze(None).unwrap();
        let before = st.analysis().unwrap().bands.len();

        // Add a lane then a band in it.
        st.add_lane(None);
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
    fn added_lanes_do_not_overlap_and_go_left_to_right() {
        let mut st = AppState::new();
        st.capture().unwrap();
        // Start from a clean slate so placement is deterministic.
        st.doc.as_mut().unwrap().project.analysis = Analysis::default();

        for _ in 0..6 {
            st.add_lane(None);
        }
        let mut lanes: Vec<(f64, f64)> = st
            .analysis()
            .unwrap()
            .lanes
            .iter()
            .map(|l| (l.u_min, l.u_max))
            .collect();
        lanes.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        // Each lane sits strictly to the right of the previous, no overlap.
        for w in lanes.windows(2) {
            assert!(
                w[0].1 <= w[1].0 + 1e-9,
                "lanes overlap: {:?} then {:?}",
                w[0],
                w[1]
            );
        }
        // All within the image.
        assert!(lanes.first().unwrap().0 >= -1e-9 && lanes.last().unwrap().1 <= 1.0 + 1e-9);
    }

    #[test]
    fn added_bands_do_not_overlap_and_go_top_to_bottom() {
        let mut st = AppState::new();
        st.capture().unwrap();
        st.doc.as_mut().unwrap().project.analysis = Analysis::default();

        st.add_lane(None);
        let lane_id = st.analysis().unwrap().lanes[0].id;
        st.selected = Some(Selection::Lane(lane_id));
        for _ in 0..5 {
            st.add_band_to_selected();
        }
        let mut bands: Vec<(f64, f64)> = st
            .analysis()
            .unwrap()
            .bands
            .iter()
            .filter(|b| b.lane_id == lane_id)
            .map(|b| (b.v_center, b.v_half_width))
            .collect();
        bands.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        assert_eq!(bands.len(), 5);
        // Each band sits below the previous with no overlap of their extents.
        for w in bands.windows(2) {
            let prev_bottom = w[0].0 + w[0].1;
            let next_top = w[1].0 - w[1].1;
            assert!(
                prev_bottom <= next_top + 1e-9,
                "bands overlap: {:?} then {:?}",
                w[0],
                w[1]
            );
        }
    }

    #[test]
    fn rotation_and_straighten() {
        let mut st = AppState::new();
        st.capture().unwrap();
        st.set_rotation(12.0);
        let msg = st.auto_straighten();
        assert!(msg.contains("Auto-straighten"));
        // Mock capture is upright, so straightening should settle near 0°.
        assert!(st.rotation_deg.abs() < 6.0, "rotation {}", st.rotation_deg);
    }

    #[test]
    fn analyze_accounts_for_orientation_without_mutating_work() {
        let mut st = AppState::new();
        st.capture().unwrap();
        let before = st.work.as_ref().unwrap().data.clone();
        st.set_rotation(180.0);
        st.analyze(None).unwrap();
        assert_eq!(st.rotation_deg, 180.0);
        assert_eq!(st.work.as_ref().unwrap().data, before);
        assert!(st.analysis().is_some_and(|a| !a.lanes.is_empty()));
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
        assert!(st.hover_size_label(0.5, 0.5).is_none());

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
        st.set_lane_ladder_by_name(ladder_lane, "NEB 1 kb DNA Ladder");

        // With a fitted ladder, hovering yields a size readout that decreases
        // as the cursor moves down the gel (larger fragments migrate less).
        let top = st.hover_size_label(0.5, 0.2).expect("bp readout near top");
        let bot = st
            .hover_size_label(0.5, 0.8)
            .expect("bp readout near bottom");
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
        assert!(st.hover_size_label(0.5, -0.1).is_none());
        assert!(st.hover_size_label(0.5, 1.5).is_none());
    }

    #[test]
    fn a_channels_hdr_bracket_is_geometric() {
        let mut st = AppState::new();
        st.set_capture_mode_idx(1); // HDR
        st.geldoc.set_hdr_min_s(0.01);
        st.geldoc.set_hdr_max_s(1.0);
        st.set_hdr_steps_idx(1); // → 3 steps
        let e = st.geldoc.channel().exposures();
        assert_eq!(e.len(), 3);
        assert!((e[0] - 0.01).abs() < 1e-9);
        assert!((e[1] - 0.1).abs() < 1e-9, "mid = {}", e[1]); // geometric mean
        assert!((e[2] - 1.0).abs() < 1e-9);
        // Log-even: constant ratio between consecutive exposures.
        assert!(((e[1] / e[0]) - (e[2] / e[1])).abs() < 1e-9);

        // Single mode = one frame at that channel's exposure.
        st.set_capture_mode_idx(0);
        st.geldoc.set_exposure_s(0.25);
        assert_eq!(st.geldoc.channel().exposures(), vec![0.25]);

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
        st.geldoc.set_exposure_s(0.05);
        st.set_hdr_lower_from_current();
        assert!((st.geldoc.channel().hdr_min_s - 0.05).abs() < 1e-9);
        st.geldoc.set_exposure_s(1.5);
        st.set_hdr_upper_from_current();
        assert!((st.geldoc.channel().hdr_max_s - 1.5).abs() < 1e-9);
        // Bounds never cross.
        assert!(st.geldoc.channel().hdr_min_s <= st.geldoc.channel().hdr_max_s);
    }

    #[test]
    fn the_live_view_follows_the_selected_channel() {
        // The Live tab and the Gel Doc EZ tab edit the same per-channel
        // settings, so switching channel switches which exposure is live.
        use opengel::instrument::{AcquisitionPlan, TrayType};
        let mut st = AppState::new();
        st.set_live_channel(AcquisitionPlan::index_of(TrayType::Uv));
        st.geldoc.set_exposure_s(0.02);
        st.set_live_channel(AcquisitionPlan::index_of(TrayType::White));
        st.geldoc.set_exposure_s(0.5);
        assert!((st.live_exposure_s() - 0.5).abs() < 1e-12);
        st.set_live_channel(AcquisitionPlan::index_of(TrayType::Uv));
        assert!((st.live_exposure_s() - 0.02).abs() < 1e-12);
    }

    #[test]
    fn auto_writes_its_metered_exposure_into_the_current_channel() {
        let mut st = AppState::new();
        st.auto_metering = true;
        st.apply_auto_exposure(0.08);
        assert!(!st.auto_metering, "the metering flag must be cleared");
        assert!((st.live_exposure_s() - 0.08).abs() < 1e-12);
        // The metered time is where nothing clips, so it is the bracket's short
        // end, not its middle.
        assert!((st.geldoc.channel().hdr_min_s - 0.08).abs() < 1e-12);
        assert!(st.geldoc.channel().hdr_max_s > 0.08);
    }

    #[test]
    fn warp_knots_only_when_shown_and_edit_persists() {
        let mut st = AppState::new();
        st.load_demo();

        // Hidden grid → no knots and nothing grabbable.
        assert!(st.warp_knot_items().is_empty());
        let some_pos = (0.5, 0.5);
        assert!(!st.press_warp_knot(some_pos.0, some_pos.1));

        // Shown grid → control-point handles appear.
        st.set_show_warp(true);
        let knots = st.warp_knot_items();
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
        let moved = st.warp_knot_items()[0];
        assert!(
            (moved.0 as f64 - target.0).abs() < 5e-3 && (moved.1 as f64 - target.1).abs() < 5e-3,
            "knot at {moved:?} not at target {target:?}"
        );
        let a = st.warp_knot_items();
        let b = st.warp_knot_items();
        assert_eq!(a, b, "edited warp must be stable across renders");

        // On a taller grid, dragging a top/bottom edge knot redistributes that
        // column's inner v rows linearly between the two edge knots.
        let img = st.work.as_ref().unwrap();
        let (w, h) = (img.width() as f64, img.height() as f64);
        st.warp_edit = Some((st.doc_gen, GelWarp::identity_grid(w, h, 4, 5)));
        st.normalize_inner_knots = true;
        let knots = st.warp_knot_items();
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
        let knots = st.warp_knot_items();
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
        assert!(st.hover_size_label(0.5, 0.5).is_none());
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
        let names = st.ladder_names();
        assert!(names.len() > 1);
        // All entries except the trailing "Custom" resolve to a real template.
        assert!(names[..names.len() - 1]
            .iter()
            .all(|n| opengel::core::ladders::by_name(n).is_some()));
    }

    #[test]
    fn ladder_names_put_recent_matching_gel_type_first() {
        let mut st = AppState::new();
        st.set_recent_ladders(vec![
            "Thermo PageRuler Prestained (10-180 kDa)".to_string(),
            "Takara 100 bp DNA Ladder".to_string(),
            "NEB 1 kb DNA Ladder".to_string(),
        ]);
        let names = st.ladder_names();
        assert_eq!(names[0], "Takara 100 bp DNA Ladder");
        assert_eq!(names[1], "NEB 1 kb DNA Ladder");
        assert_eq!(names.last().unwrap(), "Custom (set weights manually)");
        assert!(!names
            .iter()
            .any(|n| n == "Thermo PageRuler Prestained (10-180 kDa)"));
    }

    #[test]
    fn ladder_dialog_browses_a_type_other_than_the_documents() {
        // A DNA document can still browse the protein and RNA catalogues — that
        // is how a mis-flagged gel gets corrected.
        let st = AppState::new();
        assert_eq!(st.gel_type, GelType::Dna);
        for gel_type in [GelType::Protein, GelType::Rna] {
            let names = st.ladder_names_for_vendor(gel_type, None);
            assert!(names.len() > 1, "{gel_type:?} has no ladders");
            assert!(names[..names.len() - 1]
                .iter()
                .all(|n| ladders::by_name(n).is_some_and(|t| t.gel_type == gel_type)));
            let vendors = st.ladder_vendor_names(gel_type);
            assert_eq!(vendors.last().unwrap(), "Custom");
            let (v, n) = st.ladder_dialog_options_for_vendor_index(gel_type, 0);
            assert_eq!(v, vendors);
            assert!(!n.is_empty());
        }
    }

    #[test]
    fn setting_gel_type_reflags_the_document_and_drops_stale_ladders() {
        let mut st = AppState::new();
        st.load_demo();
        assert_eq!(st.gel_type, GelType::Dna);
        let lane_id = st.analysis().unwrap().lanes[0].id;
        st.set_lane_ladder_by_name(lane_id, "NEB 1 kb DNA Ladder");
        assert!(!st.analysis().unwrap().ladder_assignments.is_empty());

        let msg = st.set_gel_type(GelType::Protein);
        assert_eq!(st.gel_type, GelType::Protein);
        assert_eq!(st.doc.as_ref().unwrap().project.gel_type, GelType::Protein);
        // The demo's DNA ladders no longer apply to a protein gel.
        assert!(st.analysis().unwrap().ladder_assignments.is_empty());
        assert!(msg.contains("Protein"), "{msg}");
        assert!(msg.contains("Dropped"), "{msg}");
        assert!(st.dirty);
    }

    #[test]
    fn applying_a_protein_ladder_reflags_a_dna_document() {
        let mut st = AppState::new();
        st.load_demo();
        let lane_id = st.analysis().unwrap().lanes[0].id;
        let msg = st.apply_ladder_dialog_by_name(
            lane_id,
            GelType::Protein,
            "Thermo PageRuler Prestained (10-180 kDa)",
            10.0,
            50.0,
        );
        assert_eq!(st.gel_type, GelType::Protein);
        assert_eq!(st.doc.as_ref().unwrap().project.gel_type, GelType::Protein);
        assert!(msg.contains("Protein"), "{msg}");

        // Re-opening the dialog for that lane offers the protein catalogue.
        let (_, type_idx, _, _, _, _) = st.ladder_dialog_prefill(lane_id);
        assert_eq!(GelType::from_index(type_idx as usize), GelType::Protein);
    }
}
