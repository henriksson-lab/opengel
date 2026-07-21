# OpenGel

Capture, detect and quantify gel electrophoresis images

**under development**

Supports **DNA, RNA and protein** gels. Captures from a USB camera with
exposure control (including multi-exposure **HDR** for dynamic range), stores
everything in a self-contained `.gel.zip`, auto-detects lanes/bands and the
ladder, and quantifies band amounts in **ng and molarity** — absolutely (against
a ladder of known concentration) and relatively (between two selected bands).

## The `.gel.zip` format

A ZIP container:

```
manifest.json   { format, version, gel_type }
metadata.json   [ per-image capture params: exposure, gain, camera, bracket ]
analysis.json   lanes, bands, blobs, ladder assignments, calibration, quant
images/img_NN.png   raw captures (8- or 16-bit)
```

## Detection & quantification

* **Lanes** — column-sum intensity profile; lanes are the peaks (`gel-detect/src/classical.rs`).
* **Bands** — per-lane vertical densitometry trace, rolling-ball baseline
  subtraction, peak detection + integration (`gel-detect/src/signal.rs`).
* **Ladder ID** — match a lane's band pattern to a commercial template via a
  semi-log fit (`ln(size) ∝ migration`); pick the best-explained lane
  (`gel-detect/src/ladder_match.rs`).
* **Sizing** — semi-log calibration from the ladder sizes every other band.
* **Amounts** — intensity→mass calibration from ladder bands of known mass;
  molarity via `mass / (size × g·mol⁻¹·unit⁻¹)` (650/bp DNA, 340/nt RNA, Da for
  protein) — `gel-core/src/quant.rs`.
* **Cellpose** — plug real bindings into `gel_detect::cellpose::BlobSegmenter`;
  `CellposeDetector` clusters blobs into lanes/bands. Benchmark it against the
  classical detector with the eval harness.

The detector to ship as default is decided by numbers, not assertion: the
**evaluation harness** (`gel-detect/src/eval.rs`) scores any `GelDetector`
against Claude-annotated ground truth (lane IoU, band precision/recall, position
error).

## Build & run

```sh
cargo build                      # whole workspace (mock camera, no GUI deps issue)
cargo test                       # all crates

# CLI
cargo run -p gel-cli -- make-demo demo.gel.zip
cargo run -p gel-cli -- analyze demo.gel.zip
cargo run -p gel-cli -- info demo.gel.zip
cargo run -p gel-cli -- ladders --gel-type dna
cargo run -p gel-cli -- make-dataset datasets/demo
cargo run -p gel-cli -- eval datasets/demo

# Simulator: render degraded gels (rotation, warp, background, overexposure,
# run-out-of-frame, Poisson noise) with exact ground truth, in parallel, then
# benchmark the detector on them.
cargo run -p gel-cli -- simulate datasets/sim --count 50 --seed 1 --eval
cargo run -p gel-cli -- simulate datasets/sim --count 50 --upright --eval   # no rotation

# Analyze a loose image (jpg/png/tif), not just a .gel.zip
cargo run -p gel-cli -- analyze path/to/gel.jpg --out out.gel.zip

# Real annotated gels: convert GelGenie segmentation masks to ground truth,
# then benchmark the detector on them (see datasets/real_gels/).
cargo run -p gel-cli -- import-masks datasets/real_gels/gelgenie/quantitation_ladder_gels
cargo run -p gel-cli -- eval        datasets/real_gels/gelgenie/quantitation_ladder_gels/test_images

# GUI (needs a display)
cargo run -p gel-app
cargo run -p gel-app -- demo.gel.zip          # open a file on launch
cargo run -p gel-app --features camera        # enable real USB capture
```


## Datasets

`datasets/real_gels/` holds ~1,270 openly-licensed real gel images plus 575
hand-labelled band masks (GelGenie, CC BY 4.0; rice CAPS gels, CC BY-SA 2.1 JP;
Wikimedia/PLOS mixed) — image blobs are git-ignored, source/license sidecars and
manifests are tracked. `gel import-masks` turns GelGenie masks into eval ground
truth.

