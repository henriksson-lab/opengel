//! Synthetic demo gel used by both the CLI (`gel make-demo`, `gel make-dataset`)
//! and the desktop GUI (`opengel --demo`).
//!
//! The scene is a fixed 4-lane DNA gel whose first lane reproduces the NEB 1 kb
//! ladder, so it is identified against the built-in database. [`demo_document`]
//! renders the same scene at several exposures as one HDR bracket, exercising
//! the exposure-stepping and HDR-merge paths.

use image::{DynamicImage, GrayImage, Luma};

use crate::core::model::CaptureMeta;
use crate::core::{ladders, GelDocument, GelType};

pub const W: u32 = 220;
pub const H: u32 = 300;
const SX: f64 = 5.0;
const SY: f64 = 3.5;

/// Reference exposure (seconds); scene amplitudes are tuned for this.
pub const REF_EXPOSURE_S: f64 = 0.2;

/// The exposures (seconds) making up the demo HDR bracket: one under-exposed,
/// one nominal, one over-exposed (partly saturated).
pub const DEMO_EXPOSURES_S: [f64; 3] = [0.06, 0.2, 0.6];

pub struct SceneLane {
    pub x: f64,
    pub ladder: Option<&'static str>,
    /// Band sizes (bp).
    pub sizes: Vec<f64>,
    pub amp: f64,
}

/// Map a fragment size to a y position via a fixed semi-log calibration.
pub fn size_to_y(size: f64) -> f64 {
    let (ln_hi, ln_lo) = (10000f64.ln(), 500f64.ln());
    let slope = (280.0 - 20.0) / (ln_lo - ln_hi);
    20.0 + (size.ln() - ln_hi) * slope
}

pub fn scene() -> Vec<SceneLane> {
    let ladder = ladders::by_name("NEB 1 kb DNA Ladder").expect("built-in ladder");
    vec![
        SceneLane {
            x: 30.0,
            ladder: Some("NEB 1 kb DNA Ladder"),
            sizes: ladder.bands.iter().map(|b| b.size).collect(),
            amp: 0.9,
        },
        SceneLane { x: 80.0, ladder: None, sizes: vec![3000.0, 1200.0, 600.0], amp: 0.75 },
        SceneLane { x: 130.0, ladder: None, sizes: vec![5000.0, 900.0], amp: 0.75 },
        SceneLane { x: 180.0, ladder: None, sizes: vec![2000.0, 1500.0, 800.0, 300.0], amp: 0.75 },
    ]
}

/// The scene's radiance field (unclamped), summed over all band gaussians.
fn radiance(scene: &[SceneLane]) -> Vec<f64> {
    let mut buf = vec![0f64; (W * H) as usize];
    for lane in scene {
        for &size in &lane.sizes {
            let y0 = size_to_y(size);
            for y in 0..H {
                for x in 0..W {
                    let dx = (x as f64 - lane.x) / SX;
                    let dy = (y as f64 - y0) / SY;
                    buf[(y * W + x) as usize] += lane.amp * (-0.5 * (dx * dx + dy * dy)).exp();
                }
            }
        }
    }
    buf
}

/// Render the scene as an 8-bit image at the given exposure multiplier
/// (1.0 == nominal). Values are clamped to `[0,1]` before quantization, so
/// large multipliers saturate the brightest bands.
pub fn render_scaled(scene: &[SceneLane], scale: f64) -> GrayImage {
    let buf = radiance(scene);
    let mut img = GrayImage::new(W, H);
    for (i, p) in buf.iter().enumerate() {
        let v = ((p * scale).clamp(0.0, 1.0) * 255.0) as u8;
        img.put_pixel((i as u32) % W, (i as u32) / W, Luma([v]));
    }
    img
}

/// Render the scene at nominal exposure (back-compat helper for the dataset
/// writer, whose ground truth assumes the nominal brightness).
pub fn render(scene: &[SceneLane]) -> GrayImage {
    render_scaled(scene, 1.0)
}

/// A ready-to-use demo [`GelDocument`]: the fixed scene captured as a 3-frame
/// HDR exposure bracket (bracket group 0).
pub fn demo_document() -> GelDocument {
    let sc = scene();
    let frames: Vec<DynamicImage> = DEMO_EXPOSURES_S
        .iter()
        .map(|&t| DynamicImage::ImageLuma8(render_scaled(&sc, t / REF_EXPOSURE_S)))
        .collect();
    let metas: Vec<CaptureMeta> = DEMO_EXPOSURES_S
        .iter()
        .map(|&t| CaptureMeta {
            exposure_seconds: t,
            camera_name: Some("synthetic".into()),
            bracket_group: Some(0),
            ..Default::default()
        })
        .collect();
    GelDocument::from_frames(GelType::Dna, frames, metas)
}
