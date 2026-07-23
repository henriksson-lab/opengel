//! Synthetic demo gel used by both the CLI (`gel make-demo`, `gel make-dataset`)
//! and the desktop GUI (`opengel --demo`).
//!
//! The demo is produced by the **simulator** ([`crate::sim`]) rather than a bare
//! Gaussian renderer, so `opengel --demo` shows the realistic features: loading
//! **wells**, dim gel **autofluorescence**, and the gel sitting **inside a
//! larger camera frame** (a dark border), with flat-topped rectangular bands.
//!
//! The scene is a fixed, upright 8-lane DNA gel with three 1 kb ladder lanes
//! (indices 0, 4, 7) and five sample lanes with fixed band sizes.
//! [`demo_document`] renders it as one 3-frame HDR exposure bracket (identical
//! scene, three brightnesses), and [`demo_document_annotated`] additionally
//! attaches an annotation built from the simulator's *exact* ground truth, so
//! every lane column and band box lines up with the rendered scene.

use image::DynamicImage;

use crate::core::model::{Analysis, Band, CaptureMeta, Lane};
use crate::core::{GelDocument, GelType};
use crate::detect::eval::GroundTruth;
use crate::sim::{simulate, GelParams, RenderedGel, SimConfig, SimLane, WarpMode};

/// Demo frame dimensions (the full camera frame, gel inset via the margin).
pub const W: u32 = 480;
pub const H: u32 = 320;

/// Fixed seed: all three bracket frames share it so the scene is identical and
/// only the exposure differs.
const DEMO_SEED: u64 = 0xDE_0A;

/// The exposure gains making up the demo HDR bracket: one under-exposed, one
/// nominal, one over-exposed (partly saturated). Also used (as seconds) for the
/// `exposure_seconds` HDR-merge weights.
pub const DEMO_EXPOSURES: [f64; 3] = [0.4, 1.0, 2.5];

/// Nominal (middle) exposure — used for the single-frame dataset image.
const NOMINAL_EXPOSURE: f64 = 1.0;

/// The demo scene as a [`SimConfig`] at the given exposure gain. Everything
/// except `gel.exposure` is fixed, so calling this with the bracket exposures
/// yields three brightness-scaled views of one identical scene.
fn demo_config(exposure: f64) -> SimConfig {
    let gel = GelParams {
        width: W,
        height: H,
        seed: DEMO_SEED,
        gel_type: GelType::Dna,
        // A gentle smile + wobble warp so the gel looks like a real (slightly
        // curved) plate. The closed-form SmileWobble warp is used because its
        // inverse is O(1) per pixel — the NURBS mode would run a per-pixel Newton
        // solve (16×16 seed search), making the demo take tens of seconds.
        rotation_deg: 0.0,
        smile_px: 11.0,
        wobble_px: 4.0,
        warp_mode: WarpMode::SmileWobble,
        warp_2d: false,
        shift_px: (0.0, 0.0),
        // A dim, uneven camera background so the dark border around the gel is
        // clearly visible.
        background: 0.05,
        exposure,
        photons: 0.0,
        // A clear margin so the gel sits well inside the camera frame; strong
        // agarose autofluorescence (the gel glows, brightest near the wells),
        // wells, scattered bright specks and a bit of DNA smear — tuned to look
        // like a real stained agarose gel.
        gel_margin_frac: 0.09,
        fluorescence: 0.34,
        speck_density: 0.0009,
        smear: 0.08,
        wells: true,
        // Low band gain → faint bands over the bright fluorescent background,
        // matching a real gel photo (bright bands saturate, faint ones are grey).
        band_gain: 0.03,
        // Flat-topped rectangular bands with gentle diffusion/compression.
        lane_width_frac: 0.72,
        band_sigma_y: 2.6,
        diffusion: 1.5,
        migration_compression: 0.3,
    };
    // Three ladder lanes at 0, 4, 7; five sample lanes between them. The ladder
    // is a built-in template and its lanes reproduce that template's exact rungs
    // (see `sim::render`), so the demo self-identifies as this same ladder under
    // `--detect`. The Promega 1 kb ladder is used because, on this scene, its
    // rung pattern is the one the classical detector identifies unambiguously as
    // itself (other 1 kb ladders alias to denser near-superset templates), while
    // keeping all eight lanes cleanly resolved.
    let ladder = "Promega 1 kb DNA Ladder";
    let lanes = vec![
        SimLane::ladder(ladder),                             // 0  ladder
        SimLane::sample(vec![4000.0, 1500.0, 700.0]),        // 1
        SimLane::sample(vec![6000.0, 2000.0, 900.0, 400.0]), // 2
        SimLane::sample(vec![3000.0, 1000.0]),               // 3
        SimLane::ladder(ladder),                             // 4  ladder
        SimLane::sample(vec![5000.0, 1200.0, 500.0]),        // 5
        SimLane::sample(vec![2500.0, 800.0]),                // 6
        SimLane::ladder(ladder),                             // 7  ladder
    ];
    SimConfig { gel, lanes }
}

/// Render the three bracket frames (identical scene, three exposures).
fn demo_gels() -> Vec<RenderedGel> {
    DEMO_EXPOSURES
        .iter()
        .map(|&e| simulate(&demo_config(e)))
        .collect()
}

/// Frames + parallel capture metadata for a set of rendered bracket gels. All
/// frames share `bracket_group = 0` and carry their exposure as
/// `exposure_seconds`, so [`GelDocument::working_image`] HDR-merges them.
fn frames_and_metas(gels: &[RenderedGel]) -> (Vec<DynamicImage>, Vec<CaptureMeta>) {
    let frames = gels.iter().map(|g| g.image.clone()).collect();
    let metas = DEMO_EXPOSURES
        .iter()
        .map(|&t| CaptureMeta {
            exposure_seconds: t,
            camera_name: Some("synthetic".into()),
            bracket_group: Some(0),
            ..Default::default()
        })
        .collect();
    (frames, metas)
}

/// Build an [`Analysis`] from the simulator's exact [`GroundTruth`]: one
/// [`Lane`] per ground-truth lane and one [`Band`] per ground-truth band, so the
/// annotation boxes line up precisely with the rendered scene.
fn analysis_from_truth(truth: &GroundTruth) -> Analysis {
    let mut a = Analysis::default();
    let mut bid = 0u32;
    for (i, gl) in truth.lanes.iter().enumerate() {
        a.lanes.push(Lane {
            id: i as u32,
            u_min: gl.x_min as f64 / W as f64,
            u_max: gl.x_max as f64 / W as f64,
            label: Some(if gl.is_ladder {
                format!("Ladder {i}")
            } else {
                format!("Lane {i}")
            }),
            is_ladder: gl.is_ladder,
        });
        for gb in &gl.bands {
            let v = gb.y_center / H as f64;
            a.bands.push(Band {
                id: bid,
                lane_id: i as u32,
                v_center: v,
                v_half_width: 0.012,
                integrated_density: 0.0,
                size: None,
                known_size: None,
                angle: 0.0,
                merged_sizes: Vec::new(),
            });
            bid += 1;
        }
    }
    a
}

/// A ready-to-use demo [`GelDocument`]: the fixed scene captured as a 3-frame
/// HDR exposure bracket (bracket group 0).
pub fn demo_document() -> GelDocument {
    let gels = demo_gels();
    let (frames, metas) = frames_and_metas(&gels);
    GelDocument::from_frames(GelType::Dna, frames, metas)
}

/// [`demo_document`] plus an annotation built from the simulator's exact ground
/// truth (8 lanes, 3 marked `is_ladder`), so the boxes line up with the bands.
pub fn demo_document_annotated() -> GelDocument {
    let gels = demo_gels();
    let analysis = analysis_from_truth(&gels[0].truth);
    let (frames, metas) = frames_and_metas(&gels);
    let mut doc = GelDocument::from_frames(GelType::Dna, frames, metas);
    doc.project.analysis = analysis;
    doc
}

/// The nominal-exposure demo frame plus its exact ground truth, for writing a
/// loose image + `*.gt.json` dataset (`gel make-dataset`). `image_name` is
/// stamped into the returned [`GroundTruth::image`].
pub fn demo_dataset(image_name: &str) -> (DynamicImage, GroundTruth) {
    let g = simulate(&demo_config(NOMINAL_EXPOSURE));
    let mut truth = g.truth;
    truth.image = image_name.to_string();
    (g.image, truth)
}
