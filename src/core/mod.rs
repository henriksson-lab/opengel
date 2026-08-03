//! # gel-core
//!
//! Core, UI- and hardware-free foundations for OpenGel:
//!
//! * [`model`] — the data model ([`GelProject`], lanes, bands, ladders, ...).
//! * [`format`] — reading/writing the `.gel.zip` container.
//! * [`scn`] — reading Bio-Rad Image Lab `.scn`/`.mscn` scans.
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
pub mod scn;
pub mod warp;

pub use format::{CapturedChannel, GelDocument};
pub use imagef32::GrayF32;
pub use model::{
    Analysis, Attribute, Attributes, Band, Blob, Calibration, CaptureMeta, Channel, ChannelColor,
    GelImage, GelProject, GelType, LadderAssignment, LadderBand, LadderTemplate, Lane,
    Quantification, TargetKind, FORMAT_VERSION,
};
pub use warp::GelWarp;
