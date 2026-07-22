//! Parallel batch simulation and dataset export.

use std::path::Path;

use rayon::prelude::*;

use crate::sim::config::{SimConfig, WarpMode};
use crate::sim::render::{simulate, RenderedGel};

/// Render many configs in parallel across worker threads (rayon).
pub fn simulate_batch(configs: &[SimConfig]) -> Vec<RenderedGel> {
    configs.par_iter().map(simulate).collect()
}

/// Render `count` randomized gels (seeds `seed_base..seed_base+count`) in
/// parallel. `upright` disables rotation (for pure detection benchmarking).
pub fn simulate_random_batch(count: usize, seed_base: u64, upright: bool) -> Vec<RenderedGel> {
    simulate_random_batch_with(count, seed_base, upright, WarpMode::default())
}

/// Like [`simulate_random_batch`] but with an explicit warp model.
pub fn simulate_random_batch_with(
    count: usize,
    seed_base: u64,
    upright: bool,
    warp_mode: WarpMode,
) -> Vec<RenderedGel> {
    let configs: Vec<SimConfig> = (0..count as u64)
        .map(|i| {
            let seed = seed_base + i;
            let mut c = if upright {
                SimConfig::randomized_upright(seed)
            } else {
                SimConfig::randomized(seed)
            };
            c.gel.warp_mode = warp_mode;
            c
        })
        .collect();
    simulate_batch(&configs)
}

/// Write a batch to `dir` as `<image>.png` + `<image>.gt.json` pairs, in
/// parallel. Returns the number of pairs written.
pub fn write_dataset(dir: &Path, gels: &[RenderedGel]) -> std::io::Result<usize> {
    std::fs::create_dir_all(dir)?;
    let results: Vec<std::io::Result<()>> = gels
        .par_iter()
        .map(|g| -> std::io::Result<()> {
            let img_path = dir.join(&g.truth.image);
            g.image
                .save(&img_path)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let stem = g.truth.image.trim_end_matches(".png");
            let gt_path = dir.join(format!("{stem}.gt.json"));
            let json = serde_json::to_string_pretty(&g.truth)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            std::fs::write(gt_path, json)?;
            Ok(())
        })
        .collect();
    for r in &results {
        if let Err(e) = r {
            return Err(std::io::Error::new(e.kind(), e.to_string()));
        }
    }
    Ok(gels.len())
}
