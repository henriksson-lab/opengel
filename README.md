# OpenGel

Capture, detect and quantify gel electrophoresis images

**under development! feature requests can go into github issues**

Features:

* Supports **DNA, RNA and protein** gels.
* Camera snapshots, including multi-exposure **HDR** for dynamic range
* Detection of bands using ML, and gaussian mixture distribution to figure out if the are at an angle. The gel is modelled by [NURBS](https://en.wikipedia.org/wiki/Non-uniform_rational_B-spline), fitting band angles and ladder band positions*
* Quantification of densitys and molarities, taking gel warping into account
* Quick compute of relative mass and molarity ratios

![OpenGel GUI: a detected gel with per-band annotation boxes and the fitted NURBS warp grid overlaid](assets/screenshot.png)

![OpenGel Trace view: per-lane densitometry profiles with a migration-px bottom axis and a ladder-calibrated size (bp) top axis](assets/trace.png)

## The `.gel.zip` format

A ZIP container:

```
manifest.json   { format, version, gel_type }
metadata.json   [ per-image capture params: exposure, gain, camera, bracket ]
analysis.json   lanes, bands, blobs, ladder assignments, calibration, quant
images/img_NN.png   raw captures (8- or 16-bit)
```

## Build & run

```sh
cargo build --release             # optimized build (USB camera backend on by default)

# GUI (the default binary)
cargo run --release

# CLI
cargo run --release --bin gel
```

The crate ships two binaries — `opengel` (the desktop GUI) and `gel` (the CLI).
`default-run` points bare `cargo run` at the GUI; pass `--bin gel` for the CLI.
The USB camera backend is on by default on every platform. On Linux it needs
`libv4l-dev` at build time (`sudo apt-get install libv4l-dev`); build with
`--no-default-features` to drop it for a headless build without the v4l toolchain.


## Citing

The initial detection of bands is done using the pretrained ML model of [GelGenie](https://github.com/mattaq31/GelGenie),
converted to Rust and adapted for the NURBS model. It is an important component and it would thus be
fair you could cite:

> Aquilina, M., Wu, N. J. W., Kwan, K., Bušić, F., Dodd, J., Nicolás-Sáenz, L.,
> O'Callaghan, A., Bankhead, P., & Dunn, K. E. (2025). GelGenie: an AI-powered
> framework for gel electrophoresis image analysis. *Nature Communications*, 16,
> 4087. https://doi.org/10.1038/s41467-025-59189-0

In addition, please cite this git repository. So you could write something like:
*Gels were analyzed using https://github.com/henriksson-lab/opengel, using GelGenie[1] for band detection*


## License

* Code is by default under MIT license
* Code under src/gelgenie is a Rust conversion of [GelGenie](https://github.com/mattaq31/GelGenie), which is under Apache-2.0 license.

Note that code has been produced using agentic AI; in case you
wish to copy out any part of the code, please first please review
the code for accidental reuse of copyrighted material.
