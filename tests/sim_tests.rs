use opengel::core::GrayF32;
use opengel::detect::detector::DetectParams;
use opengel::detect::orient::{auto_straighten, estimate_rotation};
use opengel::sim::{simulate, simulate_random_batch, SimConfig};

fn work_image(g: &opengel::sim::RenderedGel) -> GrayF32 {
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
fn true_warp_recovers_migration_identity_does_not() {
    use opengel::core::warp::GelWarp;

    // A gel with a pronounced smile (and a little rotation), no noise.
    let mut cfg = SimConfig::clean(11);
    cfg.smile_px = 16.0;
    cfg.rotation_deg = 4.0;
    let g = simulate(&cfg);

    use opengel::detect::eval::warp_migration_error;
    let identity = GelWarp::identity(cfg.width, cfg.height);
    let id_err = warp_migration_error(&identity, &g.truth).expect("v_true present");
    let true_err = warp_migration_error(&g.true_warp, &g.truth).expect("v_true present");

    // The exported true warp reconstructs each band's canonical migration, while
    // the naive (identity) warp carries the full smile/rotation error.
    assert!(true_err < 0.01, "true-warp migration error = {true_err}");
    assert!(
        id_err > 3.0 * true_err.max(1e-4),
        "identity error {id_err} should dwarf true-warp error {true_err}"
    );
}

#[test]
fn pipeline_fits_smile_from_two_ladders() {
    use opengel::core::warp::GelWarp;
    use opengel::detect::eval::iso_migration_spread;

    // Smiled gel with a ladder on both edges, so matched rungs span ≥2 lanes.
    let mut cfg = SimConfig::clean(3);
    cfg.two_ladders = true;
    cfg.smile_px = 14.0;
    let g = simulate(&cfg);
    let img = work_image(&g);

    let analysis = opengel::detect::analyze(&img, cfg.gel_type, &DetectParams::default(), &[], 0.9);
    let fitted = analysis.warp.expect("pipeline produced a warp");

    let identity = GelWarp::identity(cfg.width, cfg.height);
    let id_spread = iso_migration_spread(&identity, &g.truth).expect("shared ladder sizes");
    let fit_spread = iso_migration_spread(&fitted, &g.truth).expect("shared ladder sizes");

    // The smile pass fired end-to-end: the fitted warp maps the same rung in
    // both ladder lanes to a more consistent migration than the naive rectangle.
    assert!(
        fit_spread < 0.6 * id_spread,
        "fitted spread {fit_spread} should be well under identity {id_spread}"
    );
}

#[test]
fn clean_detects_ladder() {
    let cfg = SimConfig::clean(7);
    let g = simulate(&cfg);
    let img = work_image(&g);
    let analysis = opengel::detect::analyze(&img, cfg.gel_type, &DetectParams::default(), &[], 0.9);

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
        opengel::detect::analyze(&straight, cfg.gel_type, &DetectParams::default(), &[], 0.85);
    assert!(analysis.lanes.len() >= 4, "lanes recovered after straighten");
}

#[test]
fn parallel_batch_runs() {
    let gels = simulate_random_batch(8, 1000, true);
    assert_eq!(gels.len(), 8);
    // Each has some ground-truth lanes.
    assert!(gels.iter().all(|g| !g.truth.lanes.is_empty()));
}

#[test]
fn optical_flow_dewarps_smile_from_image() {
    use opengel::core::warp::GelWarp;
    use opengel::detect::eval::iso_migration_spread;

    // Strongly smiled gel. The optical-flow warp recovers the band twist from
    // the image itself — no reliance on multi-lane ladder matching.
    let mut cfg = SimConfig::clean(3);
    cfg.two_ladders = true; // only so shared rung sizes exist to *measure* spread
    cfg.smile_px = 14.0;
    let g = simulate(&cfg);
    let img = work_image(&g);

    let params = DetectParams {
        optical_flow_warp: true,
        ..DetectParams::default()
    };
    let analysis = opengel::detect::analyze(&img, cfg.gel_type, &params, &[], 0.9);
    let fitted = analysis.warp.expect("pipeline produced a warp");

    let identity = GelWarp::identity(cfg.width, cfg.height);
    let id_spread = iso_migration_spread(&identity, &g.truth).unwrap();
    let fit_spread = iso_migration_spread(&fitted, &g.truth).unwrap();

    // Same rung across lanes maps to a far more consistent migration after the
    // flow dewarp — the twisting bands have been straightened.
    assert!(
        fit_spread < 0.2 * id_spread,
        "flow spread {fit_spread} should be well under identity {id_spread}"
    );
}
