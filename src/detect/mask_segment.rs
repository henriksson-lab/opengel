//! Mask-driven segmentation adapter.
//!
//! GelGenie (and similar deep-learning band segmenters) produce a per-pixel
//! **segmentation mask** of the gel bands. This module turns such a mask into
//! band bounding boxes and exposes it as a [`BlobSegmenter`], so a
//! mask-producing model plugs straight into
//! [`CellposeDetector`](crate::detect::cellpose::CellposeDetector) → the
//! `GelDetector` pipeline and the `eval` harness — the same seam the classical
//! and Cellpose detectors use.
//!
//! This is the OpenGel-side adapter and needs no ML runtime: it consumes a mask
//! the model already produced (e.g. exported from GelGenie, or the label masks
//! the `ImportMasks` CLI already understands). Running the model *in process*
//! (producing the mask from the raw gel) is the remaining, source-dependent
//! piece of the GelGenie integration — see `PLAN.md` §6.

use std::collections::HashMap;

use imageproc::region_labelling::{connected_components, Connectivity};

use crate::core::GrayF32;
use crate::detect::cellpose::BlobSegmenter;

/// Extract band bounding boxes from a foreground segmentation `mask`.
///
/// Pixels with normalized value `> threshold` are foreground; each 8-connected
/// component with at least `min_area` pixels becomes one box. Returns
/// `(x_min, y_min, x_max, y_max)` with `x_max`/`y_max` **exclusive**, sorted by
/// x-center (the order [`CellposeDetector`](crate::detect::cellpose::CellposeDetector)
/// expects for lane clustering).
pub fn mask_to_boxes(mask: &GrayF32, threshold: f32, min_area: u32) -> Vec<(u32, u32, u32, u32)> {
    let (w, h) = (mask.width() as u32, mask.height() as u32);
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let mut bin = image::GrayImage::new(w, h);
    for y in 0..h as usize {
        for x in 0..w as usize {
            let v = if mask.get(x, y) > threshold { 255 } else { 0 };
            bin.put_pixel(x as u32, y as u32, image::Luma([v]));
        }
    }
    let labels = connected_components(&bin, Connectivity::Eight, image::Luma([0u8]));

    #[derive(Clone, Copy)]
    struct Agg {
        n: u32,
        x0: u32,
        y0: u32,
        x1: u32,
        y1: u32,
    }
    let mut aggs: HashMap<u32, Agg> = HashMap::new();
    for (x, y, p) in labels.enumerate_pixels() {
        let l = p.0[0];
        if l == 0 {
            continue;
        }
        aggs.entry(l)
            .and_modify(|a| {
                a.n += 1;
                a.x0 = a.x0.min(x);
                a.y0 = a.y0.min(y);
                a.x1 = a.x1.max(x);
                a.y1 = a.y1.max(y);
            })
            .or_insert(Agg {
                n: 1,
                x0: x,
                y0: y,
                x1: x,
                y1: y,
            });
    }

    let mut boxes: Vec<(u32, u32, u32, u32)> = aggs
        .values()
        .filter(|a| a.n >= min_area)
        .map(|a| (a.x0, a.y0, a.x1 + 1, a.y1 + 1))
        .collect();
    // Primary sort by x-center (for lane clustering downstream); break ties by
    // y-top so the ordering is deterministic regardless of hash iteration order.
    boxes.sort_by_key(|b| ((b.0 + b.2) / 2, b.1));
    boxes
}

/// A [`BlobSegmenter`] backed by a precomputed segmentation mask.
///
/// The mask is the model's output (e.g. GelGenie's), already aligned to the gel
/// image. [`segment`](BlobSegmenter::segment) ignores the image it is passed and
/// returns the mask's components — pair it with
/// [`CellposeDetector`](crate::detect::cellpose::CellposeDetector), which
/// measures each band's density from the *real* gel image it receives.
pub struct MaskSegmenter {
    mask: GrayF32,
    /// Foreground threshold on the normalized mask value.
    pub threshold: f32,
    /// Minimum component area in pixels (drops specks).
    pub min_area: u32,
}

impl MaskSegmenter {
    /// Adapter over a segmentation `mask`. `threshold` = 0.5 and a small
    /// `min_area` are sensible defaults for a `[0,1]` probability/label mask.
    pub fn new(mask: GrayF32) -> Self {
        MaskSegmenter {
            mask,
            threshold: 0.5,
            min_area: 8,
        }
    }

    pub fn with_params(mask: GrayF32, threshold: f32, min_area: u32) -> Self {
        MaskSegmenter {
            mask,
            threshold,
            min_area,
        }
    }
}

impl BlobSegmenter for MaskSegmenter {
    fn segment(&self, _img: &GrayF32) -> Vec<(u32, u32, u32, u32)> {
        mask_to_boxes(&self.mask, self.threshold, self.min_area)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::cellpose::CellposeDetector;
    use crate::detect::detector::{DetectParams, GelDetector};

    /// A mask with three filled rectangles in two x-clusters (lanes).
    fn synthetic_mask() -> GrayF32 {
        let mut m = GrayF32::new(80, 120);
        // helper to paint a filled rect
        let mut paint = |x0: usize, y0: usize, x1: usize, y1: usize| {
            for y in y0..y1 {
                for x in x0..x1 {
                    m.data[[y, x]] = 1.0;
                }
            }
        };
        paint(10, 10, 24, 20); // lane A, band 1
        paint(10, 60, 24, 72); // lane A, band 2
        paint(50, 30, 64, 42); // lane B, band 1
        m
    }

    #[test]
    fn boxes_from_mask_are_correct() {
        let m = synthetic_mask();
        let boxes = mask_to_boxes(&m, 0.5, 8);
        assert_eq!(boxes.len(), 3, "three components");
        // Sorted by x-center: first two are lane A (x~10..24), last is lane B.
        assert_eq!(boxes[0], (10, 10, 24, 20));
        assert_eq!(boxes[1], (10, 60, 24, 72));
        assert_eq!(boxes[2], (50, 30, 64, 42));
    }

    #[test]
    fn specks_are_dropped() {
        let mut m = GrayF32::new(40, 40);
        m.data[[5, 5]] = 1.0; // a 1-px speck
        m.data[[6, 5]] = 1.0;
        let boxes = mask_to_boxes(&m, 0.5, 8);
        assert!(boxes.is_empty(), "2-px speck below min_area is dropped");
    }

    #[test]
    fn drives_cellpose_detector_into_lanes_and_bands() {
        // The mask defines the segmentation; a (here uniform) gel image supplies
        // densities. CellposeDetector should cluster into 2 lanes / 3 bands.
        let mask = synthetic_mask();
        let gel = {
            let mut g = GrayF32::new(80, 120);
            for v in g.data.iter_mut() {
                *v = 0.5;
            }
            g
        };
        let det = CellposeDetector::new(MaskSegmenter::new(mask));
        let d = det.detect(&gel, &DetectParams::default());
        assert_eq!(d.lanes.len(), 2, "two lanes clustered by x");
        assert_eq!(d.bands.len(), 3, "three bands total");
        assert!(d.bands.iter().all(|b| b.integrated_density > 0.0));
    }
}
