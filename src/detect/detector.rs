//! The pluggable detector interface. Every detection method (classical,
//! Cellpose, hybrid) implements [`GelDetector`] so they can be swapped and
//! benchmarked by the evaluation harness against the same ground truth.

use crate::core::model::{Band, Lane};
use crate::core::GrayF32;

/// Tunable parameters shared by detectors (classical uses all of them; others
/// use the relevant subset).
#[derive(Debug, Clone)]
pub struct DetectParams {
    /// True when higher pixel value means more sample (e.g. UV fluorescence).
    /// False for dark-bands-on-light scans, which are inverted internally.
    pub signal_is_bright: bool,
    /// Smoothing radius for the column (lane-finding) profile.
    pub column_smooth: usize,
    /// Lane peak prominence as a fraction of the max column profile value.
    pub lane_min_rel_prominence: f64,
    /// Minimum spacing (px) between lane centers.
    pub min_lane_distance: usize,
    /// If set, keep exactly this many lanes (the most prominent).
    pub expected_lanes: Option<usize>,
    /// Rolling-ball baseline radius for per-lane band traces.
    pub baseline_radius: usize,
    /// Smoothing radius for per-lane band traces.
    pub row_smooth: usize,
    /// Band peak prominence as a fraction of the per-lane max trace value.
    pub band_min_rel_prominence: f64,
    /// Minimum spacing (px) between band centers within a lane.
    pub min_band_distance: usize,
}

impl Default for DetectParams {
    fn default() -> Self {
        DetectParams {
            signal_is_bright: true,
            column_smooth: 3,
            lane_min_rel_prominence: 0.15,
            min_lane_distance: 8,
            expected_lanes: None,
            baseline_radius: 25,
            row_smooth: 2,
            band_min_rel_prominence: 0.05,
            min_band_distance: 4,
        }
    }
}

/// Output of a detection pass.
#[derive(Debug, Clone, Default)]
pub struct Detection {
    pub lanes: Vec<Lane>,
    pub bands: Vec<Band>,
}

/// A detection method.
pub trait GelDetector {
    /// Short identifier used in evaluation reports.
    fn name(&self) -> &str;
    /// Detect lanes and bands in a working image (higher = more signal after
    /// the detector applies any inversion per `params.signal_is_bright`).
    fn detect(&self, img: &GrayF32, params: &DetectParams) -> Detection;
}
