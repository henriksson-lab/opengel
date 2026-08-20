//! Reading real Bio-Rad Image Lab scans.
//!
//! These run against the sample images Image Lab ships, which are proprietary
//! and therefore not in this repository. Point `OPENGEL_SCN_SAMPLES` at the
//! `Sample Images` directory to enable them; without it every test here reports
//! that it was skipped and passes, so a checkout with no samples still gets a
//! green run.

use std::path::PathBuf;

use opengel::core::model::ChannelColor;
use opengel::core::scn::ScnFile;
use opengel::core::GelDocument;

/// The `Sample Images` directory, if explicitly configured.
fn samples() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("OPENGEL_SCN_SAMPLES") {
        let p = PathBuf::from(dir);
        return p.is_dir().then_some(p);
    }
    None
}

/// Every scan in the sample directory, sorted for a stable report.
fn all_scans() -> Vec<PathBuf> {
    let Some(dir) = samples() else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| opengel::core::scn::has_scn_extension(p))
        .collect();
    out.sort();
    out
}

macro_rules! skip_without_samples {
    () => {
        if samples().is_none() {
            eprintln!("skipped: set OPENGEL_SCN_SAMPLES to the Image Lab sample directory");
            return;
        }
    };
}

#[test]
fn every_shipped_sample_parses() {
    skip_without_samples!();
    let scans = all_scans();
    assert!(!scans.is_empty(), "sample directory holds no scans");

    for path in &scans {
        let name = path.file_name().unwrap().to_string_lossy();
        let file = ScnFile::load(path).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(!file.channels.is_empty(), "{name}: no channels");
        for (i, c) in file.channels.iter().enumerate() {
            assert!(
                c.width > 0 && c.height > 0,
                "{name} channel {i}: empty geometry"
            );
            // The decoded buffer must match the declared geometry exactly —
            // this is what catches a container split in the wrong place.
            assert_eq!(
                c.image.to_luma16().len(),
                c.width as usize * c.height as usize,
                "{name} channel {i}: pixel count disagrees with <size_pix>"
            );
        }
    }
    eprintln!("parsed {} sample scans", scans.len());
}

#[test]
fn the_multichannel_sample_has_three_distinct_channels() {
    skip_without_samples!();
    let path = samples().unwrap().join("MP Multichannel Blot B.mscn");
    if !path.exists() {
        eprintln!("skipped: this sample set has no MP Multichannel Blot B.mscn");
        return;
    }
    let file = ScnFile::load(&path).expect("parse");

    assert_eq!(file.channels.len(), 3);
    // Blue/Green/Red, in the order the session header assigns them.
    let colors: Vec<ChannelColor> = file.channels.iter().map(|c| c.color).collect();
    assert_eq!(
        colors,
        vec![ChannelColor::Blue, ChannelColor::Green, ChannelColor::Red]
    );
    // Every channel is the same gel, so they must share geometry.
    let geom: Vec<(u32, u32)> = file.channels.iter().map(|c| (c.width, c.height)).collect();
    assert_eq!(geom, vec![(1392, 1040); 3]);
    // But each was a separate acquisition, with its own exposure — spanning
    // more than twenty-fold here, which is exactly why channels must not be
    // merged into one another the way an exposure bracket is.
    let exposures: Vec<f64> = file.channels.iter().map(|c| c.exposure_seconds).collect();
    assert_eq!(exposures, vec![0.836, 1.534, 18.913]);
    // And its own illumination, which is the point of a multichannel scan.
    let apps: Vec<&str> = file.channels.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(apps, vec!["Stain Free Blot", "DyLight 549", "DyLight 650"]);
    let excitation: Vec<&str> = file
        .channels
        .iter()
        .map(|c| {
            c.acquisition
                .iter()
                .find(|a| a.name == "Excitation Source")
                .map(|a| a.value.as_str())
                .unwrap_or("")
        })
        .collect();
    assert_eq!(
        excitation,
        vec![
            "UV Trans illumination",
            "Green Epi illumination",
            "Red Epi illumination"
        ]
    );
    assert!(file
        .channels
        .iter()
        .all(|c| c.imager.as_deref() == Some("ChemiDoc™ MP")));
}

#[test]
fn the_gel_doc_ez_sample_matches_the_documented_geometry() {
    skip_without_samples!();
    let path = samples().unwrap().join("EZ EtBr.scn");
    if !path.exists() {
        eprintln!("skipped: this sample set has no EZ EtBr.scn");
        return;
    }
    let file = ScnFile::load(&path).expect("parse");
    assert_eq!(file.channels.len(), 1);
    let c = &file.channels[0];

    // 1392 × 1000 stored, cropped from the ICX205's native 1392 × 1040.
    assert_eq!((c.width, c.height), (1392, 1000));
    assert_eq!(c.original_size, Some((1392, 1040)));
    assert_eq!(c.size_mm, Some((139.2, 100.0)));
    // 12-bit data: the ceiling is recorded, and the samples are rescaled.
    assert_eq!(c.max_value, 4095);
    assert_eq!(c.exposure_seconds, 0.362);
    assert_eq!(c.name, "Ethidium Bromide");
    assert_eq!(c.imager.as_deref(), Some("Gel Doc™ EZ"));
    assert_eq!(c.timestamp.as_deref(), Some("2010-06-18T13:15:16"));

    // Ethidium bromide stains DNA, so the document should open in base pairs.
    assert_eq!(file.gel_type(), opengel::core::GelType::Dna);

    // The acquisition record is carried whole, including the fields specific to
    // this instrument.
    let names: Vec<&str> = c.acquisition.iter().map(|a| a.name.as_str()).collect();
    for expect in [
        "Imager",
        "Image Date",
        "Exposure Time (sec)",
        "Application",
        "Flat Field",
        "Serial Number",
        "Illumination Mode",
    ] {
        assert!(
            names.contains(&expect),
            "missing acquisition field {expect}"
        );
    }
}

#[test]
fn a_blot_asks_to_be_displayed_inverted() {
    skip_without_samples!();
    let path = samples().unwrap().join("XRS Blot Chemi.scn");
    if !path.exists() {
        eprintln!("skipped: this sample set has no XRS Blot Chemi.scn");
        return;
    }
    let file = ScnFile::load(&path).expect("parse");
    // `zero_is="white"` — dark bands on white, the conventional presentation.
    assert!(file.display_inverted);
    assert_eq!(file.channels[0].max_value, 65535);
}

#[test]
fn a_scan_survives_a_round_trip_through_our_own_container() {
    skip_without_samples!();
    let Some(path) = all_scans().into_iter().find(|p| {
        p.extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("mscn"))
    }) else {
        eprintln!("skipped: this sample set has no multichannel scan");
        return;
    };

    let scn = ScnFile::load(&path).expect("parse");
    let doc = GelDocument::from_scn(&scn);
    assert_eq!(doc.project.channels.len(), scn.channels.len());
    assert_eq!(doc.frames.len(), scn.channels.len());
    assert!(doc.project.is_multichannel());

    let bytes = doc.to_bytes().expect("write .gel.zip");
    let back = GelDocument::from_bytes(&bytes).expect("read .gel.zip");

    assert_eq!(back.project.channels.len(), doc.project.channels.len());
    assert_eq!(back.project.gel_type, doc.project.gel_type);
    assert_eq!(back.project.display_inverted, doc.project.display_inverted);
    for (a, b) in back.project.channels.iter().zip(&doc.project.channels) {
        assert_eq!((a.id, &a.name, a.color), (b.id, &b.name, b.color));
    }
    // The acquisition record has to survive the trip — it is the part a user
    // cannot reconstruct later.
    for (a, b) in back.project.images.iter().zip(&doc.project.images) {
        assert_eq!(a.channel, b.channel);
        assert_eq!(a.meta.acquisition, b.meta.acquisition);
        assert_eq!(a.meta.exposure_seconds, b.meta.exposure_seconds);
    }
    // Each channel resolves to its own working image, not a merge of all three.
    for channel in &back.project.channels {
        let img = back
            .working_image_for_channel(channel.id)
            .expect("a working image per channel");
        assert_eq!(
            img.width(),
            scn.channels[channel.id as usize].width as usize
        );
    }
    // Pixels must come back bit-exact: 16-bit PNG is lossless, and a quiet
    // downconversion to 8-bit here would throw away most of the dynamic range.
    for (i, frame) in back.frames.iter().enumerate() {
        assert_eq!(
            frame.to_luma16().into_raw(),
            scn.channels[i].image.to_luma16().into_raw(),
            "channel {i} pixels changed in the round trip"
        );
    }
}
