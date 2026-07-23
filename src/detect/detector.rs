//! The pluggable detector interface. Every detection method (classical,
//! blob-based, hybrid) implements [`GelDetector`] so they can be swapped and
//! benchmarked by the evaluation harness against the same ground truth.

use crate::core::model::{Band, Lane};
use crate::core::warp::GelWarp;
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
    /// Fit the gel warp by **optical flow** (coarse column registration of the
    /// band twist) instead of only from ladder rungs. Captures distortion
    /// between lanes where no ladder constrains it.
    pub optical_flow_warp: bool,
    /// Energy-vs-flow trade-off for the optical-flow warp: the smoothness weight
    /// balancing measured flow against a smooth displacement field. Larger =
    /// stiffer (more interpolation between confident strips).
    pub flow_smoothness: f64,
    /// Extra migration-axis (vertical) control rows beyond matched ladder/front
    /// rows. More rows let the warp bend more up/down between bands. The fit
    /// floors this internally so there is always top/bottom support.
    pub extra_vertical_edges: usize,
    /// Extra cross-lane (horizontal) control columns beyond the gel edges and the
    /// one column placed at each lane center. More columns let the warp bend more
    /// side-to-side between lanes. 0 = one column per lane only.
    pub extra_horizontal_edges: usize,
    /// Regularization pull for NURBS refinement toward the prior grid.
    pub warp_regularization: f64,
    /// Multiplier for preserving adjacent `v` control-row spacing.
    pub row_spacing_weight: f64,
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
            optical_flow_warp: false,
            flow_smoothness: 8.0,
            extra_vertical_edges: 2,
            extra_horizontal_edges: 0,
            warp_regularization: 1e-2,
            row_spacing_weight: 10.0,
        }
    }
}

/// A detected lane in **raw image pixel space** (detection runs before any warp
/// exists — see [`crate::core::warp`]).
#[derive(Debug, Clone)]
pub struct DetLane {
    pub id: u32,
    /// Left/right pixel bounds (inclusive left, exclusive right).
    pub x_min: u32,
    pub x_max: u32,
    /// Top/bottom pixel bounds of the lane's active region.
    pub y_min: u32,
    pub y_max: u32,
    pub is_ladder: bool,
}

impl DetLane {
    /// Cross-lane center x (pixels).
    pub fn x_center(&self) -> f64 {
        (self.x_min as f64 + self.x_max as f64) / 2.0
    }
}

/// A detected band in **raw image pixel space**.
#[derive(Debug, Clone)]
pub struct DetBand {
    pub id: u32,
    pub lane_id: u32,
    /// Cross-lane pixel position of the band centroid (feeds warp fitting).
    pub x_center: f64,
    /// Migration-axis pixel position of the band peak.
    pub y_center: f64,
    /// Peak half-extent in pixels.
    pub y_half_width: f64,
    /// Background-subtracted integrated density.
    pub integrated_density: f64,
    /// Local band tilt (radians) from intensity moments — the angle of the
    /// band's long axis from horizontal in the raw image. 0 = horizontal.
    pub angle: f64,
}

/// Output of a detection pass, in raw pixel coordinates.
#[derive(Debug, Clone, Default)]
pub struct Detection {
    pub lanes: Vec<DetLane>,
    pub bands: Vec<DetBand>,
}

impl Detection {
    /// Fit a gel warp from the detected lanes (their centerlines pin the `u`
    /// axis). Detection has already happened, so this closes the loop without a
    /// circular dependency: pixels → warp, never warp → pixels.
    pub fn fit_warp(&self, width: u32, height: u32) -> GelWarp {
        let centers: Vec<f64> = self.lanes.iter().map(|l| l.x_center()).collect();
        GelWarp::fit_grid(&centers, width, height)
    }

    /// Lift the pixel-space detection into rectified `(u, v)` model types using
    /// `warp`, producing the `(lanes, bands)` for an [`crate::core::model::Analysis`].
    pub fn to_model(&self, warp: &GelWarp) -> (Vec<Lane>, Vec<Band>) {
        let lanes = self
            .lanes
            .iter()
            .map(|l| {
                let (u_min, _) = warp.invert(l.x_min as f64, l.y_min as f64);
                let (u_max, _) = warp.invert(l.x_max as f64, l.y_min as f64);
                Lane {
                    id: l.id,
                    u_min: u_min.min(u_max),
                    u_max: u_min.max(u_max),
                    label: None,
                    is_ladder: l.is_ladder,
                }
            })
            .collect();
        let bands = self
            .bands
            .iter()
            .map(|b| {
                let (_, v_center) = warp.invert(b.x_center, b.y_center);
                // Half-width in v: convert the pixel extent via the local scale.
                let (_, v_lo) = warp.invert(b.x_center, b.y_center - b.y_half_width);
                let (_, v_hi) = warp.invert(b.x_center, b.y_center + b.y_half_width);
                Band {
                    id: b.id,
                    lane_id: b.lane_id,
                    v_center,
                    v_half_width: ((v_hi - v_lo).abs() / 2.0).max(1e-4),
                    integrated_density: b.integrated_density,
                    size: None,
                    known_size: None,
                    angle: b.angle,
                    merged_sizes: Vec::new(),
                }
            })
            .collect();
        (lanes, bands)
    }
}

/// A detection method.
pub trait GelDetector {
    /// Short identifier used in evaluation reports.
    fn name(&self) -> &str;
    /// Detect lanes and bands in a working image (higher = more signal after
    /// the detector applies any inversion per `params.signal_is_bright`).
    fn detect(&self, img: &GrayF32, params: &DetectParams) -> Detection;
}
