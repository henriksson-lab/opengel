//! Convert GelGenie-style segmentation masks into OpenGel evaluation ground
//! truth (`*.gt.json`), so the detector can be benchmarked on real annotated
//! gels via `gel eval`.
//!
//! GelGenie layout: a subset folder holds `{images,val_images,test_images}` and
//! matching `{masks,val_masks,test_masks}` with identical filenames. Masks are
//! label images (0 = background, nonzero = band foreground). Each connected
//! foreground component becomes a band; components are clustered by x into
//! lanes.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use opengel::core::model::GelType;
use opengel::detect::eval::{GroundTruth, GtBand, GtLane};
use imageproc::region_labelling::{connected_components, Connectivity};

/// Walk `dir`, pair every `*masks*/<name>` with `*images*/<name>`, and write
/// `<name>.gt.json` next to each image. Returns the number of pairs converted.
pub fn import_dir(dir: &Path) -> Result<usize> {
    let mut count = 0;
    for mask_path in find_masks(dir) {
        let img_path = image_for_mask(&mask_path);
        if !img_path.exists() {
            continue;
        }
        match convert_pair(&img_path, &mask_path) {
            Ok(gt) => {
                let out = img_path.with_extension("gt.json");
                std::fs::write(&out, serde_json::to_string_pretty(&gt)?)
                    .with_context(|| format!("writing {}", out.display()))?;
                count += 1;
            }
            Err(e) => eprintln!("skip {}: {e}", mask_path.display()),
        }
    }
    Ok(count)
}

fn find_masks(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(dir, &mut |p| {
        let is_img = p
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| matches!(e.to_lowercase().as_str(), "tif" | "tiff" | "png"))
            .unwrap_or(false);
        let in_mask_dir = p
            .parent()
            .and_then(|d| d.file_name())
            .and_then(|n| n.to_str())
            .map(|n| n.contains("mask"))
            .unwrap_or(false);
        if is_img && in_mask_dir {
            out.push(p.to_path_buf());
        }
    });
    out
}

fn walk(dir: &Path, f: &mut dyn FnMut(&Path)) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk(&p, f);
        } else {
            f(&p);
        }
    }
}

/// Map a mask path to its image path by swapping "masks" → "images" in the
/// directory component (handles masks / val_masks / test_masks).
fn image_for_mask(mask: &Path) -> PathBuf {
    let s = mask.to_string_lossy().replace("masks", "images");
    PathBuf::from(s)
}

fn convert_pair(img_path: &Path, mask_path: &Path) -> Result<GroundTruth> {
    let mask = image::open(mask_path)
        .with_context(|| format!("open mask {}", mask_path.display()))?
        .to_luma8();
    let (w, h) = mask.dimensions();
    // Foreground is whichever value differs from the background *mode* — robust
    // to either polarity (0/1 label masks, or palette-resolved white/brown).
    let mut hist = [0u64; 256];
    for p in mask.pixels() {
        hist[p.0[0] as usize] += 1;
    }
    let bg = hist
        .iter()
        .enumerate()
        .max_by_key(|(_, &c)| c)
        .map(|(v, _)| v as i32)
        .unwrap_or(0);
    let mut bin = image::GrayImage::new(w, h);
    for (x, y, p) in mask.enumerate_pixels() {
        let fg = ((p.0[0] as i32) - bg).abs() > 30;
        bin.put_pixel(x, y, image::Luma([if fg { 255 } else { 0 }]));
    }
    let labels = connected_components(&bin, Connectivity::Eight, image::Luma([0u8]));

    // Aggregate per-label bbox + centroid.
    #[derive(Default, Clone)]
    struct Agg {
        n: u64,
        sx: u64,
        sy: u64,
        x0: u32,
        y0: u32,
        x1: u32,
        y1: u32,
        init: bool,
    }
    let mut aggs: std::collections::HashMap<u32, Agg> = std::collections::HashMap::new();
    for (x, y, p) in labels.enumerate_pixels() {
        let l = p.0[0];
        if l == 0 {
            continue;
        }
        let a = aggs.entry(l).or_default();
        a.n += 1;
        a.sx += x as u64;
        a.sy += y as u64;
        if !a.init {
            a.x0 = x;
            a.y0 = y;
            a.x1 = x;
            a.y1 = y;
            a.init = true;
        } else {
            a.x0 = a.x0.min(x);
            a.y0 = a.y0.min(y);
            a.x1 = a.x1.max(x);
            a.y1 = a.y1.max(y);
        }
    }

    // Filter specks; keep (cx, cy, x0, x1) per band.
    let min_area = ((w as u64 * h as u64) / 20000).max(8);
    let mut bands: Vec<(f64, f64, u32, u32)> = aggs
        .values()
        .filter(|a| a.n >= min_area)
        .map(|a| {
            (
                a.sx as f64 / a.n as f64,
                a.sy as f64 / a.n as f64,
                a.x0,
                a.x1,
            )
        })
        .collect();
    bands.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

    // Cluster into lanes by x gap.
    let gap = (0.03 * w as f64).max(6.0);
    let mut lanes: Vec<GtLane> = Vec::new();
    let mut cur: Vec<(f64, f64, u32, u32)> = Vec::new();
    let mut last_cx: Option<f64> = None;
    let flush = |cur: &mut Vec<(f64, f64, u32, u32)>, lanes: &mut Vec<GtLane>| {
        if cur.is_empty() {
            return;
        }
        let x_min = cur.iter().map(|b| b.2).min().unwrap();
        let x_max = cur.iter().map(|b| b.3).max().unwrap();
        let mut gbands: Vec<GtBand> = cur
            .iter()
            .map(|b| GtBand {
                y_center: b.1,
                size: None,
            })
            .collect();
        gbands.sort_by(|a, b| a.y_center.partial_cmp(&b.y_center).unwrap());
        lanes.push(GtLane {
            x_min,
            x_max,
            is_ladder: false,
            ladder_name: None,
            bands: gbands,
        });
        cur.clear();
    };
    for b in &bands {
        if let Some(prev) = last_cx {
            if b.0 - prev > gap {
                flush(&mut cur, &mut lanes);
            }
        }
        cur.push(*b);
        last_cx = Some(b.0);
    }
    flush(&mut cur, &mut lanes);

    Ok(GroundTruth {
        image: img_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        gel_type: GelType::Dna,
        lanes,
    })
}
