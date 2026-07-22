//! Synthetic camera backend.
//!
//! Renders a fixed gel-like scene whose captured brightness scales with the
//! exposure time (with clipping at the top and a noise floor at the bottom), so
//! exposure bracketing and HDR merge behave like a real linear sensor.

use crate::core::model::CaptureMeta;
use image::{DynamicImage, GrayImage, Luma};

use crate::camera::{Camera, CameraError, CameraInfo, Capabilities, Exposure, Result};

const W: u32 = 220;
const H: u32 = 300;
/// Sensor gain mapping scene radiance × seconds → normalized signal.
const K: f64 = 3.0;

pub struct MockCamera {
    info: CameraInfo,
    exposure_s: f64,
    /// Per-pixel scene radiance (exposure-independent).
    radiance: Vec<f64>,
}

/// Discover mock cameras (always one).
pub fn list_cameras() -> Vec<CameraInfo> {
    vec![CameraInfo {
        index: 0,
        name: "Mock Gel Camera".to_string(),
    }]
}

/// Open a mock camera by index.
pub fn open(index: usize) -> Result<MockCamera> {
    if index != 0 {
        return Err(CameraError::NotFound(index));
    }
    Ok(MockCamera {
        info: CameraInfo {
            index,
            name: "Mock Gel Camera".to_string(),
        },
        exposure_s: 0.2,
        radiance: build_radiance(),
    })
}

fn size_to_y(size: f64) -> f64 {
    let (ln_hi, ln_lo) = (10000f64.ln(), 500f64.ln());
    let slope = (280.0 - 20.0) / (ln_lo - ln_hi);
    20.0 + (size.ln() - ln_hi) * slope
}

/// Build a scene with a ladder lane and sample lanes spanning a wide radiance
/// range (faint and bright bands) to make dynamic range meaningful.
fn build_radiance() -> Vec<f64> {
    let mut buf = vec![0f64; (W * H) as usize];
    let (sx, sy) = (5.0f64, 3.5f64);
    // (x, size, peak_radiance)
    let mut spots: Vec<(f64, f64, f64)> = Vec::new();
    for size in [
        10000.0, 8000.0, 6000.0, 5000.0, 4000.0, 3000.0, 2000.0, 1500.0, 1000.0, 500.0,
    ] {
        spots.push((30.0, size, 4.0));
    }
    // Sample lanes with a mix of very bright and very faint bands.
    spots.push((80.0, 3000.0, 6.0)); // bright, will saturate at long exposure
    spots.push((80.0, 1200.0, 0.4)); // faint, needs long exposure
    spots.push((130.0, 5000.0, 2.0));
    spots.push((130.0, 900.0, 0.2)); // very faint
    spots.push((180.0, 2000.0, 5.0));
    spots.push((180.0, 800.0, 1.0));

    for (x, size, amp) in spots {
        let y0 = size_to_y(size);
        for y in 0..H {
            for xx in 0..W {
                let dx = (xx as f64 - x) / sx;
                let dy = (y as f64 - y0) / sy;
                buf[(y * W + xx) as usize] += amp * (-0.5 * (dx * dx + dy * dy)).exp();
            }
        }
    }
    buf
}

impl Camera for MockCamera {
    fn info(&self) -> &CameraInfo {
        &self.info
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            manual_exposure: true,
            gain: false,
            exposure_range_s: Some((0.001, 2.0)),
        }
    }

    fn set_exposure(&mut self, exposure: Exposure) -> Result<bool> {
        match exposure {
            Exposure::Auto => Ok(false), // mock has no auto mode
            Exposure::Manual(t) => {
                self.exposure_s = t.clamp(0.001, 2.0);
                Ok(true)
            }
        }
    }

    fn current_exposure_s(&self) -> Option<f64> {
        Some(self.exposure_s)
    }

    fn capture(&mut self) -> Result<DynamicImage> {
        let mut img = GrayImage::new(W, H);
        for (i, &r) in self.radiance.iter().enumerate() {
            // Linear sensor: signal = radiance * exposure * K, clipped to 1.0.
            let signal = (r * self.exposure_s * K).min(1.0);
            let v = (signal * 255.0).round() as u8;
            let x = (i as u32) % W;
            let y = (i as u32) / W;
            img.put_pixel(x, y, Luma([v]));
        }
        Ok(DynamicImage::ImageLuma8(img))
    }
}

/// Convenience: a mock capture tagged with meta.
pub fn capture_default() -> Result<(DynamicImage, CaptureMeta)> {
    let mut cam = open(0)?;
    cam.capture_with_meta()
}
