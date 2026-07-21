use gel_core::GrayF32;
use gel_detect::detector::DetectParams;
use gel_detect::orient::{auto_straighten, estimate_rotation};
use gel_sim::{simulate, simulate_random_batch, SimConfig};

fn work_image(g: &gel_sim::RenderedGel) -> GrayF32 {
    GrayF32::from_dynamic(&g.image)
}

#[test]
fn deterministic_per_seed() {
    let a = simulate(&SimConfig::randomized(42));
    let b = simulate(&SimConfig::randomized(42));
    assert_eq!(a.image.to_luma8().into_raw(), b.image.to_luma8().into_raw());
    // Ground truth stable too.
    assert_eq!(a.truth.lanes.len(), b.truth.lanes.len());
}

#[test]
fn clean_gel_detects_ladder() {
    let cfg = SimConfig::clean(7);
    let g = simulate(&cfg);
    let img = work_image(&g);
    let analysis = gel_detect::analyze(&img, cfg.gel_type, &DetectParams::default(), &[], 0.9);

    assert!(analysis.lanes.len() >= 4, "lanes detected");
    assert!(
        analysis
            .ladder_assignments
            .iter()
            .any(|a| a.template_name.contains("1 kb")),
        "NEB 1 kb ladder identified on a clean sim"
    );
}

#[test]
fn run_out_of_frame_drops_bands() {
    // Large shift pushes part of the gel out of the image → fewer GT bands than
    // an unshifted render.
    let mut base = SimConfig::clean(3);
    base.n_sample_lanes = 4;
    let full = simulate(&base);
    let full_bands: usize = full.truth.lanes.iter().map(|l| l.bands.len()).sum();

    let mut shifted = base.clone();
    shifted.shift_px = (140.0, 0.0);
    let cut = simulate(&shifted);
    let cut_bands: usize = cut.truth.lanes.iter().map(|l| l.bands.len()).sum();

    assert!(cut_bands < full_bands, "some bands ran out of frame");
}

#[test]
fn rotation_is_recovered_by_autostraighten() {
    // Rotate a clean gel and check auto-straighten recovers the magnitude and
    // increases projection sharpness (lanes become vertical).
    let mut cfg = SimConfig::clean(11);
    cfg.rotation_deg = 22.0;
    let g = simulate(&cfg);
    let img = work_image(&g);

    let est = estimate_rotation(&img, 50.0, true);
    // Magnitude should match ~22° (sign depends on convention).
    assert!((est.abs() - 22.0).abs() < 6.0, "estimated {est}, expected ~±22");

    let (straight, _angle) = auto_straighten(&img, 50.0, true);
    // After straightening, the ladder should be identifiable again.
    let analysis =
        gel_detect::analyze(&straight, cfg.gel_type, &DetectParams::default(), &[], 0.85);
    assert!(analysis.lanes.len() >= 4, "lanes recovered after straighten");
}

#[test]
fn parallel_batch_runs() {
    let gels = simulate_random_batch(8, 1000, true);
    assert_eq!(gels.len(), 8);
    // Each has some ground-truth lanes.
    assert!(gels.iter().all(|g| !g.truth.lanes.is_empty()));
}
