//! Application state and the operations behind each UI callback.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use opengel::core::model::{
    Analysis, Band, Calibration, GelType, Lane, LadderAssignment, LadderTemplate, Quantification,
    TargetKind,
};
use opengel::core::quant::{compare, mass_ng_to_nmol, nmol_to_molar};
use opengel::core::{ladders, GelDocument, GrayF32};
use opengel::detect::detector::DetectParams;
use opengel::detect::ladder_match::best_template;

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
    /// Which captured frame to display: `None` = the merged HDR working image,
    /// `Some(i)` = raw frame `i`. Analysis always uses the merged image.
    pub view_frame: Option<usize>,
    /// Trace-tab state: y-axis mode and which lanes are plotted.
    pub trace_mode: TraceMode,
    pub selected_lanes: std::collections::BTreeSet<u32>,
    /// Loaded ladder mass (ng) per ladder lane. Missing entries default to
    /// [`DEFAULT_LADDER_LOAD_NG`]; each ladder lane can carry a different load.
    pub ladder_loads: std::collections::BTreeMap<u32, f64>,
    /// Lanes collapsed in the tree list (absent = expanded).
    pub collapsed_lanes: std::collections::BTreeSet<u32>,
    bracket_counter: u32,
}

/// Default ladder load (ng) assumed for a ladder lane with no explicit value.
pub const DEFAULT_LADDER_LOAD_NG: f64 = 500.0;

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
            view_frame: None,
            trace_mode: TraceMode::Intensity,
            selected_lanes: std::collections::BTreeSet::new(),
            ladder_loads: std::collections::BTreeMap::new(),
            collapsed_lanes: std::collections::BTreeSet::new(),
            bracket_counter: 0,
        }
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
            bands.sort_by(|x, y| x.y_center.partial_cmp(&y.y_center).unwrap());
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
                    Some(s) => format!("{s:.0} {unit} rung"),
                    None => "band".to_string(),
                };
                out.push(TreeRow {
                    kind: 1,
                    lane_id: lane.id,
                    band_id: b.id as i32,
                    expanded: false,
                    is_ladder: lane.is_ladder,
                    name,
                    rf: b.rf.map(|r| format!("{r:.2}")).unwrap_or_else(|| "-".into()),
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
        if let Some(lane) = doc.project.analysis.lanes.iter_mut().find(|l| l.id == lane_id) {
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
        self.ladder_loads.remove(&lane_id);
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

    /// The loaded ladder mass (ng) for a given ladder lane.
    pub fn ladder_load(&self, lane_id: u32) -> f64 {
        self.ladder_loads
            .get(&lane_id)
            .copied()
            .unwrap_or(DEFAULT_LADDER_LOAD_NG)
    }

    /// Set the loaded ladder mass (ng) for a ladder lane.
    pub fn set_ladder_load(&mut self, lane_id: u32, ng: f64) {
        self.ladder_loads.insert(lane_id, ng);
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

    /// Reset the contrast window to full range.
    pub fn reset_display_window(&mut self) {
        self.disp_lo = 0.0;
        self.disp_hi = 1.0;
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
                let label = l
                    .label
                    .clone()
                    .unwrap_or_else(|| format!("Lane {}", l.id));
                (l.id, label, l.is_ladder, self.selected_lanes.contains(&l.id))
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
            .filter_map(|b| b.known_size.map(|s| (b.y_center, s)))
            .collect();
        opengel::core::quant::SizingFit::fit(&pts)
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
        let mut out = Vec::new();
        for lane in &a.lanes {
            if !self.selected_lanes.contains(&lane.id) {
                continue;
            }
            let inten = subtract_baseline(
                &lane_row_profile(img, lane.x_min as usize, lane.x_max as usize),
                25,
            );
            let values: Vec<f64> = inten
                .iter()
                .enumerate()
                .map(|(y, &v)| match self.trace_mode {
                    TraceMode::Intensity => v,
                    TraceMode::Ng => v * slope,
                    TraceMode::Molarity => {
                        let ng = v * slope;
                        match fit.map(|f| f.size_at(y as f64)) {
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

    /// Populate a demonstration annotation (4 lanes with bands) over the current
    /// image, then measure each region. Captures a mock gel first if empty.
    pub fn demo_annotation(&mut self) -> String {
        if self.work.is_none() {
            let _ = self.capture();
        }
        let Some(img) = self.work.clone() else {
            return "No image to annotate.".into();
        };
        let (w, h) = (img.width() as f64, img.height() as f64);
        let mut a = Analysis::default();
        let cols: [(f64, bool); 4] = [(0.18, true), (0.40, false), (0.60, false), (0.80, false)];
        let mut bid = 0u32;
        for (i, (fx, is_ladder)) in cols.iter().enumerate() {
            let cx = fx * w;
            let half = (0.05 * w).max(6.0);
            a.lanes.push(Lane {
                id: i as u32,
                x_min: (cx - half).max(0.0) as u32,
                x_max: (cx + half).min(w) as u32,
                y_min: 0,
                y_max: h as u32,
                label: Some(if *is_ladder {
                    "Ladder".into()
                } else {
                    format!("Lane {i}")
                }),
                is_ladder: *is_ladder,
            });
            let ys: Vec<f64> = if *is_ladder {
                vec![0.18, 0.26, 0.36, 0.48, 0.62, 0.80]
            } else {
                vec![0.30, 0.50, 0.68]
            };
            for fy in ys {
                a.bands.push(Band {
                    id: bid,
                    lane_id: i as u32,
                    y_center: fy * h,
                    y_half_width: (0.012 * h).max(2.0),
                    integrated_density: 0.0,
                    rf: Some(fy),
                    size: None,
                    known_size: None,
                });
                bid += 1;
            }
        }
        if let Some(doc) = self.doc.as_mut() {
            doc.project.analysis = a;
        }
        let msg = self.measure_regions();
        format!("Demo annotation placed (4 lanes). {msg}")
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
        let mut prof: HashMap<u32, Vec<f64>> = HashMap::new();
        for lane in &a.lanes {
            let raw = lane_row_profile(&img, lane.x_min as usize, lane.x_max as usize);
            prof.insert(lane.id, subtract_baseline(&raw, 25));
        }
        let mut n = 0;
        for b in &mut a.bands {
            if let Some(p) = prof.get(&b.lane_id) {
                let y0 = (b.y_center - b.y_half_width).max(0.0) as usize;
                let y1 = ((b.y_center + b.y_half_width) as usize + 1).min(p.len());
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

    /// Every ladder lane as `(lane_id, label, template_index, load_ng)` where
    /// `template_index` is the position of its assigned template in
    /// [`Self::ladder_names`] (or -1 if unassigned) and `load_ng` is its
    /// per-lane loaded mass.
    pub fn ladder_lanes(&self) -> Vec<(u32, String, i32, f64)> {
        let names = self.ladder_names();
        let Some(a) = self.analysis() else {
            return Vec::new();
        };
        a.lanes
            .iter()
            .filter(|l| l.is_ladder)
            .map(|l| {
                let assigned = a
                    .ladder_assignments
                    .iter()
                    .find(|la| la.lane_id == l.id)
                    .map(|la| la.template_name.clone());
                let idx = assigned
                    .as_ref()
                    .and_then(|n| names.iter().position(|m| m == n))
                    .map(|p| p as i32)
                    .unwrap_or(-1);
                let label = l
                    .label
                    .clone()
                    .unwrap_or_else(|| format!("Lane {}", l.id));
                (l.id, label, idx, self.ladder_load(l.id))
            })
            .collect()
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

    /// Apply one template to *every* ladder lane at once (the "set all" control).
    pub fn set_all_ladders(&mut self, template_idx: usize) -> String {
        let ids: Vec<u32> = match self.analysis() {
            Some(a) => a.lanes.iter().filter(|l| l.is_ladder).map(|l| l.id).collect(),
            None => Vec::new(),
        };
        if ids.is_empty() {
            return "No ladder lanes — mark a lane as a ladder first.".into();
        }
        let mut matched = 0;
        for id in &ids {
            if self.set_lane_ladder(*id, template_idx).contains("rungs matched") {
                matched += 1;
            }
        }
        format!("Set ladder on {matched}/{} lane(s).", ids.len())
    }

    /// Replace the current project with the synthetic demo gel (multi-exposure
    /// HDR bracket) and place the demo annotation over it.
    pub fn load_demo(&mut self) -> String {
        let doc = opengel::core::demo::demo_document();
        self.gel_type = doc.project.gel_type;
        self.work = doc.working_image();
        self.doc = Some(doc);
        self.source_path = None;
        self.view_frame = None;
        self.reset_display_window();
        let msg = self.demo_annotation();
        format!("Loaded demo gel. {msg}")
    }

    pub fn open_path(&mut self, path: &Path) -> Result<()> {
        let doc = GelDocument::load(path).with_context(|| format!("loading {}", path.display()))?;
        self.gel_type = doc.project.gel_type;
        self.work = doc.working_image();
        self.doc = Some(doc);
        self.source_path = Some(path.to_path_buf());
        self.view_frame = None;
        self.reset_display_window();
        Ok(())
    }

    pub fn save_path(&self, path: &Path) -> Result<()> {
        let doc = self.doc.as_ref().ok_or_else(|| anyhow!("nothing to save"))?;
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
        Ok(format!("Captured {n}-frame HDR bracket from {source}."))
    }

    /// Run detection + ladder ID + sizing. If `force_template` is set, only that
    /// template is considered (min_r2 relaxed so the user's choice wins).
    pub fn analyze(&mut self, force_template: Option<&str>) -> Result<String> {
        let work = self.display_image().ok_or_else(|| anyhow!("no image loaded"))?;
        let work = &work;
        let params = DetectParams::default();

        let (candidates, min_r2): (Vec<&opengel::core::LadderTemplate>, f64) = match force_template {
            Some(name) => {
                let t = ladders::by_name(name)
                    .ok_or_else(|| anyhow!("unknown ladder {name}"))?;
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

        let doc = self
            .doc
            .as_mut()
            .ok_or_else(|| anyhow!("no document"))?;
        doc.project.analysis = analysis;
        Ok(format!("Detected {n_lanes} lanes, {n_bands} bands{ladder}."))
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
            let w = img.width() as f64;
            let cx = nx.clamp(0.0, 1.0) * w;
            let half = (0.04 * w).max(4.0);
            let id = a.lanes.iter().map(|l| l.id).max().map_or(0, |m| m + 1);
            a.lanes.push(Lane {
                id,
                x_min: (cx - half).max(0.0) as u32,
                x_max: (cx + half).min(w) as u32,
                y_min: 0,
                y_max: img.height() as u32,
                label: None,
                is_ladder: false,
            });
            format!("Added lane {id}.")
        })
    }

    pub fn delete_lane_near(&mut self, nx: f64) -> String {
        self.with_analysis_mut(|a, img| {
            let x = nx.clamp(0.0, 1.0) * img.width() as f64;
            let Some(pos) = nearest_lane(a, x) else {
                return "No lanes.".into();
            };
            let id = a.lanes[pos].id;
            a.lanes.remove(pos);
            a.bands.retain(|b| b.lane_id != id);
            a.ladder_assignments.retain(|la| la.lane_id != id);
            format!("Deleted lane {id}.")
        })
    }

    pub fn toggle_ladder_near(&mut self, nx: f64) -> String {
        self.with_analysis_mut(|a, img| {
            let x = nx.clamp(0.0, 1.0) * img.width() as f64;
            let Some(pos) = nearest_lane(a, x) else {
                return "No lanes.".into();
            };
            a.lanes[pos].is_ladder = !a.lanes[pos].is_ladder;
            format!(
                "Lane {} ladder = {}.",
                a.lanes[pos].id, a.lanes[pos].is_ladder
            )
        })
    }

    pub fn add_band_at(&mut self, nx: f64, ny: f64) -> String {
        self.with_analysis_mut(|a, img| {
            let x = nx.clamp(0.0, 1.0) * img.width() as f64;
            let yc = ny.clamp(0.0, 1.0) * img.height() as f64;
            let Some(pos) = nearest_lane(a, x) else {
                return "Add a lane first.".into();
            };
            let lane = &a.lanes[pos];
            let half = 5.0;
            let density = window_density(img, lane.x_min as usize, lane.x_max as usize, yc, half);
            let rf = Some((yc - lane.y_min as f64) / (lane.y_max - lane.y_min).max(1) as f64);
            let id = a.bands.iter().map(|b| b.id).max().map_or(0, |m| m + 1);
            let lane_id = lane.id;
            a.bands.push(Band {
                id,
                lane_id,
                y_center: yc,
                y_half_width: half,
                integrated_density: density,
                rf,
                size: None,
                known_size: None,
            });
            format!("Added band {id} to lane {lane_id}.")
        })
    }

    pub fn delete_band_near(&mut self, nx: f64, ny: f64) -> String {
        self.with_analysis_mut(|a, img| {
            let x = nx.clamp(0.0, 1.0) * img.width() as f64;
            let y = ny.clamp(0.0, 1.0) * img.height() as f64;
            let mut best = None;
            let mut best_d = f64::INFINITY;
            for (i, b) in a.bands.iter().enumerate() {
                let lane_cx = a
                    .lanes
                    .iter()
                    .find(|l| l.id == b.lane_id)
                    .map(|l| (l.x_min + l.x_max) as f64 / 2.0)
                    .unwrap_or(x);
                let d = (lane_cx - x).powi(2) + (b.y_center - y).powi(2);
                if d < best_d {
                    best_d = d;
                    best = Some(i);
                }
            }
            match best {
                Some(i) => {
                    let id = a.bands[i].id;
                    a.bands.remove(i);
                    format!("Deleted band {id}.")
                }
                None => "No bands.".into(),
            }
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
                let load = loads.get(&assign.lane_id).copied().unwrap_or(DEFAULT_LADDER_LOAD_NG);
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
                let nmol = b
                    .size
                    .and_then(|s| mass_ng_to_nmol(mass, s, gel_type));
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
fn apply_ladder_to_lane(a: &mut Analysis, lane_id: u32, template: &LadderTemplate) -> Option<usize> {
    // Band indices for this lane, in detection (y-ascending) order.
    let idxs: Vec<usize> = a
        .bands
        .iter()
        .enumerate()
        .filter(|(_, b)| b.lane_id == lane_id)
        .map(|(i, _)| i)
        .collect();
    let positions: Vec<f64> = idxs.iter().map(|&i| a.bands[i].y_center).collect();
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
        .filter_map(|b| b.known_size.map(|s| (b.y_center, s)))
        .collect();
    let Some(fit) = opengel::core::quant::SizingFit::fit(&pts) else {
        return;
    };
    let ladder_ids: std::collections::BTreeSet<u32> =
        a.lanes.iter().filter(|l| l.is_ladder).map(|l| l.id).collect();
    for b in &mut a.bands {
        if !ladder_ids.contains(&b.lane_id) {
            b.size = Some(fit.size_at(b.y_center));
        }
    }
}

/// Index of the lane whose x-center is nearest `x`.
fn nearest_lane(a: &Analysis, x: f64) -> Option<usize> {
    a.lanes
        .iter()
        .enumerate()
        .min_by(|(_, l), (_, m)| {
            let dl = ((l.x_min + l.x_max) as f64 / 2.0 - x).abs();
            let dm = ((m.x_min + m.x_max) as f64 / 2.0 - x).abs();
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
        assert!(a.ladder_assignments.iter().any(|x| x.template_name.contains("1 kb")));

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
        st.add_band_at(0.5, 0.5);
        assert!(st.analysis().unwrap().bands.len() > before);

        // Delete the band we just added back out (nearest to the click).
        st.delete_band_near(0.5, 0.5);

        // Toggle ladder + delete a lane operate without panicking.
        st.toggle_ladder_near(0.5);
        st.delete_lane_near(0.5);
        assert!(st.analysis().unwrap().lanes.len() < lanes_after);

        // Absolute calibration from the identified ladder (default 500 ng load).
        let msg = st.calibrate(10.0);
        assert!(msg.contains("Calibrated"), "got: {msg}");
        let a = st.analysis().unwrap();
        assert!(a.calibration.is_some());
        assert!(a.quantifications.iter().any(|q| q.mass_ng.unwrap_or(0.0) > 0.0));
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
    fn demo_annotation_measures_regions() {
        // No image yet: demo_annotation should capture a mock gel, place 4
        // lanes, and measure each band region from the pixels.
        let mut st = AppState::new();
        let msg = st.demo_annotation();
        assert!(msg.contains("Demo annotation"), "got: {msg}");
        let a = st.analysis().unwrap();
        assert_eq!(a.lanes.len(), 4);
        assert!(a.lanes.iter().any(|l| l.is_ladder));
        assert!(!a.bands.is_empty());
        // Measurement produced positive integrated densities for bands sitting
        // on the mock gel's bright bands.
        assert!(
            a.bands.iter().filter(|b| b.integrated_density > 0.0).count() >= 3,
            "expected several measured regions"
        );

        // Re-measuring is idempotent-ish (still positive).
        let msg2 = st.measure_regions();
        assert!(msg2.contains("Measured"), "got: {msg2}");
    }

    #[test]
    fn trace_selection_and_compute() {
        let mut st = AppState::new();
        st.demo_annotation();
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
        let sel = st.lane_items().into_iter().filter(|(_, _, _, s)| *s).count();
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
        assert!(st.ladder_names().iter().all(|n| opengel::core::ladders::by_name(n).is_some()));
    }
}
