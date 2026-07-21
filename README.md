# OpenGel

Capture, detect and quantify gel electrophoresis images

**under development**

Supports **DNA, RNA and protein** gels. Captures from a USB camera with
exposure control (including multi-exposure **HDR** for dynamic range), stores
everything in a self-contained `.gel.zip`, auto-detects lanes/bands and the
ladder, and quantifies band amounts in **ng and molarity** — absolutely (against
a ladder of known concentration) and relatively (between two selected bands).

## Project layout

A single `opengel` crate; each subdirectory of `src/` was previously its own
crate:

| Module | What it is |
|--------|------------|
| `src/gel_core` | Data model, `.gel.zip` IO, HDR merge, ladder database, quant math. |
| `src/gel_detect` | Pluggable detectors, ladder ID, orientation, evaluation harness. |
| `src/gel_sim` | Synthetic gel simulator with effects + exact ground truth (rayon). |
| `src/gel_camera` | USB camera capture + exposure (mock always; nokhwa behind `--features camera`). |
| `src/gel_cli` | The `gel` binary (headless: analyze, eval, simulate, import-masks, …). |
| `src/gel_app` | The `opengel` binary — the Slint desktop GUI. |

## The `.gel.zip` format

A ZIP container:

```
manifest.json   { format, version, gel_type }
metadata.json   [ per-image capture params: exposure, gain, camera, bracket ]
analysis.json   lanes, bands, blobs, ladder assignments, calibration, quant
images/img_NN.png   raw captures (8- or 16-bit)
```

## Detection & quantification

* **Lanes** — column-sum intensity profile; lanes are the peaks (`src/gel_detect/classical.rs`).
* **Bands** — per-lane vertical densitometry trace, rolling-ball baseline
  subtraction, peak detection + integration (`src/gel_detect/signal.rs`).
* **Ladder ID** — match a lane's band pattern to a commercial template via a
  semi-log fit (`ln(size) ∝ migration`); pick the best-explained lane
  (`src/gel_detect/ladder_match.rs`).
* **Sizing** — semi-log calibration from the ladder sizes every other band.
* **Amounts** — intensity→mass calibration from ladder bands of known mass;
  molarity via `mass / (size × g·mol⁻¹·unit⁻¹)` (650/bp DNA, 340/nt RNA, Da for
  protein) — `src/gel_core/quant.rs`.
* **Cellpose** — plug real bindings into `gel_detect::cellpose::BlobSegmenter`;
  `CellposeDetector` clusters blobs into lanes/bands. Benchmark it against the
  classical detector with the eval harness.

The detector to ship as default is decided by numbers, not assertion: the
**evaluation harness** (`src/gel_detect/eval.rs`) scores any `GelDetector`
against Claude-annotated ground truth (lane IoU, band precision/recall, position
error).

## Build & run

```sh
cargo build                      # lib + gel + opengel bins (mock camera)
cargo test                       # all crates

# CLI
cargo run --bin gel -- make-demo demo.gel.zip
cargo run --bin gel -- analyze demo.gel.zip
cargo run --bin gel -- info demo.gel.zip
cargo run --bin gel -- ladders --gel-type dna
cargo run --bin gel -- make-dataset datasets/demo
cargo run --bin gel -- eval datasets/demo

# Simulator: render degraded gels (rotation, warp, background, overexposure,
# run-out-of-frame, Poisson noise) with exact ground truth, in parallel, then
# benchmark the detector on them.
cargo run --bin gel -- simulate datasets/sim --count 50 --seed 1 --eval
cargo run --bin gel -- simulate datasets/sim --count 50 --upright --eval   # no rotation

# Analyze a loose image (jpg/png/tif), not just a .gel.zip
cargo run --bin gel -- analyze path/to/gel.jpg --out out.gel.zip

# Real annotated gels: convert GelGenie segmentation masks to ground truth,
# then benchmark the detector on them (see datasets/real_gels/).
cargo run --bin gel -- import-masks datasets/real_gels/gelgenie/quantitation_ladder_gels
cargo run --bin gel -- eval        datasets/real_gels/gelgenie/quantitation_ladder_gels/test_images

# GUI (needs a display)
cargo run --bin opengel
cargo run --bin opengel -- demo.gel.zip          # open a file on launch
cargo run --bin opengel --features camera        # enable real USB capture
```

## GUI viewer

The desktop app is an image viewer first: **zoom** (buttons / mouse wheel),
**pan** (drag), **live rotation** (slider + auto-straighten), and a display
**level** control. Lane/band overlays are composited into the image so they
zoom, pan and rotate together with it.

**Demo annotation → measurement.** *Analyze ▸ Demo annotation* drops 4 example
lanes (one ladder, three samples) with band regions; *Measure* then integrates
each region's background-subtracted density straight from the pixels
(densitometry) and fills the Bands table. This region-measurement step is
**independent of the detection algorithm** — regions can come from the demo,
manual editing, or a detector plugged in later behind
`gel_detect::detector::GelDetector`. Automatic lane/band detection is currently
deferred pending detector retuning/replacement.


## Datasets

`datasets/real_gels/` holds ~1,270 openly-licensed real gel images plus 575
hand-labelled band masks (GelGenie, CC BY 4.0; rice CAPS gels, CC BY-SA 2.1 JP;
Wikimedia/PLOS mixed) — image blobs are git-ignored, source/license sidecars and
manifests are tracked. `gel import-masks` turns GelGenie masks into eval ground
truth.

