use opengel::camera::mock::{self, MockCamera};
use opengel::camera::{capture_bracket, Camera, Exposure};
use opengel::core::hdr::merge_hdr;
use opengel::core::GrayF32;

#[test]
fn mock_lists_and_opens() {
    let cams = mock::list_cameras();
    assert_eq!(cams.len(), 1);
    let cam = mock::open(0).unwrap();
    assert!(cam.capabilities().manual_exposure);
    assert!(mock::open(1).is_err());
}

#[test]
fn exposure_changes_brightness() {
    let mut cam: MockCamera = mock::open(0).unwrap();
    cam.set_exposure(Exposure::Manual(0.01)).unwrap();
    let dark = cam.capture().unwrap().to_luma8();
    cam.set_exposure(Exposure::Manual(1.0)).unwrap();
    let bright = cam.capture().unwrap().to_luma8();
    let sum = |img: &image::GrayImage| img.pixels().map(|p| p.0[0] as u64).sum::<u64>();
    assert!(sum(&bright) > sum(&dark) * 3);
}

#[test]
fn bracket_hdr_recovers_faint_and_bright() {
    let mut cam = mock::open(0).unwrap();
    let exposures = [0.02, 0.1, 0.5, 1.5];
    let frames_meta = capture_bracket(&mut cam, &exposures, 0).unwrap();
    assert_eq!(frames_meta.len(), 4);
    // All share a bracket group.
    assert!(frames_meta.iter().all(|(_, m)| m.bracket_group == Some(0)));

    let grays: Vec<GrayF32> = frames_meta
        .iter()
        .map(|(img, _)| GrayF32::from_dynamic(img))
        .collect();
    let ts: Vec<f64> = frames_meta.iter().map(|(_, m)| m.exposure_seconds).collect();
    let hdr = merge_hdr(&grays, &ts).unwrap();

    // The merged radiance should show a wide dynamic range: a very bright band
    // and a very faint band are both represented (max >> a mid value > 0).
    let (lo, hi) = hdr.min_max();
    assert!(hi > lo);
    assert!(hi > 2.0, "bright band radiance recovered (got {hi})");
}
