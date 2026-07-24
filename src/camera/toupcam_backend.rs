//! Clean-room userspace driver for Toupcam USB cameras (Cypress FX3 based,
//! USB VID 0x0547 and friends).
//!
//! Everything here was built from reverse-engineered USB captures; see `toupcam/PROTOCOL.md`
//! for the wire protocol and `toupcam/README.md` for the RE log.
//!
//! **Cross-platform.** Device I/O goes entirely through [`nusb`], which drives WinUSB on Windows,
//! usbfs on Linux, and IOKit on macOS — no vendor DLL, no OS-specific code here. Platform notes:
//! - **Linux:** needs read/write access to the USB device. Install the vendor udev rule
//!   (`toupcamsdk/linux/udev/99-toupcam.rules`, which sets `MODE=0666` for VID 0547/04b4) into
//!   `/etc/udev/rules.d/` and replug, or run as root. If a kernel driver (`gspca_touptek`) has
//!   bound the device, [`ToupcamBackend::open`] detaches it (via `detach_and_claim_interface`).
//! - **macOS:** works from userspace; no kernel driver to detach.
//! - **Windows:** the device must be on the WinUSB driver (it is by default for these cameras).
//!
//! Bench device: **U3CMOS08500KPA**, 3328×2548 RAW8 over bulk EP 0x81.

use anyhow::{bail, Context, Result as AnyhowResult};
use futures_lite::future::block_on;
use image::{DynamicImage, GrayImage};
use nusb::transfer::{ControlIn, ControlOut, ControlType, Recipient, RequestBuffer};
use nusb::Interface;
use serde::Deserialize;

use crate::camera::{
    Camera, CameraError, CameraInfo, Capabilities, Exposure, Result as CameraResult,
};

/// USB vendor IDs used by Toupcam / ToupTek cameras (Cypress EZ-USB families).
pub const VID_TOUPTEK: u16 = 0x0547;
pub const VID_CYPRESS: u16 = 0x04b4;
pub const VID_TOUPTEK2: u16 = 0x232f;

/// The specific camera on this bench: "USB3.0 Camera" = U3CMOS08500KPA.
pub const PID_BENCH_CAM: u16 = 0x13a1;

/// True for any USB vendor id the vendor INF claims as a Toupcam device.
pub fn is_toupcam(vid: u16) -> bool {
    matches!(vid, VID_TOUPTEK | VID_CYPRESS | VID_TOUPTEK2)
}

// ---- Wire format (from PROTOCOL.md §4) ---------------------------------------

/// Bulk IN endpoint carrying image data.
pub const EP_IMAGE: u8 = 0x81;

/// Sensor full resolution of the bench camera. TODO: read from the device / make
/// dynamic once the resolution-select registers are decoded.
pub const WIDTH: usize = 3328;
pub const HEIGHT: usize = 2548;

/// One frame is exactly WIDTH*HEIGHT bytes of RAW8 Bayer — no header, no padding.
/// (3328*2548 = 8_479_744, which the capture confirmed as 16×512KB + 91136.)
pub const FRAME_BYTES: usize = WIDTH * HEIGHT;

/// The bulk read size the vendor DLL uses (512 KiB). A frame is a run of these
/// terminated by a short/zero-length transfer.
pub const BULK_CHUNK: usize = 512 * 1024;

/// XOR key obfuscating every `0x0b` runtime register byte (PROTOCOL.md §9).
pub const OBFUS_KEY: u8 = 0xEA;
/// Sensor line time in µs (exposure register = µs / line_time; verified r=1.0 over a 14-pt sweep;
/// 37.983 reproduces the vendor DLL's line counts exactly across the captured sweep).
pub const LINE_TIME_US: f64 = 37.983;
/// Minimum frame length in lines (≈99 ms → ~10 fps floor); exposure extends the frame past this.
pub const MIN_FRAME_LINES: u32 = 2608;

// ---- Init script (data-driven from the capture) ------------------------------

/// One replayed control transfer from the captured init sequence.
#[derive(Debug, Clone, serde::Deserialize)]
struct Step {
    dir: String,
    #[serde(rename = "bRequest", deserialize_with = "hex_u8")]
    b_request: u8,
    #[serde(rename = "wValue", deserialize_with = "hex_u16")]
    w_value: u16,
    #[serde(rename = "wIndex", deserialize_with = "hex_u16")]
    w_index: u16,
    #[serde(rename = "wLength")]
    w_length: u16,
    #[serde(default)]
    data: String, // hex, OUT payload
}

fn hex_u8<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<u8, D::Error> {
    let s = String::deserialize(d)?;
    u8::from_str_radix(s.trim_start_matches("0x"), 16).map_err(serde::de::Error::custom)
}
fn hex_u16<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<u16, D::Error> {
    let s = String::deserialize(d)?;
    u16::from_str_radix(s.trim_start_matches("0x"), 16).map_err(serde::de::Error::custom)
}

fn hex_bytes(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap_or(0))
        .collect()
}

/// The captured cold-open init sequence (see PROTOCOL.md §3). Regenerate with:
/// `reveng-rec ctrl captures/session-a --json --range 5096:5892 | grep -v '"bRequest":"0x0b"'`
const INIT_SCRIPT: &str = include_str!("toupcam_init_seq.jsonl");

// ---- Camera ------------------------------------------------------------------

pub struct ToupcamBackend {
    info: CameraInfo,
    iface: Interface,
    exposure_s: Option<f64>,
}

impl ToupcamBackend {
    /// Find and open the first Toupcam device, claiming interface 0.
    pub fn open() -> AnyhowResult<ToupcamBackend> {
        Self::open_opts(0, false)
    }

    /// Open with an optional USB device reset first (clean state, like a fresh enumeration).
    pub fn open_opts(index: usize, reset: bool) -> AnyhowResult<ToupcamBackend> {
        let di = nusb::list_devices()?
            .filter(|d| is_toupcam(d.vendor_id()))
            .nth(index)
            .context("no Toupcam (VID 0547/04b4/232f) device found")?;
        let info = camera_info_from_device(index, &di);
        let dev = di
            .open()
            .context("open failed — is another app (ToupView/SDK) holding the camera?")?;
        if reset {
            // Linux/macOS: re-enumerate to a clean state (helps un-wedge the FX3 after aborted
            // attempts). No-op error on Windows, which is fine.
            let _ = dev.reset();
        }
        // Cross-platform claim: on Linux this detaches a bound kernel driver (e.g. gspca_touptek)
        // first; on Windows/macOS it's a plain claim.
        let iface = dev
            .detach_and_claim_interface(0)
            .context("claim interface 0 failed (Linux: add udev rule or run as root)")?;
        let _ = iface.set_alt_setting(0); // activate the (single) alt setting's endpoints
        let cam = ToupcamBackend {
            info,
            iface,
            exposure_s: None,
        };
        cam.init()?;
        Ok(cam)
    }

    /// Vendor control OUT (host→device).
    pub fn vendor_out(&self, request: u8, value: u16, index: u16, data: &[u8]) -> AnyhowResult<()> {
        block_on(self.iface.control_out(ControlOut {
            control_type: ControlType::Vendor,
            recipient: Recipient::Device,
            request,
            value,
            index,
            data,
        }))
        .into_result()
        .with_context(|| {
            format!("control_out req=0x{request:02x} val=0x{value:04x} idx=0x{index:04x}")
        })?;
        Ok(())
    }

    /// Vendor control IN (device→host); returns up to `length` bytes.
    pub fn vendor_in(
        &self,
        request: u8,
        value: u16,
        index: u16,
        length: u16,
    ) -> AnyhowResult<Vec<u8>> {
        let data = block_on(self.iface.control_in(ControlIn {
            control_type: ControlType::Vendor,
            recipient: Recipient::Device,
            request,
            value,
            index,
            length,
        }))
        .into_result()
        .with_context(|| {
            format!("control_in req=0x{request:02x} val=0x{value:04x} idx=0x{index:04x}")
        })?;
        Ok(data)
    }

    /// Replay the captured init sequence (auth + sensor register table + calibration reads +
    /// stream-start) verbatim. This is the **proven-working** path: the auth challenge/response
    /// is deterministic (`auth_probe` confirmed the device reproduces the captured exchange
    /// exactly), so a byte-exact replay authenticates. After this + a queued [`read_frame`], the
    /// camera streams (validated on hardware — captured a live frame).
    pub fn init(&self) -> AnyhowResult<()> {
        self.init_opts(false)
    }

    pub fn init_opts(&self, skip_auth: bool) -> AnyhowResult<()> {
        let (mut n, mut skipped, mut errs) = (0usize, 0usize, 0usize);
        for (lineno, line) in INIT_SCRIPT.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let step: Step = serde_json::from_str(line)
                .with_context(|| format!("init_seq.jsonl line {}", lineno + 1))?;
            // The auth handshake is the pair of 16-byte transfers; skip both.
            if skip_auth && step.w_length == 16 {
                skipped += 1;
                continue;
            }
            // Readiness poll: the vendor DLL reads req 0x37 until the device reports ready
            // (bit0 = 1). Replaying it as fixed reads can race the device, so actually poll.
            if step.b_request == 0x37 && step.dir == "in" {
                for _ in 0..200 {
                    match self.vendor_in(0x37, step.w_value, step.w_index, step.w_length) {
                        Ok(d) if d.first().map(|b| b & 1 == 1).unwrap_or(false) => break,
                        _ => std::thread::sleep(std::time::Duration::from_millis(5)),
                    }
                }
                n += 1;
                continue;
            }
            // Individual init commands may error on real hardware (e.g. the 0x23 capability
            // probe STALLs — it returned error status in the capture too, and the vendor DLL
            // ignores it). A control-pipe STALL auto-clears on the next setup, so log and
            // continue rather than abort, mirroring the DLL.
            let res = if step.dir == "out" {
                self.vendor_out(
                    step.b_request,
                    step.w_value,
                    step.w_index,
                    &hex_bytes(&step.data),
                )
            } else {
                self.vendor_in(step.b_request, step.w_value, step.w_index, step.w_length)
                    .map(|_| ())
            };
            match res {
                Ok(()) => n += 1,
                Err(e) => {
                    errs += 1;
                    eprintln!(
                        "init: step {} req=0x{:02x} val=0x{:04x} idx=0x{:04x} failed ({e}) — continuing",
                        lineno + 1, step.b_request, step.w_value, step.w_index
                    );
                }
            }
        }
        eprintln!("init: {n} transfers ok, {errs} tolerated errors, {skipped} auth skipped; stream start issued");
        Ok(())
    }

    // ---- Runtime controls over the `0x0b` channel (PROTOCOL.md §9) -----------------------
    //
    // Every value byte is obfuscated as `byte XOR 0xEA`, carried in the low byte of `wValue`
    // (high byte constant 0xa7); `wIndex` is the (stable, obfuscated) register address; the
    // transfer is a 1-byte control-IN whose returned `0x08` is just an ack. A parameter write
    // is bracketed by register 0xa6ee (write 1 = latch on, write 0 = latch off).

    /// Send one raw `0x0b` register write (obfuscated `windex`/`wvalue` as built by
    /// [`exposure_regs`]/[`gain_regs`]). The 1-byte `0x08` ack is discarded.
    fn reg0b_raw(&self, windex: u16, wvalue: u16) -> AnyhowResult<()> {
        let _ = self.vendor_in(0x0b, wvalue, windex, 1)?;
        Ok(())
    }

    /// Set exposure time in **microseconds** (integration lines + derived frame length).
    pub fn set_exposure_us(&self, us: u32) -> AnyhowResult<()> {
        for (windex, wvalue) in exposure_regs(us) {
            self.reg0b_raw(windex, wvalue)?;
        }
        Ok(())
    }

    /// Set analog gain in **percent** (100 = 1×).
    pub fn set_gain_pct(&self, pct: u16) -> AnyhowResult<()> {
        for (windex, wvalue) in gain_regs(pct) {
            self.reg0b_raw(windex, wvalue)?;
        }
        Ok(())
    }

    /// A clone of the claimed interface (for building a bulk transfer queue — FX3 streaming needs
    /// several IN requests outstanding for the DMA to start).
    pub fn iface(&self) -> Interface {
        self.iface.clone()
    }

    /// Clear a halt/stall condition on the image endpoint (best-effort). Some devices leave the
    /// bulk pipe halted until the host clears it before streaming.
    pub fn clear_image_halt(&self) {
        let _ = self.iface.clear_halt(EP_IMAGE);
    }

    /// One bulk-IN read with a timeout (nusb's `block_on` has none). Returns `Ok(None)` on
    /// timeout, `Ok(Some(bytes))` on data (possibly a zero-length packet), `Err` on a USB error.
    /// Diagnostic-grade: on timeout the underlying transfer is abandoned on its worker thread.
    pub fn read_bulk_timeout(&self, len: usize, ms: u64) -> AnyhowResult<Option<Vec<u8>>> {
        use std::sync::mpsc::channel;
        use std::time::Duration;
        let iface = self.iface.clone();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let r = block_on(iface.bulk_in(EP_IMAGE, RequestBuffer::new(len))).into_result();
            let _ = tx.send(r);
        });
        match rx.recv_timeout(Duration::from_millis(ms)) {
            Ok(Ok(data)) => Ok(Some(data)),
            Ok(Err(e)) => Err(anyhow::anyhow!("bulk_in: {e}")),
            Err(_) => Ok(None),
        }
    }

    /// Read one RAW8 frame off the bulk endpoint using a **queued** read — the FX3 DMA only
    /// streams while several IN requests are outstanding, so a single `bulk_in` never gets data
    /// (learned the hard way). Keeps 16 requests in flight on a worker thread and accumulates
    /// [`FRAME_BYTES`]. `timeout_ms` bounds the wait for a full frame.
    pub fn read_frame(&self) -> AnyhowResult<Vec<u8>> {
        self.read_frame_timeout(15_000)
    }

    pub fn read_frame_timeout(&self, timeout_ms: u64) -> AnyhowResult<Vec<u8>> {
        use std::sync::mpsc::{channel, RecvTimeoutError};
        use std::time::{Duration, Instant};
        let iface = self.iface.clone();
        let (tx, rx) = channel::<std::result::Result<Vec<u8>, String>>();
        std::thread::spawn(move || {
            let mut q = iface.bulk_in_queue(EP_IMAGE);
            for _ in 0..16 {
                q.submit(RequestBuffer::new(BULK_CHUNK));
            }
            loop {
                let c = block_on(q.next_complete());
                let msg = c.status.map(|_| c.data.clone()).map_err(|e| e.to_string());
                let stop = msg.is_err();
                if tx.send(msg).is_err() || stop {
                    return;
                }
                q.submit(RequestBuffer::new(BULK_CHUNK));
            }
        });
        let mut frame = Vec::with_capacity(FRAME_BYTES + BULK_CHUNK);
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        while frame.len() < FRAME_BYTES {
            let now = Instant::now();
            if now >= deadline {
                bail!(
                    "frame read timed out ({} of {FRAME_BYTES} bytes)",
                    frame.len()
                );
            }
            match rx.recv_timeout(deadline - now) {
                Ok(Ok(d)) => frame.extend_from_slice(&d),
                Ok(Err(e)) => bail!("bulk stream error: {e}"),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => bail!("bulk stream thread ended"),
            }
        }
        if frame.len() < FRAME_BYTES {
            bail!("short frame: {} of {FRAME_BYTES} bytes", frame.len());
        }
        frame.truncate(FRAME_BYTES);
        Ok(frame)
    }
}

fn camera_info_from_device(index: usize, dev: &nusb::DeviceInfo) -> CameraInfo {
    let product = dev.product_string().unwrap_or("Toupcam USB Camera");
    let serial = dev.serial_number().unwrap_or("");
    let serial_suffix = if serial.is_empty() {
        String::new()
    } else {
        format!(" serial {serial}")
    };
    CameraInfo {
        index,
        name: format!(
            "Toupcam {product} {:04x}:{:04x} bus {} addr {}{}",
            dev.vendor_id(),
            dev.product_id(),
            dev.bus_number(),
            dev.device_address(),
            serial_suffix
        ),
    }
}

fn backend_err(e: impl std::fmt::Display) -> CameraError {
    CameraError::Backend(e.to_string())
}

/// List available Toupcam/ToupTek USB devices.
pub fn list_cameras() -> CameraResult<Vec<CameraInfo>> {
    let devices = nusb::list_devices().map_err(backend_err)?;
    Ok(devices
        .filter(|d| is_toupcam(d.vendor_id()))
        .enumerate()
        .map(|(index, dev)| camera_info_from_device(index, &dev))
        .collect())
}

/// Open a Toupcam/ToupTek USB device by the index returned from [`list_cameras`].
pub fn open(index: usize) -> CameraResult<ToupcamBackend> {
    ToupcamBackend::open_opts(index, false).map_err(backend_err)
}

impl Camera for ToupcamBackend {
    fn info(&self) -> &CameraInfo {
        &self.info
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            manual_exposure: true,
            gain: true,
            exposure_range_s: Some((
                LINE_TIME_US / 1_000_000.0,
                0xffff as f64 * LINE_TIME_US / 1_000_000.0,
            )),
        }
    }

    fn set_exposure(&mut self, exposure: Exposure) -> CameraResult<bool> {
        match exposure {
            Exposure::Auto => Ok(false),
            Exposure::Manual(t) => {
                let min_s = LINE_TIME_US / 1_000_000.0;
                let max_s = 0xffff as f64 * LINE_TIME_US / 1_000_000.0;
                let clamped = t.clamp(min_s, max_s);
                let us = (clamped * 1_000_000.0).round().clamp(1.0, u32::MAX as f64) as u32;
                self.set_exposure_us(us).map_err(backend_err)?;
                self.exposure_s = Some(us as f64 / 1_000_000.0);
                Ok(true)
            }
        }
    }

    fn current_exposure_s(&self) -> Option<f64> {
        self.exposure_s
    }

    fn capture(&mut self) -> CameraResult<DynamicImage> {
        let raw = self.read_frame().map_err(backend_err)?;
        let img = GrayImage::from_raw(WIDTH as u32, HEIGHT as u32, raw)
            .ok_or_else(|| CameraError::Capture("Toupcam raw frame size mismatch".into()))?;
        Ok(DynamicImage::ImageLuma8(img))
    }
}

/// Obfuscate a register byte into a `0x0b` `wValue` (high byte constant `0xa7`, low byte XOR key).
fn wval(byte: u8) -> u16 {
    0xa700 | (byte ^ OBFUS_KEY) as u16
}

/// The exact `(wIndex, wValue)` sequence a `set_exposure_us` transaction sends (PROTOCOL.md §9).
/// Pure — verified byte-for-byte against the captured vendor traffic in the tests below.
pub fn exposure_regs(us: u32) -> Vec<(u16, u16)> {
    let lines = ((us as f64 / LINE_TIME_US).round() as u32).clamp(1, 0xffff);
    let frame = lines.saturating_add(22).max(MIN_FRAME_LINES);
    let (lh, ll) = ((lines >> 8) as u8, lines as u8);
    let (fh, fl) = ((frame >> 8) as u8, frame as u8);
    vec![
        (0xa6ee, wval(1)), // latch on
        (0xa5e8, wval(lh)),
        (0xa5e9, wval(ll)),
        (0xa4aa, wval(fh)),
        (0xa4ab, wval(fl)),
        (0x96ea, wval(0)),
        (0x95fa, wval(0)),
        (0xa6ee, wval(0)), // latch off
    ]
}

/// The `(wIndex, wValue)` sequence for a `set_gain_pct` transaction. `gain% = 100·1024/(1024-code)`.
pub fn gain_regs(pct: u16) -> Vec<(u16, u16)> {
    let pct = pct.max(100) as f64;
    let code = (1024.0 - 102400.0 / pct).round().clamp(0.0, 1023.0) as u32;
    vec![
        (0xa6ee, wval(1)),
        (0xa5ee, wval((code >> 8) as u8)),
        (0xa5ef, wval(code as u8)),
        (0xa6ee, wval(0)),
    ]
}

/// Save a RAW8 buffer as a binary PGM (P5) grayscale image — viewable everywhere,
/// no image-encoding dependency. This shows the raw Bayer mosaic (not debayered).
pub fn save_pgm(path: &str, raw: &[u8], width: usize, height: usize) -> AnyhowResult<()> {
    use std::io::Write;
    if raw.len() < width * height {
        bail!("raw buffer {} smaller than {width}x{height}", raw.len());
    }
    let mut f = std::fs::File::create(path)?;
    write!(f, "P5\n{width} {height}\n255\n")?;
    f.write_all(&raw[..width * height])?;
    Ok(())
}

/// Bayer color-filter phase — which color sits at the top-left pixel. TBD for this sensor;
/// try all four against a lit frame and keep the one whose colors are correct.
#[derive(Clone, Copy, Debug)]
pub enum Bayer {
    Rggb,
    Grbg,
    Gbrg,
    Bggr,
}

impl Bayer {
    pub const ALL: [Bayer; 4] = [Bayer::Rggb, Bayer::Grbg, Bayer::Gbrg, Bayer::Bggr];
    pub fn name(self) -> &'static str {
        match self {
            Bayer::Rggb => "rggb",
            Bayer::Grbg => "grbg",
            Bayer::Gbrg => "gbrg",
            Bayer::Bggr => "bggr",
        }
    }
}

/// Half-resolution debayer: collapse each 2×2 Bayer block to one RGB pixel (R + averaged G + B).
/// Simple, artifact-free, and enough to see the image and verify color/phase. Returns
/// `(out_w, out_h, rgb)` with `rgb` tightly packed `out_w*out_h*3`.
pub fn debayer_half(
    raw: &[u8],
    width: usize,
    height: usize,
    phase: Bayer,
) -> (usize, usize, Vec<u8>) {
    let (ow, oh) = (width / 2, height / 2);
    let mut out = vec![0u8; ow * oh * 3];
    let at = |x: usize, y: usize| raw[y * width + x] as u32;
    for by in 0..oh {
        for bx in 0..ow {
            let (x, y) = (bx * 2, by * 2);
            let (p00, p01, p10, p11) = (at(x, y), at(x + 1, y), at(x, y + 1), at(x + 1, y + 1));
            let (r, g, b) = match phase {
                Bayer::Rggb => (p00, (p01 + p10) / 2, p11),
                Bayer::Bggr => (p11, (p01 + p10) / 2, p00),
                Bayer::Grbg => (p01, (p00 + p11) / 2, p10),
                Bayer::Gbrg => (p10, (p00 + p11) / 2, p01),
            };
            let o = (by * ow + bx) * 3;
            out[o] = r as u8;
            out[o + 1] = g as u8;
            out[o + 2] = b as u8;
        }
    }
    (ow, oh, out)
}

/// Save a RAW8 (grayscale) buffer as PNG.
pub fn save_png_gray(path: &str, raw: &[u8], width: usize, height: usize) -> AnyhowResult<()> {
    let img =
        image::GrayImage::from_raw(width as u32, height as u32, raw[..width * height].to_vec())
            .context("gray buffer size mismatch")?;
    img.save(path)?;
    Ok(())
}

/// Save packed RGB8 as PNG.
pub fn save_png_rgb(path: &str, rgb: &[u8], width: usize, height: usize) -> AnyhowResult<()> {
    let img = image::RgbImage::from_raw(
        width as u32,
        height as u32,
        rgb[..width * height * 3].to_vec(),
    )
    .context("rgb buffer size mismatch")?;
    img.save(path)?;
    Ok(())
}

/// Save packed RGB8 as a binary PPM (P6).
pub fn save_ppm(path: &str, rgb: &[u8], width: usize, height: usize) -> AnyhowResult<()> {
    use std::io::Write;
    if rgb.len() < width * height * 3 {
        bail!("rgb buffer too small for {width}x{height}");
    }
    let mut f = std::fs::File::create(path)?;
    write!(f, "P6\n{width} {height}\n255\n")?;
    f.write_all(&rgb[..width * height * 3])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Byte-for-byte against captures/sweep2 (the vendor DLL's own traffic), proving our
    // control-encoding (XOR 0xEA, big-endian split, a6ee latch bracket) is correct.

    #[test]
    fn exposure_regs_match_captured_wire() {
        // 215021 µs -> a5e8=a7fc a5e9=a7f7 a4aa=a7fc a4ab=a7d9 (lines=5661, frame=5683)
        let r = exposure_regs(215021);
        assert_eq!(r[0], (0xa6ee, 0xa7eb), "latch on = write 1");
        assert_eq!(r[1], (0xa5e8, 0xa7fc), "exposure high");
        assert_eq!(r[2], (0xa5e9, 0xa7f7), "exposure low");
        assert_eq!(r[3], (0xa4aa, 0xa7fc), "frame high");
        assert_eq!(r[4], (0xa4ab, 0xa7d9), "frame low");
        assert_eq!(r[7], (0xa6ee, 0xa7ea), "latch off = write 0");
    }

    #[test]
    fn gain_regs_match_captured_wire() {
        // 3334 % -> a5ee=a7e9 a5ef=a70b (code=993)
        let r = gain_regs(3334);
        assert_eq!(r[0], (0xa6ee, 0xa7eb));
        assert_eq!(r[1], (0xa5ee, 0xa7e9));
        assert_eq!(r[2], (0xa5ef, 0xa70b));
        assert_eq!(r[3], (0xa6ee, 0xa7ea));
    }

    #[test]
    fn frame_length_clamps_at_min() {
        // Short exposure -> frame length stays at the 2608-line floor.
        let r = exposure_regs(5000);
        let fh = (r[3].1 & 0xff) as u8 ^ OBFUS_KEY;
        let fl = (r[4].1 & 0xff) as u8 ^ OBFUS_KEY;
        assert_eq!(((fh as u32) << 8) | fl as u32, MIN_FRAME_LINES);
    }
}
