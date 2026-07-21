//! Synthetic demo gel + dataset generation.
//!
//! Both a `.gel.zip` (for `analyze`/`info`) and a loose image + ground-truth
//! JSON (for `eval`) are rendered from the *same* scene (see
//! [`opengel::core::demo`]), so the ground truth is exact by construction. The
//! ladder lane reproduces the NEB 1 kb ladder so it is identified against the
//! built-in database.

use anyhow::Result;
use opengel::core::demo::{demo_document, render, scene, size_to_y};
use opengel::core::GelType;
use opengel::detect::eval::{GroundTruth, GtBand, GtLane};

pub fn write_demo(out: &std::path::Path) -> Result<()> {
    let doc = demo_document();
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
