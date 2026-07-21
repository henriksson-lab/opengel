//! Application state and the operations behind each UI callback.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use gel_core::model::{Analysis, Band, Calibration, GelType, Lane, Quantification, TargetKind};
use gel_core::quant::{compare, mass_ng_to_nmol, nmol_to_molar};
use gel_core::{ladders, GelDocument, GrayF32};
use gel_detect::detector::DetectParams;

pub struct AppState {
    pub doc: Option<GelDocument>,
    pub work: Option<GrayF32>,
    pub gel_type: GelType,
    pub source_path: Option<PathBuf>,
    /// Rotation correction applied before analysis/display (degrees).
    pub rotation_deg: f64,
    bracket_counter: u32,
}

impl AppState {
    pub fn new() -> Self {
        AppState {
            doc: None,
            work: None,
            gel_type: GelType::Dna,
            source_path: None,
            rotation_deg: 0.0,
            bracket_counter: 0,
        }
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
        use gel_detect::classical::lane_row_profile;
        use gel_detect::signal::subtract_baseline;
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
        let est = gel_detect::orient::estimate_rotation(w, 50.0, true);
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

    pub fn open_path(&mut self, path: &Path) -> Result<()> {
        let doc = GelDocument::load(path).with_context(|| format!("loading {}", path.display()))?;
        self.gel_type = doc.project.gel_type;
        self.work = doc.working_image();
        self.doc = Some(doc);
        self.source_path = Some(path.to_path_buf());
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
        Ok(format!("Captured {n}-frame HDR bracket from {source}."))
    }

    /// Run detection + ladder ID + sizing. If `force_template` is set, only that
    /// template is considered (min_r2 relaxed so the user's choice wins).
    pub fn analyze(&mut self, force_template: Option<&str>) -> Result<String> {
        let work = self.display_image().ok_or_else(|| anyhow!("no image loaded"))?;
        let work = &work;
        let params = DetectParams::default();

        let (candidates, min_r2): (Vec<&gel_core::LadderTemplate>, f64) = match force_template {
            Some(name) => {
                let t = ladders::by_name(name)
                    .ok_or_else(|| anyhow!("unknown ladder {name}"))?;
                (vec![t], 0.0)
            }
            None => (Vec::new(), 0.9),
        };
        let analysis = gel_detect::analyze(work, self.gel_type, &params, &candidates, min_r2);
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

    /// Force a specific ladder (by index into `ladder_names`) and re-analyze.
    pub fn force_ladder(&mut self, idx: usize) -> String {
        let names = self.ladder_names();
        let Some(name) = names.get(idx).cloned() else {
            return "No ladder selected.".into();
        };
        match self.analyze(Some(&name)) {
            Ok(msg) => msg,
            Err(e) => format!("Analyze failed: {e}"),
        }
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

    /// Fit an intensity→mass calibration from the identified ladder lane, given
    /// the total loaded ladder mass (ng), then quantify every band (ng + nmol).
    ///
    /// Per-rung masses use the template values if present, else the total is
    /// distributed proportionally to fragment size (a reasonable default for
    /// mass ladders; the user can override the ladder).
    pub fn calibrate(&mut self, total_ng: f64, volume_ul: f64) -> String {
        let gel_type = self.gel_type;
        self.with_analysis_mut(move |a, _img| {
            let Some(assign) = a.ladder_assignments.first().cloned() else {
                return "Identify a ladder first (Analyze).".into();
            };
            // Ladder bands with known size + density.
            let ladder_bands: Vec<(f64, f64)> = a
                .bands
                .iter()
                .filter(|b| b.lane_id == assign.lane_id && b.known_size.is_some())
                .map(|b| (b.integrated_density, b.known_size.unwrap()))
                .collect();
            if ladder_bands.len() < 2 {
                return "Not enough ladder bands to calibrate.".into();
            }
            let size_sum: f64 = ladder_bands.iter().map(|(_, s)| s).sum();
            // (density, mass) points.
            let points: Vec<(f64, f64)> = ladder_bands
                .iter()
                .map(|(d, s)| (*d, total_ng * s / size_sum))
                .collect();
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
                "Calibrated from {} ladder bands ({total_ng:.0} ng total).",
                ladder_bands.len()
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

        // Absolute calibration from the identified ladder.
        let msg = st.calibrate(500.0, 10.0);
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
    fn ladder_names_match_gel_type() {
        let st = AppState::new();
        assert!(!st.ladder_names().is_empty());
        assert!(st.ladder_names().iter().all(|n| gel_core::ladders::by_name(n).is_some()));
    }
}
