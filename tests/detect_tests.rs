use opengel::core::model::{GelType, LadderBand, LadderTemplate};
use opengel::core::GrayF32;
use opengel::detect::detector::{DetectParams, GelDetector};
use opengel::detect::eval::{aggregate, evaluate, GroundTruth, GtBand, GtLane};
use opengel::detect::{analyze, ClassicalDetector};
use ndarray::Array2;

const W: usize = 200;
const H: usize = 300;

// Semi-log model used to place the synthetic ladder.
fn ladder_size(y: f64) -> f64 {
    (-0.008 * y + 8.0).exp()
}

struct BandSpec {
    x: f64,
    y: f64,
}

/// Render a synthetic bright-on-dark gel from a list of Gaussian bands.
fn synth(bands: &[BandSpec]) -> GrayF32 {
    let mut data = Array2::<f32>::zeros((H, W));
    let (sx, sy) = (5.0f64, 4.0f64);
    for b in bands {
        let x0 = (b.x - 4.0 * sx).max(0.0) as usize;
        let x1 = (b.x + 4.0 * sx).min(W as f64) as usize;
        let y0 = (b.y - 4.0 * sy).max(0.0) as usize;
        let y1 = (b.y + 4.0 * sy).min(H as f64) as usize;
        for y in y0..y1 {
            for x in x0..x1 {
                let dx = (x as f64 - b.x) / sx;
                let dy = (y as f64 - b.y) / sy;
                let v = 0.85 * (-0.5 * (dx * dx + dy * dy)).exp();
                data[[y, x]] += v as f32;
            }
        }
    }
    data.mapv_inplace(|v| v.min(1.0));
    GrayF32 { data }
}

fn ladder_ys() -> Vec<f64> {
    vec![40.0, 90.0, 140.0, 190.0, 240.0]
}

fn build_gel() -> (GrayF32, LadderTemplate) {
    let mut bands = Vec::new();
    // Lane 0 (x=30): the ladder, 5 rungs.
    for &y in &ladder_ys() {
        bands.push(BandSpec { x: 30.0, y });
    }
    // Sample lanes at x = 70, 110, 150 with 3 bands each.
    for &x in &[70.0, 110.0, 150.0] {
        for &y in &[70.0, 150.0, 210.0] {
            bands.push(BandSpec { x, y });
        }
    }
    let template = LadderTemplate {
        name: "test-ladder".into(),
        gel_type: GelType::Dna,
        vendor: None,
        catalog: None,
        standard_load_ng: None,
        bands: ladder_ys()
            .iter()
            .map(|&y| LadderBand {
                size: ladder_size(y),
                mass_ng: None,
                reference: false,
            })
            .collect(),
    };
    (synth(&bands), template)
}

#[test]
fn classical_finds_lanes_and_bands() {
    let (img, _) = build_gel();
    let det = ClassicalDetector::new();
    let out = det.detect(&img, &DetectParams::default());
    assert_eq!(out.lanes.len(), 4, "expected 4 lanes");
    // Ladder lane should have 5 bands, sample lanes 3 each => 5 + 9 = 14.
    assert_eq!(out.bands.len(), 14, "expected 14 bands total");
    // Leftmost lane brackets x=30.
    let l0 = &out.lanes[0];
    assert!(l0.x_min <= 30 && l0.x_max >= 30);
}

#[test]
fn pipeline_identifies_ladder_and_sizes_bands() {
    let (img, template) = build_gel();
    let cands = [&template];
    let analysis = analyze(&img, GelType::Dna, &DetectParams::default(), &cands, 0.9);

    // The leftmost lane is the ladder.
    let ladder_lane = analysis.lanes.iter().find(|l| l.is_ladder).expect("a ladder lane");
    assert_eq!(ladder_lane.id, 0);
    assert_eq!(analysis.ladder_assignments.len(), 1);
    assert_eq!(analysis.ladder_assignments[0].template_name, "test-ladder");

    // Ladder bands get known sizes matching the template (top→bottom).
    let mut ladder_bands: Vec<_> = analysis
        .bands
        .iter()
        .filter(|b| b.lane_id == ladder_lane.id)
        .collect();
    ladder_bands.sort_by(|a, b| a.y_center.partial_cmp(&b.y_center).unwrap());
    assert_eq!(ladder_bands.len(), 5);
    for (band, &y) in ladder_bands.iter().zip(&ladder_ys()) {
        let expected = ladder_size(y);
        let got = band.known_size.unwrap();
        assert!((got - expected).abs() / expected < 0.01);
    }

    // Sample bands get interpolated sizes that decrease with migration (y).
    let mut sample: Vec<_> = analysis
        .bands
        .iter()
        .filter(|b| b.lane_id != ladder_lane.id)
        .collect();
    assert!(!sample.is_empty());
    for b in &sample {
        let s = b.size.expect("sample band sized");
        assert!(s > 0.0 && s < 1e6);
    }
    // Within a sample lane, larger y => smaller size.
    sample.retain(|b| b.lane_id == 1);
    sample.sort_by(|a, b| a.y_center.partial_cmp(&b.y_center).unwrap());
    for w in sample.windows(2) {
        assert!(w[0].size.unwrap() > w[1].size.unwrap());
    }
}

#[test]
fn eval_harness_scores_perfect_on_ground_truth() {
    let (img, _) = build_gel();
    let det = ClassicalDetector::new();

    // Ground truth mirrors the synthetic construction.
    let mut lanes = vec![GtLane {
        x_min: 20,
        x_max: 40,
        is_ladder: true,
        ladder_name: Some("test-ladder".into()),
        bands: ladder_ys().iter().map(|&y| GtBand { y_center: y, size: None }).collect(),
    }];
    for &x in &[70u32, 110, 150] {
        lanes.push(GtLane {
            x_min: x - 10,
            x_max: x + 10,
            is_ladder: false,
            ladder_name: None,
            bands: [70.0, 150.0, 210.0]
                .iter()
                .map(|&y| GtBand { y_center: y, size: None })
                .collect(),
        });
    }
    let gt = GroundTruth {
        image: "synthetic".into(),
        gel_type: GelType::Dna,
        lanes,
    };

    let m = evaluate(&det, &img, &DetectParams::default(), &gt, 6.0);
    assert_eq!(m.lane_count_pred, 4);
    assert_eq!(m.band_false_negatives, 0, "no missed bands");
    assert_eq!(m.band_false_positives, 0, "no spurious bands");
    assert!(m.band_precision() > 0.99 && m.band_recall() > 0.99);
    assert!(m.lane_iou_mean > 0.4);

    let agg = aggregate(&[m]);
    assert_eq!(agg.images, 1);
    assert!(agg.mean_band_f1 > 0.99);
}
