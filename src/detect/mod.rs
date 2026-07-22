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

pub mod cellpose;
pub mod classical;
pub mod detector;
pub mod eval;
pub mod flow;
pub mod ladder_match;
pub mod orient;
pub mod pipeline;
pub mod signal;

pub use classical::ClassicalDetector;
pub use detector::{DetectParams, Detection, GelDetector};
pub use pipeline::analyze;
