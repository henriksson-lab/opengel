//! Reading and writing the `.gel.zip` container.
//!
//! Layout inside the ZIP:
//! ```text
//! manifest.json     { format, version, gel_type }
//! metadata.json     [ GelImage, ... ]   (capture params per image)
//! analysis.json     Analysis            (lanes/bands/blobs/calibration/...)
//! images/img_00.png raw captures (8- or 16-bit)
//! ```
//!
//! A [`GelDocument`] bundles the deserialized [`GelProject`] with the decoded
//! pixel frames (parallel to `project.images`).

use std::io::{Cursor, Read, Write};
use std::path::Path;

use image::{DynamicImage, ImageFormat};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;

use crate::core::model::{
    Analysis, Attributes, CaptureMeta, Channel, ChannelColor, GelImage, GelProject, GelType,
    HdrRecord, FORMAT_VERSION,
};
use crate::core::GrayF32;

/// One channel's worth of a fresh acquisition, on its way into a document.
pub struct CapturedChannel {
    /// What the channel is called in the document — the light source it was
    /// taken under.
    pub name: String,
    /// Display colour, so a multi-channel gel can be told apart at a glance.
    pub color: ChannelColor,
    /// The frames: one for a single capture, a bracket for HDR.
    pub frames: Vec<(DynamicImage, CaptureMeta)>,
}

#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("image error: {0}")]
    Image(#[from] image::ImageError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("missing required entry `{0}` in .gel.zip")]
    MissingEntry(&'static str),
    #[error("frame count ({frames}) does not match image count ({images})")]
    FrameMismatch { frames: usize, images: usize },
}

pub type Result<T> = std::result::Result<T, FormatError>;

/// The `manifest.json` payload.
#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    format: String,
    version: u32,
    gel_type: GelType,
    /// Present when a merged HDR image (`images/merged.png`) is stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hdr: Option<HdrRecord>,
    /// Acquisition channels — one entry for an ordinary single-channel gel.
    channels: Vec<Channel>,
    /// Document-level metadata carried over from the source file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    metadata: Attributes,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    display_inverted: bool,
}

/// Filename of the persisted HDR merge inside the zip.
const MERGED_FILENAME: &str = "images/merged.png";

/// A project plus its decoded image frames.
pub struct GelDocument {
    pub project: GelProject,
    /// Decoded frames, parallel to `project.images`.
    pub frames: Vec<DynamicImage>,
    /// The saved HDR merge (linear radiance), when `project.hdr` is set. Used
    /// directly as the working image instead of re-merging the bracket on load.
    pub merged: Option<GrayF32>,
}

impl GelDocument {
    /// Build a document, generating `GelImage` records from the frames.
    ///
    /// Each frame becomes `images/img_NN.png`. Provide `metas` parallel to
    /// `frames` for capture parameters (or an empty slice for defaults).
    pub fn from_frames(
        gel_type: GelType,
        frames: Vec<DynamicImage>,
        metas: Vec<crate::core::model::CaptureMeta>,
    ) -> Self {
        let mut project = GelProject::new(gel_type);
        for (i, frame) in frames.iter().enumerate() {
            let meta = metas.get(i).cloned().unwrap_or_default();
            let sixteen_bit = matches!(
                frame,
                DynamicImage::ImageLuma16(_)
                    | DynamicImage::ImageRgb16(_)
                    | DynamicImage::ImageLumaA16(_)
                    | DynamicImage::ImageRgba16(_)
            );
            project.images.push(GelImage {
                id: i as u32,
                filename: format!("images/img_{i:02}.png"),
                width: frame.width(),
                height: frame.height(),
                sixteen_bit,
                channel: 0,
                meta,
            });
        }
        GelDocument {
            project,
            frames,
            merged: None,
        }
    }

    /// Import a Bio-Rad Image Lab scan as a document.
    ///
    /// Each channel of the scan becomes one channel here, holding one frame.
    /// The instrument's acquisition record rides along on each frame's
    /// [`CaptureMeta::acquisition`][crate::core::CaptureMeta::acquisition] —
    /// carried, not interpreted, because instruments disagree about what they
    /// report and dropping the unfamiliar parts would lose exactly the details
    /// that make an image reproducible.
    pub fn from_scn(scn: &crate::core::scn::ScnFile) -> Self {
        use crate::core::model::{Attribute, CaptureMeta};

        let mut project = GelProject::new(scn.gel_type());
        project.metadata = scn.metadata.clone();
        project.display_inverted = scn.display_inverted;
        project.channels.clear();

        let mut frames = Vec::with_capacity(scn.channels.len());
        for (i, channel) in scn.channels.iter().enumerate() {
            project
                .channels
                .push(Channel::new(i as u32, &channel.name, channel.color));

            // The bit depth the file arrived at is part of how it was taken, and
            // is not otherwise recoverable once the samples are rescaled.
            let mut acquisition = channel.acquisition.clone();
            if channel.max_value != u16::MAX as u32 {
                acquisition.push(Attribute::new(
                    "Source Bit Ceiling",
                    channel.max_value.to_string(),
                ));
            }
            if let Some((w, h)) = channel.original_size {
                acquisition.push(Attribute::new("Native Readout", format!("{w} × {h} px")));
            }
            if let Some((w, h)) = channel.size_mm {
                acquisition.push(Attribute::new("Imaged Area", format!("{w} × {h} mm")));
            }

            project.images.push(GelImage {
                id: i as u32,
                filename: format!("images/img_{i:02}.png"),
                width: channel.width,
                height: channel.height,
                sixteen_bit: true,
                channel: i as u32,
                meta: CaptureMeta {
                    exposure_seconds: channel.exposure_seconds,
                    gain: None,
                    camera_name: channel.imager.clone(),
                    timestamp: channel.timestamp.clone(),
                    // Channels are separate acquisitions, not an exposure
                    // bracket of one scene: merging them would average away the
                    // very differences they were taken to record.
                    bracket_group: None,
                    acquisition,
                },
            });
            frames.push(channel.image.clone());
        }

        GelDocument {
            project,
            frames,
            merged: None,
        }
    }

    /// Build a document from a multi-channel acquisition: one entry per channel,
    /// each holding that channel's frames (one for a single capture, several for
    /// an HDR bracket).
    ///
    /// Frames stay grouped by channel and are never mixed: a bracket *within* a
    /// channel HDR-merges, but two channels are separate acquisitions of the
    /// same gel, and merging them would average away the differences they were
    /// taken to record.
    pub fn from_channels(gel_type: GelType, channels: Vec<CapturedChannel>) -> Self {
        let mut project = GelProject::new(gel_type);
        project.channels.clear();
        let mut frames = Vec::new();

        for (channel_id, channel) in channels.into_iter().enumerate() {
            let channel_id = channel_id as u32;
            project
                .channels
                .push(Channel::new(channel_id, &channel.name, channel.color));
            for (frame, meta) in channel.frames {
                let sixteen_bit = matches!(
                    frame,
                    DynamicImage::ImageLuma16(_)
                        | DynamicImage::ImageRgb16(_)
                        | DynamicImage::ImageLumaA16(_)
                        | DynamicImage::ImageRgba16(_)
                );
                let id = frames.len() as u32;
                project.images.push(GelImage {
                    id,
                    filename: format!("images/img_{id:02}.png"),
                    width: frame.width(),
                    height: frame.height(),
                    sixteen_bit,
                    channel: channel_id,
                    meta: CaptureMeta { ..meta },
                });
                frames.push(frame);
            }
        }

        // A capture that produced nothing would leave a channel-less project,
        // which no view can render; keep the default single channel instead.
        if project.channels.is_empty() {
            project.channels.push(Channel::new(0, "Channel 1", ChannelColor::Gray));
        }

        GelDocument {
            project,
            frames,
            merged: None,
        }
    }

    /// Read a `.scn`/`.mscn` file straight into a document.
    pub fn load_scn(path: impl AsRef<Path>) -> std::result::Result<Self, crate::core::scn::ScnError> {
        Ok(Self::from_scn(&crate::core::scn::ScnFile::load(path)?))
    }

    /// Serialize to a `.gel.zip` file at `path`.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        if self.frames.len() != self.project.images.len() {
            return Err(FormatError::FrameMismatch {
                frames: self.frames.len(),
                images: self.project.images.len(),
            });
        }
        let bytes = self.to_bytes()?;
        std::fs::write(path, bytes)?;
        Ok(())
    }

    /// Serialize the whole document into an in-memory ZIP byte buffer.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

            let manifest = Manifest {
                format: "opengel".to_string(),
                version: FORMAT_VERSION,
                gel_type: self.project.gel_type,
                hdr: self.project.hdr,
                channels: self.project.channels.clone(),
                metadata: self.project.metadata.clone(),
                display_inverted: self.project.display_inverted,
            };
            write_json(&mut zip, "manifest.json", &manifest, opts)?;
            write_json(&mut zip, "metadata.json", &self.project.images, opts)?;
            write_json(&mut zip, "analysis.json", &self.project.analysis, opts)?;

            // Images are already compressed (PNG), so store without deflate.
            let store =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            for (img, frame) in self.project.images.iter().zip(&self.frames) {
                zip.start_file(&img.filename, store)?;
                let mut png = Vec::new();
                frame.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)?;
                zip.write_all(&png)?;
            }
            // Persist the HDR merge (16-bit, normalized by the record's scale).
            if let (Some(rec), Some(merged)) = (self.project.hdr, &self.merged) {
                zip.start_file(MERGED_FILENAME, store)?;
                let png = encode_merged_png(merged, rec.scale)?;
                zip.write_all(&png)?;
            }
            zip.finish()?;
        }
        Ok(buf)
    }

    /// Load a `.gel.zip` file from `path`.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        Self::from_bytes(&bytes)
    }

    /// Derive the single working grayscale image for analysis/display.
    ///
    /// If the images form a bracket group (shared `bracket_group`, >1 frame,
    /// positive exposures) they are HDR-merged; otherwise the first frame is
    /// used. Returns `None` when there are no frames.
    pub fn working_image(&self) -> Option<crate::core::GrayF32> {
        self.working_image_for_channel(self.project.channels.first().map(|c| c.id).unwrap_or(0))
    }

    /// The working image for one channel.
    ///
    /// Channels are separate acquisitions of the same gel, so each resolves on
    /// its own: an exposure bracket *within* a channel still HDR-merges, but
    /// frames from different channels are never merged together.
    pub fn working_image_for_channel(&self, channel: u32) -> Option<crate::core::GrayF32> {
        use std::collections::BTreeMap;
        // A saved (possibly option-tuned) HDR merge wins over re-merging.
        if let Some(merged) = &self.merged {
            return Some(merged.clone());
        }
        if self.frames.is_empty() {
            return None;
        }
        let in_channel = self.project.image_indices_for_channel(channel);
        // An unknown channel id falls back to every frame rather than to none:
        // showing the wrong channel beats showing a blank window.
        let in_channel = if in_channel.is_empty() {
            (0..self.frames.len()).collect()
        } else {
            in_channel
        };
        let first = *in_channel.first()?;
        let mut groups: BTreeMap<Option<u32>, Vec<usize>> = BTreeMap::new();
        for &i in &in_channel {
            groups
                .entry(self.project.images[i].meta.bracket_group)
                .or_default()
                .push(i);
        }
        let bracket = groups
            .iter()
            .filter(|(k, v)| k.is_some() && v.len() > 1)
            .max_by_key(|(_, v)| v.len());
        if let Some((_, idxs)) = bracket {
            let frames: Vec<crate::core::GrayF32> = idxs
                .iter()
                .map(|&i| crate::core::GrayF32::from_dynamic(&self.frames[i]))
                .collect();
            let exposures: Vec<f64> = idxs
                .iter()
                .map(|&i| self.project.images[i].meta.exposure_seconds)
                .collect();
            if exposures.iter().all(|&t| t > 0.0) {
                if let Ok(merged) = crate::core::hdr::merge_hdr(&frames, &exposures) {
                    return Some(merged);
                }
            }
        }
        Some(crate::core::GrayF32::from_dynamic(&self.frames[first]))
    }

    /// Load a document from an in-memory ZIP byte buffer.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;

        let manifest: Manifest = read_json(&mut zip, "manifest.json")?
            .ok_or(FormatError::MissingEntry("manifest.json"))?;
        let images: Vec<GelImage> = read_json(&mut zip, "metadata.json")?
            .ok_or(FormatError::MissingEntry("metadata.json"))?;
        let analysis: Analysis = read_json(&mut zip, "analysis.json")?.unwrap_or_default();

        let mut frames = Vec::with_capacity(images.len());
        for img in &images {
            let mut raw = Vec::new();
            zip.by_name(&img.filename)
                .map_err(|_| FormatError::MissingEntry("images/*.png"))?
                .read_to_end(&mut raw)?;
            let frame = image::load_from_memory_with_format(&raw, ImageFormat::Png)?;
            frames.push(frame);
        }

        // Reconstruct the saved HDR merge, if present.
        let merged = match manifest.hdr {
            Some(rec) => {
                let mut raw = Vec::new();
                zip.by_name(MERGED_FILENAME)
                    .map_err(|_| FormatError::MissingEntry("images/merged.png"))?
                    .read_to_end(&mut raw)?;
                let img = image::load_from_memory_with_format(&raw, ImageFormat::Png)?;
                Some(decode_merged_png(&img, rec.scale))
            }
            None => None,
        };

        let project = GelProject {
            format: manifest.format,
            version: manifest.version,
            gel_type: manifest.gel_type,
            images,
            channels: manifest.channels,
            metadata: manifest.metadata,
            display_inverted: manifest.display_inverted,
            analysis,
            hdr: manifest.hdr,
        };
        Ok(GelDocument {
            project,
            frames,
            merged,
        })
    }
}

/// Encode a linear-radiance merge as a 16-bit grayscale PNG, normalized so
/// `scale` maps to full-white: `png = clamp(radiance/scale, 0, 1) * 65535`.
fn encode_merged_png(merged: &GrayF32, scale: f64) -> Result<Vec<u8>> {
    let (w, h) = (merged.width() as u32, merged.height() as u32);
    let inv = if scale > 0.0 { 1.0 / scale as f32 } else { 1.0 };
    let mut buf = image::ImageBuffer::<image::Luma<u16>, Vec<u16>>::new(w, h);
    for (x, y, px) in buf.enumerate_pixels_mut() {
        let r = (merged.get(x as usize, y as usize) * inv).clamp(0.0, 1.0);
        *px = image::Luma([(r * 65535.0).round() as u16]);
    }
    let mut png = Vec::new();
    DynamicImage::ImageLuma16(buf).write_to(&mut Cursor::new(&mut png), ImageFormat::Png)?;
    Ok(png)
}

/// Inverse of [`encode_merged_png`]: `radiance = png / 65535 * scale`.
fn decode_merged_png(img: &DynamicImage, scale: f64) -> GrayF32 {
    let luma = img.to_luma16();
    let (w, h) = (luma.width() as usize, luma.height() as usize);
    let mut data = ndarray::Array2::<f32>::zeros((h, w));
    let s = scale as f32;
    for (x, y, px) in luma.enumerate_pixels() {
        data[[y as usize, x as usize]] = px.0[0] as f32 / 65535.0 * s;
    }
    GrayF32 { data }
}

fn write_json<W: Write + std::io::Seek, T: Serialize>(
    zip: &mut zip::ZipWriter<W>,
    name: &str,
    value: &T,
    opts: SimpleFileOptions,
) -> Result<()> {
    zip.start_file(name, opts)?;
    let json = serde_json::to_vec_pretty(value)?;
    zip.write_all(&json)?;
    Ok(())
}

fn read_json<R: Read + std::io::Seek, T: for<'de> Deserialize<'de>>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<Option<T>> {
    let mut file = match zip.by_name(name) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut s = String::new();
    file.read_to_string(&mut s)?;
    Ok(Some(serde_json::from_str(&s)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::hdr::HdrOptions;

    fn tiny_frame(v: u8) -> DynamicImage {
        DynamicImage::ImageLuma8(image::ImageBuffer::from_pixel(4, 4, image::Luma([v])))
    }

    #[test]
    fn hdr_merge_roundtrips_through_zip() {
        let frames = vec![tiny_frame(40), tiny_frame(80)];
        let mut doc = GelDocument::from_frames(GelType::Dna, frames, Vec::new());

        // A merged radiance image spanning [0, 1.5].
        let mut data = ndarray::Array2::<f32>::zeros((4, 4));
        for (i, v) in data.iter_mut().enumerate() {
            *v = i as f32 * 0.1;
        }
        let merged = GrayF32 { data };
        let scale = 1.6;
        doc.merged = Some(merged.clone());
        doc.project.hdr = Some(HdrRecord {
            options: HdrOptions {
                align: true,
                ..Default::default()
            },
            scale,
        });

        let bytes = doc.to_bytes().unwrap();
        let loaded = GelDocument::from_bytes(&bytes).unwrap();

        // The merge round-trips within 16-bit quantization.
        let lm = loaded.merged.as_ref().expect("merged persisted");
        assert_eq!((lm.width(), lm.height()), (4, 4));
        for (a, b) in merged.data.iter().zip(lm.data.iter()) {
            assert!((a - b).abs() < 1e-3, "{a} vs {b}");
        }
        // The record (options + scale) persisted, and working_image uses it.
        let rec = loaded.project.hdr.expect("hdr record");
        assert!(rec.options.align);
        assert!((rec.scale - scale).abs() < 1e-9);
        assert_eq!(loaded.working_image().map(|w| (w.width(), w.height())), Some((4, 4)));
    }

    #[test]
    fn document_without_hdr_still_loads() {
        // Backward compat: a doc with no HDR record has no merged.png and loads.
        let doc = GelDocument::from_frames(GelType::Dna, vec![tiny_frame(50)], Vec::new());
        let bytes = doc.to_bytes().unwrap();
        let loaded = GelDocument::from_bytes(&bytes).unwrap();
        assert!(loaded.merged.is_none());
        assert!(loaded.project.hdr.is_none());
    }
}
