//! # gel-core
//!
//! Core, UI- and hardware-free foundations for OpenGel:
//!
//! * [`model`] — the data model ([`GelProject`], lanes, bands, ladders, ...).
//! * [`format`] — reading/writing the `.gel.zip` container.
//! * [`imagef32`] — the `f32` working image representation.
//! * [`hdr`] — merging an exposure bracket into a radiance image.
//! * [`ladders`] — the built-in commercial ladder database.
//! * [`quant`] — sizing, calibration and molarity math.

pub mod demo;
pub mod format;
pub mod hdr;
pub mod imagef32;
pub mod ladders;
pub mod model;
pub mod quant;

pub use format::GelDocument;
pub use imagef32::GrayF32;
pub use model::{
    Analysis, Band, Blob, Calibration, CaptureMeta, GelImage, GelProject, GelType, Lane,
    LadderAssignment, LadderBand, LadderTemplate, Quantification, TargetKind, FORMAT_VERSION,
};
