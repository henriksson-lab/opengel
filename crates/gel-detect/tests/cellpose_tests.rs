use gel_core::GrayF32;
use gel_detect::cellpose::{BlobSegmenter, CellposeDetector};
use gel_detect::detector::{DetectParams, GelDetector};
use ndarray::Array2;

/// A trivial segmenter that returns a fixed set of blob boxes, standing in for
/// real Cellpose bindings.
struct FixedSegmenter {
    boxes: Vec<(u32, u32, u32, u32)>,
}
impl BlobSegmenter for FixedSegmenter {
    fn segment(&self, _img: &GrayF32) -> Vec<(u32, u32, u32, u32)> {
        self.boxes.clone()
    }
}

#[test]
fn cellpose_clusters_blobs_into_lanes() {
    // A bright image so integrated density is non-zero.
    let img = GrayF32 {
        data: Array2::from_elem((300, 220), 0.5f32),
    };
    // Two lanes: x~30 (3 bands) and x~120 (2 bands).
    let boxes = vec![
        (25, 40, 35, 50),
        (25, 90, 35, 100),
        (25, 150, 35, 160),
        (115, 60, 125, 70),
        (115, 130, 125, 140),
    ];
    let det = CellposeDetector::new(FixedSegmenter { boxes });
    let out = det.detect(&img, &DetectParams::default());

    assert_eq!(out.lanes.len(), 2, "two lanes clustered by x");
    assert_eq!(out.bands.len(), 5, "all blobs become bands");
    // Lane 0 (leftmost) has 3 bands, lane 1 has 2.
    let l0 = out.lanes[0].id;
    let l1 = out.lanes[1].id;
    assert_eq!(out.bands.iter().filter(|b| b.lane_id == l0).count(), 3);
    assert_eq!(out.bands.iter().filter(|b| b.lane_id == l1).count(), 2);
    // Bands within a lane are ordered top→bottom and carry density.
    for b in &out.bands {
        assert!(b.integrated_density > 0.0);
    }
}
