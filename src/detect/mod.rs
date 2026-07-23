//! # gel-detect
//!
//! Pluggable gel detection engine and evaluation harness.
//!
//! * [`detector`] — the [`GelDetector`](detector::GelDetector) trait + params.
//! * [`signal`] — 1-D smoothing, rolling-ball baseline, peak integration.
//! * [`classical`] — the classical densitometry detector.
//! * [`ladder_match`] — identify which lane is a ladder and its template.
//! * [`pipeline`] — detection → ladder ID → sizing, producing an `Analysis`.
//! * [`eval`] — score detectors against annotated ground truth.

pub mod blob_detector;
pub mod classical;
pub mod detector;
pub mod eval;
pub mod flow;
#[cfg(feature = "gelgenie-ml")]
pub mod gelgenie_ml;
pub mod ladder_match;
pub mod mask_segment;
#[cfg(feature = "gelgenie-ml")]
mod models {
    pub mod gelgenie_unet_1024;
}
pub mod orient;
pub mod pipeline;
pub mod signal;

pub use classical::ClassicalDetector;
pub use detector::{DetectParams, Detection, GelDetector};
#[cfg(feature = "gelgenie-ml")]
pub use gelgenie_ml::{GelGenieDetector, GelGenieRuntime};
pub use mask_segment::{mask_to_boxes, MaskSegmenter};
pub use pipeline::{analyze, analyze_detection};
