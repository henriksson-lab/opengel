//! Cellpose-based ("blob-first") detector.
//!
//! Cellpose (or any instance segmenter) plugs in via [`BlobSegmenter`]: the
//! user's Rust Cellpose bindings implement `segment` to return blob bounding
//! boxes. [`CellposeDetector`] then clusters blobs into lanes by x-position and
//! turns each blob into a band with its integrated density. This lets the
//! evaluation harness benchmark the blob-first approach against the classical
//! detector on identical ground truth.

use crate::core::GrayF32;

use crate::detect::detector::{DetBand, DetLane, DetectParams, Detection, GelDetector};

/// A blob/mask segmenter. Returns blob bounding boxes as
/// `(x_min, y_min, x_max, y_max)` in pixels (x_max/y_max exclusive).
pub trait BlobSegmenter {
    fn segment(&self, img: &GrayF32) -> Vec<(u32, u32, u32, u32)>;
}

/// Detector that turns segmented blobs into lanes + bands.
pub struct CellposeDetector<S: BlobSegmenter> {
    segmenter: S,
    /// Max x-gap (px) between blob centers within the same lane cluster.
    pub lane_gap: u32,
}

impl<S: BlobSegmenter> CellposeDetector<S> {
    pub fn new(segmenter: S) -> Self {
        CellposeDetector {
            segmenter,
            lane_gap: 20,
        }
    }
}

fn integrated_density(img: &GrayF32, bbox: (u32, u32, u32, u32)) -> f64 {
    let (x0, y0, x1, y1) = bbox;
    let mut sum = 0.0;
    for y in y0..y1.min(img.height() as u32) {
        for x in x0..x1.min(img.width() as u32) {
            sum += img.get(x as usize, y as usize) as f64;
        }
    }
    sum
}

impl<S: BlobSegmenter> GelDetector for CellposeDetector<S> {
    fn name(&self) -> &str {
        "cellpose"
    }

    fn detect(&self, img: &GrayF32, params: &DetectParams) -> Detection {
        let work = if params.signal_is_bright {
            img.clone()
        } else {
            img.inverted()
        };
        let mut blobs = self.segmenter.segment(&work);
        if blobs.is_empty() {
            return Detection::default();
        }
        // Sort by x-center to cluster into lanes.
        blobs.sort_by_key(|b| (b.0 + b.2) / 2);

        let h = work.height() as u32;
        let mut lanes: Vec<DetLane> = Vec::new();
        let mut bands: Vec<DetBand> = Vec::new();
        let mut lane_blobs: Vec<(u32, u32, u32, u32)> = Vec::new();
        let mut band_id = 0u32;

        let flush = |lanes: &mut Vec<DetLane>,
                     bands: &mut Vec<DetBand>,
                     band_id: &mut u32,
                     group: &[(u32, u32, u32, u32)]| {
            if group.is_empty() {
                return;
            }
            let lane_id = lanes.len() as u32;
            let x_min = group.iter().map(|b| b.0).min().unwrap();
            let x_max = group.iter().map(|b| b.2).max().unwrap();
            lanes.push(DetLane {
                id: lane_id,
                x_min,
                x_max,
                y_min: 0,
                y_max: h,
                is_ladder: false,
            });
            let mut sorted = group.to_vec();
            sorted.sort_by_key(|b| b.1 + b.3);
            for b in sorted {
                let y_center = (b.1 + b.3) as f64 / 2.0;
                let y_half = (b.3 as f64 - b.1 as f64) / 2.0;
                bands.push(DetBand {
                    id: *band_id,
                    lane_id,
                    x_center: (b.0 as f64 + b.2 as f64) / 2.0,
                    y_center,
                    y_half_width: y_half.max(0.5),
                    integrated_density: 0.0, // filled by caller (needs img)
                    angle: 0.0,              // mask segmenters don't measure tilt
                });
                *band_id += 1;
            }
        };

        let mut last_cx: Option<u32> = None;
        for b in &blobs {
            let cx = (b.0 + b.2) / 2;
            if let Some(prev) = last_cx {
                if cx.saturating_sub(prev) > self.lane_gap {
                    flush(&mut lanes, &mut bands, &mut band_id, &lane_blobs);
                    lane_blobs.clear();
                }
            }
            lane_blobs.push(*b);
            last_cx = Some(cx);
        }
        flush(&mut lanes, &mut bands, &mut band_id, &lane_blobs);

        // Fill integrated densities from the blob boxes.
        for (band, blob) in bands
            .iter_mut()
            .zip(sorted_by_lane_then_y(&blobs, self.lane_gap))
        {
            band.integrated_density = integrated_density(&work, blob);
        }

        Detection { lanes, bands }
    }
}

/// Reproduce the same lane/band ordering used above so densities line up with
/// bands. (Blobs sorted by x, clustered by `lane_gap`, then by y within lane.)
fn sorted_by_lane_then_y(
    blobs: &[(u32, u32, u32, u32)],
    lane_gap: u32,
) -> Vec<(u32, u32, u32, u32)> {
    let mut out = Vec::new();
    let mut group: Vec<(u32, u32, u32, u32)> = Vec::new();
    let mut last_cx: Option<u32> = None;
    let push_group = |group: &mut Vec<(u32, u32, u32, u32)>, out: &mut Vec<_>| {
        group.sort_by_key(|b: &(u32, u32, u32, u32)| b.1 + b.3);
        out.append(group);
    };
    for b in blobs {
        let cx = (b.0 + b.2) / 2;
        if let Some(prev) = last_cx {
            if cx.saturating_sub(prev) > lane_gap {
                push_group(&mut group, &mut out);
            }
        }
        group.push(*b);
        last_cx = Some(cx);
    }
    push_group(&mut group, &mut out);
    out
}
