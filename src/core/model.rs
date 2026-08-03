//! Core data model for an OpenGel project.
//!
//! Everything here is `serde`-serializable. A [`GelProject`] is what a
//! `.gel.zip` deserializes into (minus the raw image pixels, which live in the
//! ZIP as PNGs and are referenced by [`GelImage::filename`]).

use serde::{Deserialize, Serialize};

use crate::core::warp::GelWarp;

/// The kind of gel, which determines molarity conversion and which ladder
/// templates are applicable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GelType {
    /// Double-stranded DNA. Size unit: base pairs (bp).
    Dna,
    /// RNA. Size unit: nucleotides (nt).
    Rna,
    /// Protein (SDS-PAGE). Size unit: daltons (Da).
    Protein,
}

impl GelType {
    /// Average molar mass used to convert a size into g/mol.
    ///
    /// * DNA:  ~650 g/mol per base pair.
    /// * RNA:  ~340 g/mol per nucleotide.
    /// * Protein: size *is* the molar mass (Da == g/mol), so the per-unit
    ///   factor is 1.0.
    pub fn g_per_mol_per_size_unit(self) -> f64 {
        match self {
            GelType::Dna => 650.0,
            GelType::Rna => 340.0,
            GelType::Protein => 1.0,
        }
    }

    /// Human-readable size unit label.
    pub fn size_unit(self) -> &'static str {
        match self {
            GelType::Dna => "bp",
            GelType::Rna => "nt",
            GelType::Protein => "Da",
        }
    }
}

/// One name/value pair of acquisition metadata, exactly as the source recorded
/// it.
///
/// Deliberately untyped. Instruments disagree about what they report — a Gel
/// Doc EZ writes `Illumination Mode`, a ChemiDoc MP writes `Excitation Source`
/// and `Emission Filter`, a ChemiDoc XRS+ adds `Binning` — and a fixed schema
/// would silently drop whatever it did not anticipate. We do not interpret
/// these; we carry them, and show them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attribute {
    /// Human-readable name, e.g. `"Exposure Time (sec)"`.
    pub name: String,
    pub value: String,
}

impl Attribute {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Attribute {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// An acquisition record: attributes in the order the source listed them, which
/// is the order a user reading the instrument's own report expects.
pub type Attributes = Vec<Attribute>;

/// The colour a channel is drawn in when channels are composited.
///
/// Bio-Rad's `<colormap>` element, and ours. Anything unrecognized reads as
/// [`ChannelColor::Gray`] rather than failing the load — a colour is a display
/// preference, and losing one is not worth losing the pixels over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelColor {
    #[default]
    Gray,
    Red,
    Green,
    Blue,
    Cyan,
    Magenta,
    Yellow,
}

impl ChannelColor {
    /// Parse a `<colormap>` string. Unknown names fall back to `Gray`.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "red" => ChannelColor::Red,
            "green" => ChannelColor::Green,
            "blue" => ChannelColor::Blue,
            "cyan" => ChannelColor::Cyan,
            "magenta" => ChannelColor::Magenta,
            "yellow" => ChannelColor::Yellow,
            _ => ChannelColor::Gray,
        }
    }

    /// Linear RGB weights used to tint this channel when compositing.
    pub fn rgb(self) -> (f32, f32, f32) {
        match self {
            ChannelColor::Gray => (1.0, 1.0, 1.0),
            ChannelColor::Red => (1.0, 0.0, 0.0),
            ChannelColor::Green => (0.0, 1.0, 0.0),
            ChannelColor::Blue => (0.0, 0.0, 1.0),
            ChannelColor::Cyan => (0.0, 1.0, 1.0),
            ChannelColor::Magenta => (1.0, 0.0, 1.0),
            ChannelColor::Yellow => (1.0, 1.0, 0.0),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ChannelColor::Gray => "Gray",
            ChannelColor::Red => "Red",
            ChannelColor::Green => "Green",
            ChannelColor::Blue => "Blue",
            ChannelColor::Cyan => "Cyan",
            ChannelColor::Magenta => "Magenta",
            ChannelColor::Yellow => "Yellow",
        }
    }
}

/// One acquisition channel: the same gel, imaged under one illumination.
///
/// Channels share the document's geometry — one warp, one set of lanes, one set
/// of band positions — because they are the same physical gel photographed
/// several times. What differs per channel is intensity, which is why
/// [`Band::channel_density`] is a vector while the band's position is not.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: u32,
    /// Display name — the source's application ("Stain Free Blot") when it has
    /// one, otherwise "Channel N".
    pub name: String,
    #[serde(default)]
    pub color: ChannelColor,
}

impl Channel {
    pub fn new(id: u32, name: impl Into<String>, color: ChannelColor) -> Self {
        Channel {
            id,
            name: name.into(),
            color,
        }
    }
}

/// Per-image capture metadata (stored in `metadata.json`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CaptureMeta {
    /// Exposure time in seconds. Used for HDR radiance scaling.
    pub exposure_seconds: f64,
    /// Sensor gain (ISO-like), if known. Informational.
    #[serde(default)]
    pub gain: Option<f64>,
    /// Camera device name/identifier, if known.
    #[serde(default)]
    pub camera_name: Option<String>,
    /// ISO-8601 capture timestamp, if known.
    #[serde(default)]
    pub timestamp: Option<String>,
    /// Bracket group id: images sharing a value form one HDR exposure bracket.
    #[serde(default)]
    pub bracket_group: Option<u32>,
    /// The instrument's own acquisition record for this frame, carried verbatim.
    /// Populated when importing a vendor file; empty for our own captures until
    /// something fills it in.
    #[serde(default)]
    pub acquisition: Attributes,
}

/// A single captured image, referenced by filename inside the ZIP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GelImage {
    /// Stable id (index-like), unique within a project.
    pub id: u32,
    /// Path inside the ZIP, e.g. `images/img_00.png`.
    pub filename: String,
    pub width: u32,
    pub height: u32,
    /// True if the PNG is 16-bit (higher dynamic range per frame).
    #[serde(default)]
    pub sixteen_bit: bool,
    /// Which channel this frame belongs to, by [`Channel::id`].
    pub channel: u32,
    pub meta: CaptureMeta,
}

/// A lane, expressed in the gel's rectified coordinate space: a `u`-interval
/// (cross-lane axis). Its image footprint is the curved strip
/// `warp.eval([u_min, u_max] × [0, 1])`. Under the identity warp this is just a
/// vertical pixel column `[u_min·W, u_max·W]`, recovering the naive rectangle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lane {
    pub id: u32,
    /// Left/right bounds along the cross-lane axis `u`, in `[0, 1]`.
    pub u_min: f64,
    pub u_max: f64,
    #[serde(default)]
    pub label: Option<String>,
    /// True when this lane has been identified/assigned as a ladder.
    #[serde(default)]
    pub is_ladder: bool,
}

impl Lane {
    /// Cross-lane center `u`.
    pub fn u_center(&self) -> f64 {
        0.5 * (self.u_min + self.u_max)
    }

    /// Pixel x-bounds of the lane strip (evaluated at the top of the gel),
    /// via `warp`. Use for densitometry columns and overlay drawing.
    pub fn px_x_bounds(&self, warp: &GelWarp) -> (usize, usize) {
        let x0 = warp.eval(self.u_min, 0.0).0;
        let x1 = warp.eval(self.u_max, 0.0).0;
        (x0.min(x1).max(0.0) as usize, x0.max(x1).max(0.0) as usize)
    }

    /// Pixel x of the lane center (at the top of the gel).
    pub fn px_x_center(&self, warp: &GelWarp) -> f64 {
        warp.eval(self.u_center(), 0.0).0
    }
}

/// A band within a lane, at an iso-migration coordinate `v_center` (the
/// rectified migration axis). Its image footprint is the smile curve
/// `warp.eval(u, v_center)` over the lane's `u`-range. Because `v` *is* the
/// rectified migration coordinate, the retention factor **Rf ≡ v_center**.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Band {
    pub id: u32,
    pub lane_id: u32,
    /// Migration coordinate of the band peak, in `[0, 1]` (= Rf).
    pub v_center: f64,
    /// Peak half-extent along `v` (band spread in migration).
    pub v_half_width: f64,
    /// Background-subtracted integrated density (area under the peak), measured
    /// on the document's working image.
    pub integrated_density: f64,
    /// The same measurement taken separately in each channel, indexed by
    /// [`Channel::id`]. Empty until a per-channel measurement has been run.
    ///
    /// The band's *position* is deliberately not per-channel: channels are the
    /// same gel under different light, so they share geometry and differ only
    /// in how much signal each one sees.
    #[serde(default)]
    pub channel_density: Vec<f64>,
    /// Estimated size (bp/nt/Da) from ladder calibration.
    #[serde(default)]
    pub size: Option<f64>,
    /// Known size when this band belongs to a ladder lane.
    #[serde(default)]
    pub known_size: Option<f64>,
    /// Local band tilt (radians) measured from intensity moments in the raw
    /// image: the angle of the band's long axis from horizontal. Drives the
    /// rotated annotation box and constrains the gel warp. 0 = horizontal.
    #[serde(default)]
    pub angle: f64,
    /// Extra ladder rung sizes this band carries because two or more rungs were
    /// too close to resolve and merged into one blob (in addition to `known_size`).
    /// Empty for ordinary single-rung bands. Displayed as "N + M bp".
    #[serde(default)]
    pub merged_sizes: Vec<f64>,
}

impl Band {
    /// Retention factor — identical to `v_center` by construction, kept as a
    /// named accessor for call sites that speak in Rf.
    pub fn rf(&self) -> f64 {
        self.v_center
    }

    /// Pixel y of the band peak along the lane at cross-lane position `u`.
    pub fn px_y_center(&self, warp: &GelWarp, u: f64) -> f64 {
        warp.eval(u, self.v_center).1
    }

    /// Pixel half-extent of the band along migration at `u`.
    pub fn px_y_half(&self, warp: &GelWarp, u: f64) -> f64 {
        let y0 = warp.eval(u, (self.v_center - self.v_half_width).max(0.0)).1;
        let y1 = warp.eval(u, (self.v_center + self.v_half_width).min(1.0)).1;
        ((y1 - y0).abs() / 2.0).max(0.5)
    }
}

/// A free-form region (spot/blob) not necessarily tied to a lane. Stored as an
/// axis-aligned bounding box plus its integrated density; a polygon mask may be
/// added later.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Blob {
    pub id: u32,
    pub x_min: u32,
    pub y_min: u32,
    pub x_max: u32,
    pub y_max: u32,
    pub integrated_density: f64,
    #[serde(default)]
    pub label: Option<String>,
}

/// One rung of a commercial ladder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LadderBand {
    /// Size in the gel-type's unit (bp/nt/Da).
    pub size: f64,
    /// Mass in ng of this band for the manufacturer's standard load, if known.
    #[serde(default)]
    pub mass_ng: Option<f64>,
    /// True for "reference" bands the manufacturer loads at increased intensity
    /// (extra-thick / brighter bands used for quick orientation).
    #[serde(default)]
    pub reference: bool,
}

/// A commercial (or custom) ladder definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LadderTemplate {
    pub name: String,
    pub gel_type: GelType,
    /// Manufacturer, e.g. "NEB", "Thermo Fisher", "Bio-Rad", "Promega".
    #[serde(default)]
    pub vendor: Option<String>,
    /// Catalog number(s), e.g. "N3232".
    #[serde(default)]
    pub catalog: Option<String>,
    /// Total loaded mass (ng) the `mass_ng` values correspond to, if specified.
    #[serde(default)]
    pub standard_load_ng: Option<f64>,
    /// Bands, conventionally ordered from largest to smallest size.
    pub bands: Vec<LadderBand>,
}

impl LadderTemplate {
    /// Sizes of the reference (extra-thick) bands.
    pub fn reference_sizes(&self) -> Vec<f64> {
        self.bands
            .iter()
            .filter(|b| b.reference)
            .map(|b| b.size)
            .collect()
    }
}

/// Assignment of a ladder template to a lane, mapping detected bands to rungs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LadderAssignment {
    pub lane_id: u32,
    pub template_name: String,
    /// `map[i] = Some(band_id)` links ladder rung `i` to a detected band.
    #[serde(default)]
    pub rung_to_band: Vec<Option<u32>>,
}

/// Intensity → mass calibration model fitted from ladder bands of known mass.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Calibration {
    /// mass = slope * density  (forced through origin).
    Linear { slope: f64 },
    /// mass = a * density + b   (affine).
    Affine { a: f64, b: f64 },
    /// log(mass) = a * log(density) + b  (power law; handles saturation).
    LogLog { a: f64, b: f64 },
}

impl Calibration {
    /// Predict mass (ng) from an integrated density.
    pub fn mass_ng(&self, density: f64) -> f64 {
        match *self {
            Calibration::Linear { slope } => slope * density,
            Calibration::Affine { a, b } => a * density + b,
            Calibration::LogLog { a, b } => (a * density.max(1e-12).ln() + b).exp(),
        }
    }
}

/// Quantification result for one band or blob.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quantification {
    /// Band id, or blob id, this result refers to.
    pub target_id: u32,
    /// Whether `target_id` refers to a band or a blob.
    pub target_kind: TargetKind,
    pub mass_ng: Option<f64>,
    pub molarity_nmol: Option<f64>,
    pub size: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Band,
    Blob,
}

/// The full analysis state (serialized as `analysis.json`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Analysis {
    /// Fitted gel warp surface mapping rectified `(u, v)` → image pixels. `None`
    /// means the identity warp (naive rectangle); lanes/bands then map linearly
    /// to pixels via the image dimensions.
    #[serde(default)]
    pub warp: Option<GelWarp>,
    #[serde(default)]
    pub lanes: Vec<Lane>,
    #[serde(default)]
    pub bands: Vec<Band>,
    #[serde(default)]
    pub blobs: Vec<Blob>,
    #[serde(default)]
    pub ladder_assignments: Vec<LadderAssignment>,
    #[serde(default)]
    pub calibration: Option<Calibration>,
    #[serde(default)]
    pub quantifications: Vec<Quantification>,
}

impl Analysis {
    /// The effective warp for an image of the given size: the fitted warp if
    /// present, else the identity warp (naive rectangle).
    pub fn warp_or_identity(&self, width: u32, height: u32) -> GelWarp {
        self.warp
            .clone()
            .unwrap_or_else(|| GelWarp::identity(width, height))
    }
}

/// Current on-disk format version.
pub const FORMAT_VERSION: u32 = 1;

/// A full project: everything in a `.gel.zip` except raw pixels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GelProject {
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default = "default_version")]
    pub version: u32,
    pub gel_type: GelType,
    #[serde(default)]
    pub images: Vec<GelImage>,
    /// Acquisition channels — always at least one, so no call site has to
    /// branch on whether a document is multichannel just to read its pixels.
    pub channels: Vec<Channel>,
    /// Document-level metadata from the source file — its name, the operator,
    /// a description. Carried verbatim, like [`CaptureMeta::acquisition`].
    #[serde(default)]
    pub metadata: Attributes,
    /// Display the image inverted by default: the source said a zero sample
    /// should render *white*, which is how blots are conventionally shown. It
    /// describes presentation only — the pixels are stored as they were read,
    /// with high values meaning more signal either way.
    #[serde(default)]
    pub display_inverted: bool,
    #[serde(default)]
    pub analysis: Analysis,
    /// Record of a saved HDR merge (`images/merged.png`), if one was computed and
    /// persisted — the options used and the radiance scale to reconstruct it.
    #[serde(default)]
    pub hdr: Option<HdrRecord>,
}

/// Metadata for a persisted HDR merge. The merged radiance image is stored as a
/// 16-bit `images/merged.png` normalized by `scale`; radiance = `png / 65535 *
/// scale`. `options` records which optional stages produced it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct HdrRecord {
    pub options: crate::core::hdr::HdrOptions,
    pub scale: f64,
}

fn default_format() -> String {
    "opengel".to_string()
}
fn default_version() -> u32 {
    FORMAT_VERSION
}

impl GelProject {
    /// Create an empty project of the given gel type.
    pub fn new(gel_type: GelType) -> Self {
        GelProject {
            format: default_format(),
            version: FORMAT_VERSION,
            gel_type,
            images: Vec::new(),
            channels: vec![Channel::new(0, "Channel 1", ChannelColor::Gray)],
            metadata: Attributes::new(),
            display_inverted: false,
            analysis: Analysis::default(),
            hdr: None,
        }
    }

    /// True when the document holds more than one acquisition channel.
    pub fn is_multichannel(&self) -> bool {
        self.channels.len() > 1
    }

    /// Indices into [`GelProject::images`] belonging to `channel`.
    pub fn image_indices_for_channel(&self, channel: u32) -> Vec<usize> {
        self.images
            .iter()
            .enumerate()
            .filter(|(_, img)| img.channel == channel)
            .map(|(i, _)| i)
            .collect()
    }
}
