//! Synthetic demo gel + dataset generation.
//!
//! Both a `.gel.zip` (for `analyze`/`info`) and a loose image + ground-truth
//! JSON (for `eval`) are rendered from the *same* scene, so the ground truth is
//! exact by construction. The ladder lane reproduces the NEB 1 kb ladder so it
//! is identified against the built-in database.

use anyhow::Result;
use gel_core::model::CaptureMeta;
use gel_core::{ladders, GelDocument, GelType};
use gel_detect::eval::{GroundTruth, GtBand, GtLane};
use image::{DynamicImage, GrayImage, Luma};

const W: u32 = 220;
const H: u32 = 300;
const SX: f64 = 5.0;
const SY: f64 = 3.5;

struct SceneLane {
    x: f64,
    ladder: Option<&'static str>,
    /// Band sizes (bp).
    sizes: Vec<f64>,
    amp: f64,
}

/// Map a fragment size to a y position via a fixed semi-log calibration.
fn size_to_y(size: f64) -> f64 {
    let (ln_hi, ln_lo) = (10000f64.ln(), 500f64.ln());
    let slope = (280.0 - 20.0) / (ln_lo - ln_hi);
    20.0 + (size.ln() - ln_hi) * slope
}

fn scene() -> Vec<SceneLane> {
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

fn render(scene: &[SceneLane]) -> GrayImage {
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
    let mut img = GrayImage::new(W, H);
    for (i, p) in buf.iter().enumerate() {
        let v = (p.min(1.0) * 255.0) as u8;
        img.put_pixel((i as u32) % W, (i as u32) / W, Luma([v]));
    }
    img
}

pub fn write_demo(out: &std::path::Path) -> Result<()> {
    let img = DynamicImage::ImageLuma8(render(&scene()));
    let meta = CaptureMeta {
        exposure_seconds: 0.2,
        camera_name: Some("synthetic".into()),
        ..Default::default()
    };
    let doc = GelDocument::from_frames(GelType::Dna, vec![img], vec![meta]);
    doc.save(out)?;
    println!("wrote demo gel to {}", out.display());
    Ok(())
}

/// Write `<dir>/demo.png` and `<dir>/demo.gt.json` for use with `gel eval`.
pub fn write_dataset(dir: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let sc = scene();
    let img = render(&sc);
    let png_path = dir.join("demo.png");
    img.save(&png_path)?;

    let lanes: Vec<GtLane> = sc
        .iter()
        .map(|l| GtLane {
            x_min: (l.x - 12.0) as u32,
            x_max: (l.x + 12.0) as u32,
            is_ladder: l.ladder.is_some(),
            ladder_name: l.ladder.map(|s| s.to_string()),
            bands: l
                .sizes
                .iter()
                .map(|&s| GtBand {
                    y_center: size_to_y(s),
                    size: Some(s),
                })
                .collect(),
        })
        .collect();
    let gt = GroundTruth {
        image: "demo.png".into(),
        gel_type: GelType::Dna,
        lanes,
    };
    let json = serde_json::to_string_pretty(&gt)?;
    let gt_path = dir.join("demo.gt.json");
    std::fs::write(&gt_path, json)?;
    println!("wrote dataset: {} and {}", png_path.display(), gt_path.display());
    Ok(())
}
