//! A simple single-channel `f32` image used as the working representation for
//! detection, HDR merging and densitometry.
//!
//! Pixel values are normalized luminance in `[0, 1]` for 8/16-bit inputs; HDR
//! radiance images may exceed `1.0`.

use image::DynamicImage;
use ndarray::Array2;

/// Grayscale floating-point image. Indexed `[y, x]` (row-major: rows = height).
#[derive(Debug, Clone)]
pub struct GrayF32 {
    /// `[height, width]` array of luminance/radiance values.
    pub data: Array2<f32>,
}

impl GrayF32 {
    pub fn new(width: usize, height: usize) -> Self {
        GrayF32 {
            data: Array2::zeros((height, width)),
        }
    }

    pub fn width(&self) -> usize {
        self.data.ncols()
    }
    pub fn height(&self) -> usize {
        self.data.nrows()
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> f32 {
        self.data[[y, x]]
    }

    /// Convert a decoded image to normalized grayscale.
    ///
    /// 16-bit inputs divide by 65535, 8-bit by 255. RGB is converted to luma.
    pub fn from_dynamic(img: &DynamicImage) -> Self {
        let (w, h) = (img.width() as usize, img.height() as usize);
        // Fill from the contiguous (row-major) luma buffer in one tight pass.
        // Per-element `Array2[[y, x]]` indexing here is ~100× slower and was a
        // real bottleneck for live preview frames.
        let flat: Vec<f32> = match img {
            DynamicImage::ImageLuma16(buf) => {
                buf.as_raw().iter().map(|&v| v as f32 / 65535.0).collect()
            }
            other => other
                .to_luma8()
                .as_raw()
                .iter()
                .map(|&b| b as f32 / 255.0)
                .collect(),
        };
        let data =
            Array2::from_shape_vec((h, w), flat).unwrap_or_else(|_| Array2::<f32>::zeros((h, w)));
        GrayF32 { data }
    }

    /// Invert intensity (`1 - v`, clamped at 0). Useful to turn dark-band-on-
    /// light-background images into "signal = bright" for densitometry.
    pub fn inverted(&self) -> Self {
        GrayF32 {
            data: self.data.mapv(|v| (1.0 - v).max(0.0)),
        }
    }

    /// Bilinear sample at floating-point `(x, y)`; out-of-bounds reads as 0.
    pub fn sample_bilinear(&self, x: f32, y: f32) -> f32 {
        let (w, h) = (self.width(), self.height());
        if w == 0 || h == 0 || x < 0.0 || y < 0.0 {
            return 0.0;
        }
        let x0 = x.floor() as isize;
        let y0 = y.floor() as isize;
        let x1 = x0 + 1;
        let y1 = y0 + 1;
        if x0 < 0 || y0 < 0 || x1 >= w as isize || y1 >= h as isize {
            // Fall back to nearest valid pixel for the border.
            let xi = (x.round() as isize).clamp(0, w as isize - 1) as usize;
            let yi = (y.round() as isize).clamp(0, h as isize - 1) as usize;
            return self.get(xi, yi);
        }
        let fx = x - x0 as f32;
        let fy = y - y0 as f32;
        let (x0, y0, x1, y1) = (x0 as usize, y0 as usize, x1 as usize, y1 as usize);
        let a = self.get(x0, y0);
        let b = self.get(x1, y0);
        let c = self.get(x0, y1);
        let d = self.get(x1, y1);
        let top = a + (b - a) * fx;
        let bot = c + (d - c) * fx;
        top + (bot - top) * fy
    }

    /// Rotate the image about its center by `angle_deg` (counter-clockwise),
    /// keeping the same dimensions (content may clip at corners). Uses inverse
    /// bilinear sampling. Areas outside the source read as 0.
    pub fn rotated(&self, angle_deg: f64) -> GrayF32 {
        let (w, h) = (self.width(), self.height());
        let mut out = Array2::<f32>::zeros((h, w));
        if w == 0 || h == 0 {
            return GrayF32 { data: out };
        }
        let cx = (w as f64 - 1.0) / 2.0;
        let cy = (h as f64 - 1.0) / 2.0;
        let rad = angle_deg.to_radians();
        // Inverse rotation maps output pixel -> source pixel.
        let (c, s) = (rad.cos(), rad.sin());
        for y in 0..h {
            for x in 0..w {
                let dx = x as f64 - cx;
                let dy = y as f64 - cy;
                let sx = cx + dx * c + dy * s;
                let sy = cy - dx * s + dy * c;
                out[[y, x]] = self.sample_bilinear(sx as f32, sy as f32);
            }
        }
        GrayF32 { data: out }
    }

    /// Min and max pixel values, for display window/level.
    pub fn min_max(&self) -> (f32, f32) {
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for &v in self.data.iter() {
            if v < lo {
                lo = v;
            }
            if v > hi {
                hi = v;
            }
        }
        if !lo.is_finite() {
            (0.0, 1.0)
        } else {
            (lo, hi)
        }
    }
}
