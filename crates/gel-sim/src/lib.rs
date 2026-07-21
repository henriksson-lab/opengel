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
//! * [`config::SimConfig`] — scene + effect parameters (`clean`, `randomized`).
//! * [`render::simulate`] — render one gel → [`render::RenderedGel`].
//! * [`batch`] — parallel batch rendering and dataset export.

pub mod batch;
pub mod config;
pub mod render;
pub mod rng;

pub use batch::{simulate_batch, simulate_random_batch, write_dataset};
pub use config::SimConfig;
pub use render::{simulate, RenderedGel};
