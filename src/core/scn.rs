//! Reading Bio-Rad Image Lab `.scn` / `.mscn` scans.
//!
//! The container is a plain MIME `multipart/mixed` document — no magic number,
//! no index, no compression, no checksum. Top level is:
//!
//! ```text
//! part 0        text/xml   ItemHeaderTag            session + display state
//! part 1..N     multipart  ScanImageTag0..N-1       one per channel
//! part N+1      text/xml   ItemProtocolSettingsTag  protocol + analysis
//! ```
//!
//! and each channel part nests one more level:
//!
//! ```text
//! sub 0   application/octet-stream  ImageData    raw 16-bit LE pixels
//! sub 1   text/xml                  ImageHeader  geometry + acquisition record
//! ```
//!
//! Four extensions share this container: `.scn` (single channel), `.mscn`
//! (multi-channel), and the `.sscn` / `.smscn` "secured" variants. The secured
//! ones are *not* encrypted — they carry one extra `<document_signing>` element
//! holding an unkeyed digest — so the same parser reads all four and simply
//! ignores the signature.
//!
//! Analysis (lanes, bands, volumes) lives in the final part. Its schema is not
//! documented and is not read here; the pixels and the acquisition record are.

use std::path::Path;

use image::{DynamicImage, ImageBuffer, Luma};

use crate::core::model::{Attribute, Attributes, ChannelColor, GelType};

/// Extensions this reader recognizes, lowercase and without the dot.
pub const EXTENSIONS: [&str; 4] = ["scn", "mscn", "sscn", "smscn"];

/// True if `path` looks like an Image Lab scan by extension.
pub fn has_scn_extension(path: impl AsRef<Path>) -> bool {
    path.as_ref()
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| EXTENSIONS.contains(&e.as_str()))
}

#[derive(Debug, thiserror::Error)]
pub enum ScnError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not an Image Lab scan: {0}")]
    NotAScan(&'static str),
    #[error("malformed MIME container: {0}")]
    Mime(String),
    #[error("malformed XML: {0}")]
    Xml(#[from] roxmltree::Error),
    #[error("{0}")]
    Missing(String),
    #[error("channel {channel}: image data is {got} bytes, but {width}×{height}×2 needs {want}")]
    PixelCount {
        channel: usize,
        got: usize,
        want: usize,
        width: u32,
        height: u32,
    },
}

pub type Result<T> = std::result::Result<T, ScnError>;

/// One channel of a scan: its pixels, and what the instrument recorded about
/// how they were taken.
#[derive(Debug, Clone)]
pub struct ScnChannel {
    /// The source's `Application` attribute when it has one ("Stain Free Blot",
    /// "Ethidium Bromide"), else "Channel N".
    pub name: String,
    pub color: ChannelColor,
    pub width: u32,
    pub height: u32,
    /// Native sensor geometry before cropping, when it differs from the stored
    /// size — a Gel Doc EZ stores 1392×1000 cropped from a 1392×1040 readout.
    pub original_size: Option<(u32, u32)>,
    /// Physical size of the imaged area in millimetres, when the file says so.
    pub size_mm: Option<(f64, f64)>,
    /// The sample value the source treats as full scale: 4095 for 12-bit data,
    /// 65535 for 16-bit. Pixels are rescaled to full 16-bit range on import;
    /// this records what they came in as.
    pub max_value: u32,
    pub exposure_seconds: f64,
    /// ISO-8601 acquisition timestamp, when recorded.
    pub timestamp: Option<String>,
    /// The instrument model, e.g. "Gel Doc™ EZ".
    pub imager: Option<String>,
    /// The whole `<scan_attributes>` block, verbatim and in source order.
    pub acquisition: Attributes,
    /// 16-bit grayscale pixels, rescaled so `max_value` maps to 65535.
    pub image: DynamicImage,
}

/// A parsed Image Lab scan.
#[derive(Debug, Clone)]
pub struct ScnFile {
    /// The document name from the session header, when non-empty.
    pub name: Option<String>,
    /// Session-level metadata: name, user, description, scan id.
    pub metadata: Attributes,
    pub channels: Vec<ScnChannel>,
    /// The source asked for a zero sample to render white — the conventional
    /// presentation for blots. A display preference only; pixels are unchanged.
    pub display_inverted: bool,
}

impl ScnFile {
    /// Read and parse a scan from disk.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        Self::from_bytes(&bytes)
    }

    /// Parse a scan already in memory.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let top = Part::split_document(bytes)?;

        let mut name = None;
        let mut metadata = Attributes::new();
        let mut display_inverted = false;
        let mut colormaps: Vec<ChannelColor> = Vec::new();
        let mut channels: Vec<ScnChannel> = Vec::new();

        for part in &top {
            match part.description.as_deref() {
                Some("ItemHeaderTag") => {
                    let text = String::from_utf8_lossy(part.body);
                    let doc = parse_xml(&text)?;
                    let root = doc.root_element();
                    name = child_text(root, "name").filter(|s| !s.is_empty());
                    metadata = session_metadata(root);
                    colormaps = scan_colormaps(root);
                }
                // `ScanImageTag0`, `ScanImageTag1`, … — one nested multipart per
                // channel. Matched by prefix because the index is part of the
                // name.
                Some(d) if d.starts_with("ScanImageTag") => {
                    let index = channels.len();
                    let (channel, zero_is_white) = parse_channel(part, index)?;
                    // Every channel of a real file agrees on this; take the
                    // first one's word for the document.
                    if index == 0 {
                        display_inverted = zero_is_white;
                    }
                    channels.push(channel);
                }
                // The trailing part holds protocol settings and Image Lab's own
                // lane/band analysis. Its schema is undocumented; skipped.
                _ => {}
            }
        }

        if channels.is_empty() {
            return Err(ScnError::NotAScan("no image parts in the container"));
        }

        // Colours come from the session header's `<scan_N>` blocks, which are
        // indexed in the same order as the image parts.
        for (i, channel) in channels.iter_mut().enumerate() {
            if let Some(&color) = colormaps.get(i) {
                channel.color = color;
            }
        }

        Ok(ScnFile {
            name,
            metadata,
            channels,
            display_inverted,
        })
    }

    /// The gel type implied by the channels' applications.
    ///
    /// Bio-Rad names a reagent rather than a sample kind, so this reads the
    /// reagent: Coomassie stains protein, ethidium bromide stains DNA. Falls
    /// back to DNA, the commoner case, when nothing matches — the user can
    /// change it, and guessing wrong is visible immediately in the size units.
    pub fn gel_type(&self) -> GelType {
        for channel in &self.channels {
            if let Some(t) = guess_gel_type(&channel.name) {
                return t;
            }
        }
        GelType::Dna
    }
}

/// Map a Bio-Rad application/reagent name onto a gel type.
fn guess_gel_type(application: &str) -> Option<GelType> {
    let a = application.to_ascii_lowercase();
    const PROTEIN: [&str; 8] = [
        "coomassie",
        "stain free",
        "stain-free",
        "sypro",
        "oriole",
        "blot",
        "silver",
        "flamingo",
    ];
    const DNA: [&str; 6] = [
        "ethidium",
        "etbr",
        "sybr",
        "gelred",
        "gelgreen",
        "sybr safe",
    ];
    if PROTEIN.iter().any(|k| a.contains(k)) {
        return Some(GelType::Protein);
    }
    if DNA.iter().any(|k| a.contains(k)) {
        return Some(GelType::Dna);
    }
    None
}

// ---- channel parsing -------------------------------------------------------

/// Parse one nested channel part into pixels plus metadata. Returns the channel
/// and whether the source wanted it displayed inverted.
fn parse_channel(part: &Part<'_>, index: usize) -> Result<(ScnChannel, bool)> {
    let boundary = part
        .boundary
        .as_deref()
        .ok_or_else(|| ScnError::Mime(format!("channel {index} is not a nested multipart")))?;
    let subs = Part::split_body(part.body, boundary)?;

    let data = subs
        .iter()
        .find(|s| s.description.as_deref() == Some("ImageData"))
        .ok_or_else(|| ScnError::Missing(format!("channel {index} has no ImageData part")))?;
    let header = subs
        .iter()
        .find(|s| s.description.as_deref() == Some("ImageHeader"))
        .ok_or_else(|| ScnError::Missing(format!("channel {index} has no ImageHeader part")))?;

    let text = String::from_utf8_lossy(header.body);
    let doc = parse_xml(&text)?;
    let root = doc.root_element();

    let (width, height) = size_element(root, "size_pix")
        .ok_or_else(|| ScnError::Missing(format!("channel {index} header has no <size_pix>")))?;
    let original_size = size_element(root, "org_size_pix").filter(|&s| s != (width, height));
    let size_mm = size_element_f64(root, "size_mm");
    let little_endian = child_text(root, "endian")
        .map(|e| !e.eq_ignore_ascii_case("big"))
        .unwrap_or(true);
    let max_value = element_attr(root, "scanner", "max_value")
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(u16::MAX as u32);
    let zero_is_white = element_attr(root, "image", "zero_is")
        .is_some_and(|v| v.eq_ignore_ascii_case("white"));

    let acquisition = scan_attributes(root);
    let lookup = |key: &str| {
        acquisition
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(key))
            .map(|a| a.value.clone())
    };
    let exposure_seconds = lookup("Exposure Time (sec)")
        .and_then(|v| v.trim().parse::<f64>().ok())
        .unwrap_or(0.0);
    let timestamp = lookup("Image Date");
    let imager = lookup("Imager").map(|s| s.trim().to_string());
    let name = lookup("Application")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("Channel {}", index + 1));

    let image = decode_pixels(data.body, width, height, little_endian, max_value, index)?;

    Ok((
        ScnChannel {
            name,
            color: ChannelColor::Gray,
            width,
            height,
            original_size,
            size_mm,
            max_value,
            exposure_seconds,
            timestamp,
            imager,
            acquisition,
            image,
        },
        zero_is_white,
    ))
}

/// Turn the raw sample buffer into a 16-bit grayscale image.
///
/// Samples are rescaled so the source's `max_value` maps to 65535. Without
/// this, 12-bit data (`max_value = 4095`) would sit in the bottom sixteenth of
/// the range and every downstream normalization — display, thresholds, the HDR
/// merge — would read it as a nearly black image. The scaling is a pure change
/// of units: no information is added or lost, and the original ceiling is kept
/// in [`ScnChannel::max_value`].
fn decode_pixels(
    body: &[u8],
    width: u32,
    height: u32,
    little_endian: bool,
    max_value: u32,
    channel: usize,
) -> Result<DynamicImage> {
    let want = width as usize * height as usize * 2;
    if body.len() < want {
        return Err(ScnError::PixelCount {
            channel,
            got: body.len(),
            want,
            width,
            height,
        });
    }
    let scale = u16::MAX as f64 / max_value as f64;
    let samples: Vec<u16> = body[..want]
        .chunks_exact(2)
        .map(|c| {
            let raw = if little_endian {
                u16::from_le_bytes([c[0], c[1]])
            } else {
                u16::from_be_bytes([c[0], c[1]])
            };
            if max_value == u16::MAX as u32 {
                raw
            } else {
                ((raw as f64 * scale).round() as u32).min(u16::MAX as u32) as u16
            }
        })
        .collect();
    let buf = ImageBuffer::<Luma<u16>, Vec<u16>>::from_raw(width, height, samples)
        .ok_or_else(|| ScnError::Missing(format!("channel {channel}: pixel buffer rejected")))?;
    Ok(DynamicImage::ImageLuma16(buf))
}

// ---- XML helpers -----------------------------------------------------------

/// Parse one of the metadata documents.
///
/// Every XML body in these files opens with `<!DOCTYPE XML>`, which roxmltree
/// refuses unless asked — its default guards against entity-expansion attacks.
/// The declaration here is inert (it defines no entities and references no
/// external subset), and roxmltree never fetches external resources, so
/// allowing it costs nothing and is the difference between reading a real file
/// and rejecting every one of them.
fn parse_xml(text: &str) -> Result<roxmltree::Document<'_>> {
    let opts = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    Ok(roxmltree::Document::parse_with_options(text, opts)?)
}

fn child<'a>(node: roxmltree::Node<'a, 'a>, tag: &str) -> Option<roxmltree::Node<'a, 'a>> {
    node.children().find(|c| c.has_tag_name(tag))
}

fn child_text(node: roxmltree::Node<'_, '_>, tag: &str) -> Option<String> {
    child(node, tag).map(|c| c.text().unwrap_or("").trim().to_string())
}

fn element_attr(node: roxmltree::Node<'_, '_>, tag: &str, attr: &str) -> Option<String> {
    child(node, tag)?.attribute(attr).map(|s| s.to_string())
}

fn size_element(node: roxmltree::Node<'_, '_>, tag: &str) -> Option<(u32, u32)> {
    let e = child(node, tag)?;
    let w = e.attribute("width")?.parse().ok()?;
    let h = e.attribute("height")?.parse().ok()?;
    Some((w, h))
}

fn size_element_f64(node: roxmltree::Node<'_, '_>, tag: &str) -> Option<(f64, f64)> {
    let e = child(node, tag)?;
    let w = e.attribute("width")?.parse().ok()?;
    let h = e.attribute("height")?.parse().ok()?;
    Some((w, h))
}

/// The `<scan_attributes>` block as name/value pairs, in document order.
///
/// Each child carries a display `name` and a `value`; where the name is absent
/// the tag itself is used, so an attribute this code has never seen still
/// arrives with something readable next to it.
fn scan_attributes(root: roxmltree::Node<'_, '_>) -> Attributes {
    let Some(block) = child(root, "scan_attributes") else {
        return Attributes::new();
    };
    block
        .children()
        .filter(|c| c.is_element())
        .map(|c| {
            let name = c
                .attribute("name")
                .map(|s| s.to_string())
                .unwrap_or_else(|| c.tag_name().name().to_string());
            let value = c.attribute("value").unwrap_or("").trim().to_string();
            Attribute::new(name, value)
        })
        .collect()
}

/// Session-level metadata from the top-level header.
fn session_metadata(root: roxmltree::Node<'_, '_>) -> Attributes {
    const FIELDS: [(&str, &str); 4] = [
        ("name", "Name"),
        ("user", "User"),
        ("description", "Description"),
        ("scan_id", "Scan ID"),
    ];
    FIELDS
        .iter()
        .filter_map(|(tag, label)| {
            child_text(root, tag)
                .filter(|v| !v.is_empty())
                .map(|v| Attribute::new(*label, v))
        })
        .collect()
}

/// Per-channel colours from the `<scan_0>`, `<scan_1>`, … blocks.
fn scan_colormaps(root: roxmltree::Node<'_, '_>) -> Vec<ChannelColor> {
    let mut out = Vec::new();
    for i in 0.. {
        let Some(scan) = child(root, &format!("scan_{i}")) else {
            break;
        };
        out.push(
            child_text(scan, "colormap")
                .map(|c| ChannelColor::parse(&c))
                .unwrap_or_default(),
        );
    }
    out
}

// ---- MIME ------------------------------------------------------------------

/// One MIME part: its decoded headers and its body.
struct Part<'a> {
    description: Option<String>,
    /// Set when this part is itself a `multipart/*`.
    boundary: Option<String>,
    body: &'a [u8],
}

impl<'a> Part<'a> {
    /// Split a whole document: read its own header block for the boundary, then
    /// split the rest.
    fn split_document(bytes: &'a [u8]) -> Result<Vec<Part<'a>>> {
        let (headers, body) = split_headers(bytes)
            .ok_or(ScnError::NotAScan("no MIME header block at the start"))?;
        let headers = String::from_utf8_lossy(headers);
        if header_value(&headers, "Content-Type").is_none() {
            return Err(ScnError::NotAScan("no Content-Type header"));
        }
        let boundary = boundary_of(&headers)
            .ok_or(ScnError::NotAScan("Content-Type declares no boundary"))?;
        Part::split_body(body, &boundary)
    }

    /// Split a multipart body on `boundary`.
    ///
    /// A part ends at whichever comes first: its declared `Content-Length`, or
    /// the next delimiter. Both bounds are needed, and neither alone is right.
    ///
    /// The delimiter alone is not enough because image parts are raw 16-bit
    /// samples with no transfer encoding, so a buffer of pixels can spell the
    /// boundary by chance and truncate the image silently.
    ///
    /// `Content-Length` alone is not enough because Image Lab over-declares it
    /// for the XML parts — by six bytes in the sampled files — which drags the
    /// following delimiter into the body and makes the XML unparseable. Taking
    /// the smaller of the two absorbs that bug without giving up the protection
    /// for pixels.
    fn split_body(body: &'a [u8], boundary: &str) -> Result<Vec<Part<'a>>> {
        let delim = format!("--{boundary}");
        // A delimiter begins a line, so the newline before it belongs to the
        // delimiter rather than to the part. Match on the LF alone and strip a
        // preceding CR separately: headers in these files are CRLF, but the XML
        // bodies are LF and run straight into the delimiter with no CR at all.
        let mut line_delim = vec![b'\n'];
        line_delim.extend_from_slice(delim.as_bytes());

        let mut parts = Vec::new();
        let Some(mut pos) = find(body, delim.as_bytes()) else {
            return Err(ScnError::Mime(format!("boundary {boundary} never appears")));
        };

        loop {
            pos += delim.len();
            // `--boundary--` closes the multipart.
            if body[pos..].starts_with(b"--") {
                break;
            }
            let Some((headers, after)) = split_headers(&body[pos..]) else {
                break;
            };
            let header_text = String::from_utf8_lossy(headers);
            let start = body.len() - after.len();

            let declared_end = header_value(&header_text, "Content-Length")
                .and_then(|v| v.trim().parse::<usize>().ok())
                .map(|n| start + n)
                .filter(|&end| end <= body.len());
            let delim_pos = find(after, &line_delim).map(|p| start + p);

            let mut end = match (declared_end, delim_pos) {
                (Some(a), Some(b)) => a.min(b),
                (Some(a), None) => a,
                (None, Some(b)) => b,
                (None, None) => body.len(),
            };
            if Some(end) == delim_pos && end > start && body[end - 1] == b'\r' {
                end -= 1;
            }

            parts.push(Part {
                description: header_value(&header_text, "Content-Description")
                    .map(|s| s.trim().to_string()),
                boundary: boundary_of(&header_text),
                body: &body[start..end],
            });

            // Resume from the delimiter that closed this part. Searching from
            // `end` rather than from `start` keeps pixel data that happens to
            // spell the boundary from being read as one.
            match find(&body[end..], delim.as_bytes()) {
                Some(p) => pos = end + p,
                None => break,
            }
        }
        Ok(parts)
    }
}

/// Split a header block from the body it precedes, at the blank line.
///
/// Headers are CRLF-terminated in these files, but a bare-LF blank line is
/// accepted too — it costs one branch and saves a baffling parse failure on a
/// file some other tool rewrote.
fn split_headers(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let crlf = find(bytes, b"\r\n\r\n").map(|p| (p, p + 4));
    let lf = find(bytes, b"\n\n").map(|p| (p, p + 2));
    let (end, start) = match (crlf, lf) {
        (Some(a), Some(b)) => {
            if a.0 <= b.0 {
                a
            } else {
                b
            }
        }
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => return None,
    };
    Some((&bytes[..end], &bytes[start..]))
}

/// Case-insensitive lookup of a header value, continuation lines not supported
/// (these files use none).
fn header_value(headers: &str, name: &str) -> Option<String> {
    headers.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(name)
            .then(|| value.trim().to_string())
    })
}

/// The `boundary="…"` parameter of a `Content-Type`, if it is a multipart.
fn boundary_of(headers: &str) -> Option<String> {
    let ct = header_value(headers, "Content-Type")?;
    if !ct.to_ascii_lowercase().contains("multipart/") {
        return None;
    }
    let after = &ct[ct.to_ascii_lowercase().find("boundary=")? + "boundary=".len()..];
    let trimmed = after.trim_start();
    Some(match trimmed.strip_prefix('"') {
        Some(quoted) => quoted.split('"').next()?.to_string(),
        None => trimmed
            .split(|c: char| c == ';' || c.is_whitespace())
            .next()?
            .to_string(),
    })
}

/// First index of `needle` in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal two-channel document, built by hand so the parser is tested
    /// without needing a vendor file present.
    fn synthetic(channels: usize, max_value: u32, zero_is: &str) -> Vec<u8> {
        let (w, h) = (2u32, 2u32);
        let mut out = Vec::new();
        out.extend_from_slice(b"MIME-Version: 1.0 (Generated by test)\r\n");
        out.extend_from_slice(b"Content-Type: multipart/mixed; boundary=\"TOP\"\r\n\r\n");

        let mut header_xml = String::from("<!DOCTYPE XML>\n<root version=\"1\">\n <name>synthetic</name>\n <user>tester</user>\n");
        header_xml.push_str(&format!(" <channel_count>{channels}</channel_count>\n"));
        for i in 0..channels {
            header_xml.push_str(&format!(
                " <scan_{i}>\n  <colormap>{}</colormap>\n </scan_{i}>\n",
                ["Red", "Green", "Blue"][i % 3]
            ));
        }
        header_xml.push_str("</root>\n");
        out.extend_from_slice(b"--TOP\r\n");
        out.extend_from_slice(b"Content-Type: text/xml; charset=\"utf8\"\r\n");
        out.extend_from_slice(format!("Content-Length: {}\r\n", header_xml.len()).as_bytes());
        out.extend_from_slice(b"Content-Description: ItemHeaderTag\r\n\r\n");
        out.extend_from_slice(header_xml.as_bytes());
        out.extend_from_slice(b"\r\n");

        for i in 0..channels {
            let inner = format!("SUB{i}");
            // 0, 2000, 4000, 6000 — clipped at the ceiling, so the top-right
            // sample saturates for 12-bit data and does not for 16-bit.
            let pixels: Vec<u8> = (0..(w * h) as usize)
                .flat_map(|p| ((p as u32 * 2000).min(max_value) as u16).to_le_bytes())
                .collect();
            let img_xml = format!(
                "<!DOCTYPE XML>\n<root>\n <endian>little</endian>\n <size_pix width=\"{w}\" height=\"{h}\"/>\n \
                 <org_size_pix width=\"{w}\" height=\"{}\"/>\n \
                 <scanner max_value=\"{max_value}\" data_ceiling=\"{max_value}\"/>\n \
                 <image zero_is=\"{zero_is}\"/>\n <scan_attributes>\n  \
                 <imager type=\"0\" value=\"Gel Doc\u{2122} EZ\" name=\"Imager\"/>\n  \
                 <exposure_time type=\"1\" value=\"0.5\" name=\"Exposure Time (sec)\"/>\n  \
                 <application type=\"0\" value=\"Ethidium Bromide\" name=\"Application\"/>\n \
                 </scan_attributes>\n</root>\n",
                h + 40
            );

            out.extend_from_slice(b"--TOP\r\n");
            out.extend_from_slice(
                format!("Content-Type: multipart/mixed; boundary=\"{inner}\"\r\n").as_bytes(),
            );
            out.extend_from_slice(format!("Content-Description: ScanImageTag{i}\r\n\r\n").as_bytes());

            out.extend_from_slice(format!("--{inner}\r\n").as_bytes());
            out.extend_from_slice(b"Content-Type: application/octet-stream\r\n");
            out.extend_from_slice(format!("Content-Length: {}\r\n", pixels.len()).as_bytes());
            out.extend_from_slice(b"Content-Description: ImageData\r\n\r\n");
            out.extend_from_slice(&pixels);
            out.extend_from_slice(b"\r\n");

            out.extend_from_slice(format!("--{inner}\r\n").as_bytes());
            out.extend_from_slice(b"Content-Type: text/xml; charset=\"utf8\"\r\n");
            out.extend_from_slice(format!("Content-Length: {}\r\n", img_xml.len()).as_bytes());
            out.extend_from_slice(b"Content-Description: ImageHeader\r\n\r\n");
            out.extend_from_slice(img_xml.as_bytes());
            out.extend_from_slice(b"\r\n");
            out.extend_from_slice(format!("--{inner}--\r\n").as_bytes());
        }

        out.extend_from_slice(b"--TOP\r\n");
        out.extend_from_slice(b"Content-Type: text/xml; charset=\"utf8\"\r\n");
        out.extend_from_slice(b"Content-Description: ItemProtocolSettingsTag\r\n\r\n");
        out.extend_from_slice(b"<root/>\r\n");
        out.extend_from_slice(b"--TOP--\r\n");
        out
    }

    #[test]
    fn a_single_channel_scan_parses() {
        let file = ScnFile::from_bytes(&synthetic(1, 4095, "black")).expect("parse");
        assert_eq!(file.channels.len(), 1);
        assert_eq!(file.name.as_deref(), Some("synthetic"));
        let c = &file.channels[0];
        assert_eq!((c.width, c.height), (2, 2));
        assert_eq!(c.name, "Ethidium Bromide");
        assert_eq!(c.exposure_seconds, 0.5);
        assert_eq!(c.imager.as_deref(), Some("Gel Doc™ EZ"));
        assert!(!file.display_inverted);
    }

    #[test]
    fn a_multichannel_scan_keeps_every_channel_and_its_colour() {
        let file = ScnFile::from_bytes(&synthetic(3, 65535, "black")).expect("parse");
        assert_eq!(file.channels.len(), 3);
        let colors: Vec<_> = file.channels.iter().map(|c| c.color).collect();
        assert_eq!(
            colors,
            vec![ChannelColor::Red, ChannelColor::Green, ChannelColor::Blue]
        );
    }

    #[test]
    fn twelve_bit_data_is_rescaled_to_the_full_range() {
        // Left as-is, a 4095-ceiling sample would read as 6% brightness and the
        // whole image would look black.
        let file = ScnFile::from_bytes(&synthetic(1, 4095, "black")).expect("parse");
        let img = file.channels[0].image.to_luma16();
        assert_eq!(file.channels[0].max_value, 4095);
        assert_eq!(img.get_pixel(0, 0).0[0], 0);
        // The last sample sits at the 4095 ceiling, so it must land at full
        // scale rather than at 6% of it.
        assert_eq!(img.get_pixel(1, 1).0[0], u16::MAX);
        // 2000/4095 of full scale, carried across proportionally.
        assert_eq!(img.get_pixel(1, 0).0[0], 32007);
    }

    #[test]
    fn sixteen_bit_data_passes_through_untouched() {
        let file = ScnFile::from_bytes(&synthetic(1, 65535, "black")).expect("parse");
        let img = file.channels[0].image.to_luma16();
        assert_eq!(img.get_pixel(1, 0).0[0], 2000);
        assert_eq!(img.get_pixel(0, 1).0[0], 4000);
        assert_eq!(img.get_pixel(1, 1).0[0], 6000);
    }

    #[test]
    fn zero_is_white_sets_the_display_preference_only() {
        let plain = ScnFile::from_bytes(&synthetic(1, 65535, "black")).expect("parse");
        let inverted = ScnFile::from_bytes(&synthetic(1, 65535, "white")).expect("parse");
        assert!(!plain.display_inverted);
        assert!(inverted.display_inverted);
        // The pixels themselves must be identical — only presentation differs.
        assert_eq!(
            plain.channels[0].image.to_luma16().into_raw(),
            inverted.channels[0].image.to_luma16().into_raw()
        );
    }

    #[test]
    fn the_native_readout_size_is_kept_when_it_differs() {
        let file = ScnFile::from_bytes(&synthetic(1, 65535, "black")).expect("parse");
        assert_eq!(file.channels[0].original_size, Some((2, 42)));
    }

    #[test]
    fn the_acquisition_record_is_carried_verbatim_and_in_order() {
        let file = ScnFile::from_bytes(&synthetic(1, 65535, "black")).expect("parse");
        let names: Vec<&str> = file.channels[0]
            .acquisition
            .iter()
            .map(|a| a.name.as_str())
            .collect();
        assert_eq!(names, vec!["Imager", "Exposure Time (sec)", "Application"]);
    }

    #[test]
    fn image_data_containing_the_boundary_string_is_not_truncated() {
        // The pixel buffer is raw, so it can spell the delimiter by chance.
        // Content-Length is what keeps that from silently cutting the image
        // short.
        let mut bytes = synthetic(1, 65535, "black");
        let at = find(&bytes, b"Content-Description: ImageData\r\n\r\n").unwrap()
            + "Content-Description: ImageData\r\n\r\n".len();
        bytes[at..at + 4].copy_from_slice(b"--SU");
        let file = ScnFile::from_bytes(&bytes).expect("parse");
        assert_eq!(file.channels[0].image.to_luma16().len(), 4);
    }

    #[test]
    fn a_non_scan_file_is_refused_rather_than_half_parsed() {
        assert!(ScnFile::from_bytes(b"not a mime document at all").is_err());
        assert!(ScnFile::from_bytes(&[]).is_err());
    }

    #[test]
    fn reagents_map_onto_gel_types() {
        assert_eq!(guess_gel_type("Ethidium Bromide"), Some(GelType::Dna));
        assert_eq!(guess_gel_type("Coomassie Blue"), Some(GelType::Protein));
        assert_eq!(guess_gel_type("Stain Free Blot"), Some(GelType::Protein));
        assert_eq!(guess_gel_type("Chemi"), None);
    }

    #[test]
    fn the_extension_check_accepts_all_four_variants_and_nothing_else() {
        for ext in ["scn", "mscn", "sscn", "smscn", "MSCN"] {
            assert!(has_scn_extension(format!("gel.{ext}")), "{ext}");
        }
        assert!(!has_scn_extension("gel.zip"));
        assert!(!has_scn_extension("gel.png"));
    }
}
