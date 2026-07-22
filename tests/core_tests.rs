use opengel::core::hdr::merge_hdr;
use opengel::core::imagef32::GrayF32;
use opengel::core::model::CaptureMeta;
use opengel::core::quant::{compare, mass_ng_to_nmol, nmol_to_molar, SizingFit};
use opengel::core::{ladders, Calibration, GelDocument, GelType};
use image::{DynamicImage, GrayImage, Luma};

fn solid(w: u32, h: u32, v: u8) -> DynamicImage {
    DynamicImage::ImageLuma8(GrayImage::from_pixel(w, h, Luma([v])))
}

#[test]
fn gel_zip_roundtrip() {
    let frames = vec![solid(16, 12, 40), solid(16, 12, 200)];
    let metas = vec![
        CaptureMeta {
            exposure_seconds: 0.1,
            bracket_group: Some(0),
            ..Default::default()
        },
        CaptureMeta {
            exposure_seconds: 0.5,
            bracket_group: Some(0),
            ..Default::default()
        },
    ];
    let mut doc = GelDocument::from_frames(GelType::Dna, frames, metas);
    // Add a lane so we exercise analysis serialization too.
    doc.project.analysis.lanes.push(opengel::core::Lane {
        id: 0,
        u_min: 0.125,
        u_max: 0.875,
        label: Some("L1".into()),
        is_ladder: true,
    });

    let bytes = doc.to_bytes().unwrap();
    let back = GelDocument::from_bytes(&bytes).unwrap();

    assert_eq!(back.project.gel_type, GelType::Dna);
    assert_eq!(back.project.images.len(), 2);
    assert_eq!(back.frames.len(), 2);
    assert_eq!(back.project.images[1].meta.exposure_seconds, 0.5);
    assert_eq!(back.project.analysis.lanes.len(), 1);
    assert!(back.project.analysis.lanes[0].is_ladder);
    assert_eq!(back.frames[0].width(), 16);
}

#[test]
fn hdr_recovers_linear_radiance() {
    // Same scene at two exposures: pixel value scales with exposure time.
    // radiance = v / t should agree between frames.
    let short = GrayF32 {
        data: ndarray::arr2(&[[0.1f32, 0.2], [0.3, 0.15]]),
    };
    let long = GrayF32 {
        data: ndarray::arr2(&[[0.4f32, 0.8], [0.9, 0.6]]),
    };
    // long exposure is 4x the short one, values are 4x (still unsaturated).
    let merged = merge_hdr(&[short, long], &[0.1, 0.4]).unwrap();
    // radiance for pixel [0,0]: 0.1/0.1 = 1.0 and 0.4/0.4 = 1.0.
    assert!((merged.get(0, 0) - 1.0).abs() < 1e-4);
    assert!((merged.get(1, 0) - 2.0).abs() < 1e-4);
}

#[test]
fn hdr_rejects_bad_input() {
    let f = GrayF32::new(2, 2);
    assert!(merge_hdr(&[], &[]).is_err());
    assert!(merge_hdr(&[f.clone()], &[0.0]).is_err());
    assert!(merge_hdr(&[f], &[0.1, 0.2]).is_err());
}

#[test]
fn sizing_semilog_fit() {
    // Construct a perfect semi-log ladder: size = exp(-0.01 * pos + 9).
    let a = -0.01;
    let b = 9.0;
    let pts: Vec<(f64, f64)> = (0..8)
        .map(|i| {
            let pos = i as f64 * 50.0;
            (pos, (a * pos + b).exp())
        })
        .collect();
    let fit = SizingFit::fit(&pts).unwrap();
    assert!((fit.a - a).abs() < 1e-6);
    assert!((fit.b - b).abs() < 1e-6);
    // Round-trip a size through position.
    let size = 1000.0;
    let pos = fit.position_at(size).unwrap();
    assert!((fit.size_at(pos) - size).abs() < 1e-3);
}

#[test]
fn calibration_linear_fit_and_predict() {
    let pts = [(10.0, 5.0), (20.0, 10.0), (30.0, 15.0)]; // mass = 0.5*density
    let cal = Calibration::fit_linear(&pts).unwrap();
    match cal {
        Calibration::Linear { slope } => assert!((slope - 0.5).abs() < 1e-9),
        _ => panic!("expected linear"),
    }
    assert!((cal.mass_ng(40.0) - 20.0).abs() < 1e-9);
}

#[test]
fn molarity_dna() {
    // 650 ng of a 1000 bp dsDNA fragment.
    // g/mol = 1000 * 650 = 650000; mol = 650e-9/650000 = 1e-12 mol = 1e-3 nmol.
    let nmol = mass_ng_to_nmol(650.0, 1000.0, GelType::Dna).unwrap();
    assert!((nmol - 1e-3).abs() < 1e-9);
    // Concentration in 10 uL: 1e-3 nmol = 1e-12 mol in 1e-5 L = 1e-7 mol/L.
    let molar = nmol_to_molar(nmol, 10.0).unwrap();
    assert!((molar - 1e-7).abs() < 1e-12);
}

#[test]
fn molarity_protein_uses_da_directly() {
    // 50000 Da protein, 50 ng. g/mol = 50000. mol = 50e-9/50000 = 1e-12 mol.
    let nmol = mass_ng_to_nmol(50.0, 50000.0, GelType::Protein).unwrap();
    assert!((nmol - 1e-3).abs() < 1e-9);
}

#[test]
fn relative_comparison() {
    // Blob A twice as dense as B, same size -> mass ratio 2, molar ratio 2.
    let r = compare(200.0, Some(500.0), 100.0, Some(500.0)).unwrap();
    assert!((r.mass_ratio - 2.0).abs() < 1e-9);
    assert!((r.molar_ratio.unwrap() - 2.0).abs() < 1e-9);
    // Same mass but A is twice the size -> half the moles.
    let r2 = compare(100.0, Some(1000.0), 100.0, Some(500.0)).unwrap();
    assert!((r2.mass_ratio - 1.0).abs() < 1e-9);
    assert!((r2.molar_ratio.unwrap() - 0.5).abs() < 1e-9);
}

#[test]
fn ladder_database_loads() {
    assert!(!ladders::all().is_empty());
    let dna = ladders::for_gel_type(GelType::Dna);
    assert!(dna.iter().any(|t| t.name.contains("1 kb")));
    let neb = ladders::by_name("NEB 1 kb DNA Ladder").unwrap();
    assert_eq!(neb.bands.len(), 10);
    // Bands ordered largest-first.
    assert!(neb.bands[0].size > neb.bands[1].size);
    // Metadata: catalog number, per-band ng, and a reference (extra-thick) band.
    assert_eq!(neb.catalog.as_deref(), Some("N3232"));
    assert_eq!(neb.vendor.as_deref(), Some("NEB"));
    assert!(neb.bands.iter().any(|b| b.mass_ng.is_some()));
    assert_eq!(neb.reference_sizes(), vec![3000.0]);
    // Every ladder has a vendor + catalog and stays largest-first.
    for t in ladders::all() {
        assert!(t.vendor.is_some(), "{} missing vendor", t.name);
        assert!(t.catalog.is_some(), "{} missing catalog", t.name);
        for w in t.bands.windows(2) {
            assert!(w[0].size > w[1].size, "{} not largest-first", t.name);
        }
    }
}
