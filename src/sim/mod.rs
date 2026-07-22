//! # gel-sim
//!
//! Synthetic gel image simulator for testing the detection engine. Renders
//! gels with configurable, physically-motivated effects and emits **exact
//! ground truth** (band centers, sizes, ladder identity) so detectors can be
//! benchmarked with the evaluation harness.
//!
//! Effects: uneven background, nonlinear vertical warp (smile/wobble), whole-gel
//! rotation (≤ 50°), overexposure (saturation clipping), run-out-of-frame
//! cropping, and Poisson shot noise. Batches render in parallel via rayon.
//!
//! **"Smile" is a test-only concept.** It lives here so tests can bake a *known*
//! migration bow into synthetic gels and measure how well a fit removes it (see
//! `detect::eval::iso_migration_spread` / `warp_migration_error`). Production
//! code — detection, warp fitting, the GUI — models distortion **only** as the
//! NURBS `GelWarp`; do not reintroduce smile as a modeling concept outside the
//! simulator and its tests.
//!
//! * [`config::SimConfig`] — scene + effect parameters (`clean`, `randomized`).
//! * [`render::simulate`] — render one gel → [`render::RenderedGel`].
//! * [`batch`] — parallel batch rendering and dataset export.

pub mod batch;
pub mod config;
pub mod render;
pub mod rng;

pub use batch::{simulate_batch, simulate_random_batch, simulate_random_batch_with, write_dataset};
pub use config::{GelParams, LaneContent, SimConfig, SimLane, WarpMode};
pub use render::{simulate, RenderedGel};
