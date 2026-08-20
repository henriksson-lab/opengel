//! Synthetic camera backend.
//!
//! Renders a gel-like scene whose captured brightness scales with the exposure
//! time (with clipping at the top and a floor at the bottom), so exposure
//! bracketing and HDR merge behave like a real linear sensor.
//!
//! **The scene follows the light.** When the simulated enclosure is driving the
//! bench (see [`crate::simbench`]) the camera photographs whichever light source
//! is on: a nucleic-acid stain under UV, the same gel much weaker under blue
//! light, a bright transilluminated field with absorbing bands under white
//! light, protein bands under stain-free — and near-darkness with the lamps off.
//! A darkroom whose picture never changes when the lamps do would make the
//! whole channel model untestable without hardware.

use crate::core::model::CaptureMeta;
use crate::instrument::TrayType;
use crate::simbench::BenchLight;
use image::{DynamicImage, GrayImage, Luma};

use crate::camera::{Camera, CameraError, CameraInfo, Capabilities, Exposure, Result};

const W: u32 = 220;
const H: u32 = 300;
/// Sensor gain mapping scene radiance × seconds → normalized signal.
const K: f64 = 3.0;
/// Signal (per second of exposure) with no light on the gel. Not zero: a real
/// sensor still accumulates, and a pure-black frame would hide the difference
/// between "lamps off" and "camera broken". Applied only in the dark — a
/// fluorescence image *is* a dark field, and giving it a floor would put a step
/// at the frame edge that the auto-straighten sharpness measure would chase.
const DARK_RADIANCE: f64 = 0.02;

pub struct MockCamera {
    info: CameraInfo,
    exposure_s: f64,
    /// The scene last rendered, and the light it was rendered for. Building a
    /// radiance field is a few million exp() calls, so it is kept until the
    /// light changes rather than rebuilt per frame.
    scene: Scene,
}

struct Scene {
    light: BenchLight,
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
        scene: Scene::for_light(crate::simbench::light()),
    })
}

fn size_to_y(size: f64) -> f64 {
    let (ln_hi, ln_lo) = (10000f64.ln(), 500f64.ln());
    let slope = (280.0 - 20.0) / (ln_lo - ln_hi);
    20.0 + (size.ln() - ln_hi) * slope
}

/// One band: lane centre x, apparent size (drives its y), peak radiance.
type Spot = (f64, f64, f64);

/// The bands a light source reveals.
///
/// The light sources are not filtered views of one image — a different dye
/// fluoresces (or absorbs) under each, so each shows its own bands. Under UV a
/// nucleic-acid stain shows the full ladder and every sample band; blue light
/// excites the same stain far more weakly, so the faintest bands drop into the
/// noise; stain-free imaging shows the *proteins*, which are different molecules
/// at different positions; white light is transmitted through the gel and the
/// bands absorb it.
fn spots_for(light: TrayType) -> Vec<Spot> {
    let mut spots: Vec<Spot> = Vec::new();
    match light {
        // Nucleic-acid stain under 302 nm: the reference scene, spanning a wide
        // radiance range so dynamic range is meaningful.
        TrayType::Uv => {
            for size in [
                10000.0, 8000.0, 6000.0, 5000.0, 4000.0, 3000.0, 2000.0, 1500.0, 1000.0, 500.0,
            ] {
                spots.push((30.0, size, 4.0));
            }
            spots.push((80.0, 3000.0, 6.0)); // bright, saturates at long exposure
            spots.push((80.0, 1200.0, 0.4)); // faint, needs a long exposure
            spots.push((130.0, 5000.0, 2.0));
            spots.push((130.0, 900.0, 0.2)); // very faint
            spots.push((180.0, 2000.0, 5.0));
            spots.push((180.0, 800.0, 1.0));
        }
        // Same gel, same stain, much weaker excitation: the ladder and the
        // strong bands survive, the faint ones do not.
        TrayType::Blue => {
            for size in [10000.0, 6000.0, 4000.0, 2000.0, 1000.0] {
                spots.push((30.0, size, 0.9));
            }
            spots.push((80.0, 3000.0, 1.4));
            spots.push((130.0, 5000.0, 0.5));
            spots.push((180.0, 2000.0, 1.1));
        }
        // Stain-free: the proteins, which are other molecules at other places.
        TrayType::StainFree => {
            for size in [9000.0, 5000.0, 2500.0, 1100.0] {
                spots.push((30.0, size, 2.5));
            }
            spots.push((105.0, 7000.0, 3.0));
            spots.push((105.0, 1600.0, 1.2));
            spots.push((155.0, 4200.0, 2.2));
            spots.push((155.0, 700.0, 0.6));
        }
        // White light is transmitted through the gel; the bands absorb it, so
        // they are rendered as negative radiance against a bright field.
        TrayType::White => {
            spots.push((30.0, 8000.0, -0.16));
            spots.push((30.0, 3000.0, -0.16));
            spots.push((30.0, 1000.0, -0.16));
            spots.push((95.0, 6000.0, -0.22));
            spots.push((95.0, 1400.0, -0.10));
            spots.push((165.0, 3500.0, -0.26));
        }
    }
    spots
}

/// The uniform field a light source puts on the sensor before any band.
/// Fluorescence is imaged against darkness; white light is a bright field the
/// bands are seen against.
fn background_for(light: TrayType) -> f64 {
    match light {
        TrayType::White => 0.30,
        _ => 0.0,
    }
}

impl Scene {
    fn for_light(light: BenchLight) -> Self {
        let (sx, sy) = (5.0f64, 3.5f64);
        // With nobody driving the bench the mock stands in for a plain camera,
        // and shows the nucleic-acid scene rather than an unexplained black
        // frame.
        let shown = match light {
            BenchLight::Unset => Some(TrayType::Uv),
            BenchLight::Dark => None,
            BenchLight::Lit(tray) => Some(tray),
        };
        let background = shown.map(background_for).unwrap_or(DARK_RADIANCE);
        let mut radiance = vec![background; (W * H) as usize];
        if let Some(light) = shown {
            for (x, size, amp) in spots_for(light) {
                let y0 = size_to_y(size);
                for y in 0..H {
                    for xx in 0..W {
                        let dx = (xx as f64 - x) / sx;
                        let dy = (y as f64 - y0) / sy;
                        radiance[(y * W + xx) as usize] += amp * (-0.5 * (dx * dx + dy * dy)).exp();
                    }
                }
            }
        }
        // An absorbing band cannot take away more light than there is.
        for r in &mut radiance {
            *r = r.max(0.0);
        }
        Scene { light, radiance }
    }
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
        // Follow the lamps: the enclosure may have been switched to another tray
        // (or switched off entirely) since the last frame.
        let light = crate::simbench::light();
        if light != self.scene.light {
            self.scene = Scene::for_light(light);
        }
        let mut img = GrayImage::new(W, H);
        for (i, &r) in self.scene.radiance.iter().enumerate() {
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
