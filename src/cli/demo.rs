//! Synthetic demo gel + dataset generation.
//!
//! Both a `.gel.zip` (for `analyze`/`info`) and a loose image + ground-truth
//! JSON (for `eval`) are produced by the *simulator*-backed demo scene (see
//! [`opengel::core::demo`]), so the ground truth is exact by construction. The
//! ladder lanes reproduce the NEB 1 kb ladder so they are identified against the
//! built-in database.

use anyhow::Result;
use opengel::core::demo::{demo_dataset, demo_document};

pub fn write_demo(out: &std::path::Path) -> Result<()> {
    let doc = demo_document();
    doc.save(out)?;
    println!("wrote demo gel to {}", out.display());
    Ok(())
}

/// Write `<dir>/demo.png` and `<dir>/demo.gt.json` for use with `gel eval`,
/// straight from the simulator's exact ground truth.
pub fn write_dataset(dir: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let (img, gt) = demo_dataset("demo.png");
    let png_path = dir.join("demo.png");
    img.save(&png_path)?;
    let json = serde_json::to_string_pretty(&gt)?;
    let gt_path = dir.join("demo.gt.json");
    std::fs::write(&gt_path, json)?;
    println!(
        "wrote dataset: {} and {}",
        png_path.display(),
        gt_path.display()
    );
    Ok(())
}
