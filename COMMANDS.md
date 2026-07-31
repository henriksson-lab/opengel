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

# Cameras: which devices each backend sees (nu-manager devices + webcams)
cargo run --release --example list_cameras
```

