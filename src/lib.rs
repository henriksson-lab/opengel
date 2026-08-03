//! # OpenGel
//!
//! Single-crate library for capturing, simulating, detecting and quantifying
//! gel-electrophoresis images. Each module was previously its own crate:
//!
//! * [`core`] — data model, `.gel.zip` IO, HDR merge, ladders, quant math.
//! * [`detect`] — pluggable detectors, ladder ID, orientation, eval harness.
//! * [`camera`] — USB camera capture + exposure (mock + optional nokhwa).
//! * [`instrument`] — imaging enclosures (trays, door, illumination, faults),
//!   which sit *around* a camera rather than being one.
//! * [`sim`] — synthetic gel simulator with effects and ground truth.
//! * [`simbench`] — the one thing the simulated enclosure and the mock camera
//!   share: which light source is currently on the virtual gel.
//!
//! The `gel` and `opengel` binaries live under `src/cli` and `src/gui`.

pub mod camera;
pub mod core;
pub mod detect;
pub mod instrument;
pub mod sim;
pub mod simbench;
