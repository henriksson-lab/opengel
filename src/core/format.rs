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

use crate::core::model::{Analysis, GelImage, GelProject, GelType, FORMAT_VERSION};

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
}

/// A project plus its decoded image frames.
pub struct GelDocument {
    pub project: GelProject,
    /// Decoded frames, parallel to `project.images`.
    pub frames: Vec<DynamicImage>,
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
                meta,
            });
        }
        GelDocument { project, frames }
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
            let opts = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);

            let manifest = Manifest {
                format: "opengel".to_string(),
                version: FORMAT_VERSION,
                gel_type: self.project.gel_type,
            };
            write_json(&mut zip, "manifest.json", &manifest, opts)?;
            write_json(&mut zip, "metadata.json", &self.project.images, opts)?;
            write_json(&mut zip, "analysis.json", &self.project.analysis, opts)?;

            for (img, frame) in self.project.images.iter().zip(&self.frames) {
                // Images are already compressed (PNG), so store without deflate.
                let store = SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored);
                zip.start_file(&img.filename, store)?;
                let mut png = Vec::new();
                frame.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)?;
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
        use std::collections::BTreeMap;
        if self.frames.is_empty() {
            return None;
        }
        let mut groups: BTreeMap<Option<u32>, Vec<usize>> = BTreeMap::new();
        for (i, img) in self.project.images.iter().enumerate() {
            groups.entry(img.meta.bracket_group).or_default().push(i);
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
        Some(crate::core::GrayF32::from_dynamic(&self.frames[0]))
    }

    /// Load a document from an in-memory ZIP byte buffer.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;

        let manifest: Manifest = read_json(&mut zip, "manifest.json")?
            .ok_or(FormatError::MissingEntry("manifest.json"))?;
        let images: Vec<GelImage> = read_json(&mut zip, "metadata.json")?
            .ok_or(FormatError::MissingEntry("metadata.json"))?;
        let analysis: Analysis =
            read_json(&mut zip, "analysis.json")?.unwrap_or_default();

        let mut frames = Vec::with_capacity(images.len());
        for img in &images {
            let mut raw = Vec::new();
            zip.by_name(&img.filename)
                .map_err(|_| FormatError::MissingEntry("images/*.png"))?
                .read_to_end(&mut raw)?;
            let frame = image::load_from_memory_with_format(&raw, ImageFormat::Png)?;
            frames.push(frame);
        }

        let project = GelProject {
            format: manifest.format,
            version: manifest.version,
            gel_type: manifest.gel_type,
            images,
            analysis,
        };
        Ok(GelDocument { project, frames })
    }
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
