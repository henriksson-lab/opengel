//! Quantification math: sizing from a ladder, intensity→mass calibration, and
//! mass→amount (moles) conversion for DNA / RNA / protein.

use crate::core::model::{Calibration, GelType};

/// Ordinary-least-squares fit of `y = a*x + b`.
///
/// Returns `None` if fewer than two points or zero x-variance.
fn ols(points: &[(f64, f64)]) -> Option<(f64, f64)> {
    let n = points.len();
    if n < 2 {
        return None;
    }
    let nf = n as f64;
    let sx: f64 = points.iter().map(|p| p.0).sum();
    let sy: f64 = points.iter().map(|p| p.1).sum();
    let sxx: f64 = points.iter().map(|p| p.0 * p.0).sum();
    let sxy: f64 = points.iter().map(|p| p.0 * p.1).sum();
    let denom = nf * sxx - sx * sx;
    if denom.abs() < 1e-12 {
        return None;
    }
    let a = (nf * sxy - sx * sy) / denom;
    let b = (sy - a * sx) / nf;
    Some((a, b))
}

/// Semi-log migration model: `ln(size) = a * position + b`.
///
/// This is the standard gel sizing relationship — log of fragment size is
/// approximately linear in migration distance over the ladder's range.
#[derive(Debug, Clone, Copy)]
pub struct SizingFit {
    pub a: f64,
    pub b: f64,
}

impl SizingFit {
    /// Fit from `(position, size)` pairs (position in pixels, size in bp/nt/Da).
    pub fn fit(points: &[(f64, f64)]) -> Option<Self> {
        let logged: Vec<(f64, f64)> = points
            .iter()
            .filter(|p| p.1 > 0.0)
            .map(|p| (p.0, p.1.ln()))
            .collect();
        let (a, b) = ols(&logged)?;
        Some(SizingFit { a, b })
    }

    /// Estimate size at a migration position.
    pub fn size_at(&self, position: f64) -> f64 {
        (self.a * position + self.b).exp()
    }

    /// Inverse: migration position for a given size (a != 0).
    pub fn position_at(&self, size: f64) -> Option<f64> {
        if self.a.abs() < 1e-12 || size <= 0.0 {
            return None;
        }
        Some((size.ln() - self.b) / self.a)
    }
}

impl Calibration {
    /// Fit `mass = slope * density` through the origin from `(density, mass)`.
    pub fn fit_linear(points: &[(f64, f64)]) -> Option<Calibration> {
        let sdd: f64 = points.iter().map(|p| p.0 * p.0).sum();
        let sdm: f64 = points.iter().map(|p| p.0 * p.1).sum();
        if sdd.abs() < 1e-12 {
            return None;
        }
        Some(Calibration::Linear { slope: sdm / sdd })
    }

    /// Fit affine `mass = a*density + b`.
    pub fn fit_affine(points: &[(f64, f64)]) -> Option<Calibration> {
        let (a, b) = ols(points)?;
        Some(Calibration::Affine { a, b })
    }

    /// Fit power law `log(mass) = a*log(density) + b` (handles saturation).
    pub fn fit_loglog(points: &[(f64, f64)]) -> Option<Calibration> {
        let logged: Vec<(f64, f64)> = points
            .iter()
            .filter(|p| p.0 > 0.0 && p.1 > 0.0)
            .map(|p| (p.0.ln(), p.1.ln()))
            .collect();
        let (a, b) = ols(&logged)?;
        Some(Calibration::LogLog { a, b })
    }
}

/// Convert a mass in ng to an amount in nanomoles, given the molecule size and
/// gel type.
///
/// `moles = mass_g / (size * g_per_mol_per_unit)`.
pub fn mass_ng_to_nmol(mass_ng: f64, size: f64, gel_type: GelType) -> Option<f64> {
    let g_per_mol = size * gel_type.g_per_mol_per_size_unit();
    if g_per_mol <= 0.0 {
        return None;
    }
    let mass_g = mass_ng * 1e-9;
    let mol = mass_g / g_per_mol;
    Some(mol * 1e9) // nmol
}

/// Convert an amount in nanomoles and a volume in microliters to a molar
/// concentration (mol/L).
pub fn nmol_to_molar(nmol: f64, volume_ul: f64) -> Option<f64> {
    if volume_ul <= 0.0 {
        return None;
    }
    let mol = nmol * 1e-9;
    let liters = volume_ul * 1e-6;
    Some(mol / liters)
}

/// Relative comparison of two regions by integrated density.
#[derive(Debug, Clone)]
pub struct RelativeResult {
    /// density(a) / density(b).
    pub density_ratio: f64,
    /// Mass of A relative to B (== density_ratio; calibration-free).
    pub mass_ratio: f64,
    /// Molar ratio, if both sizes are known.
    pub molar_ratio: Option<f64>,
}

/// Compare two blobs/bands. `size_*` are the (optional) molecule sizes.
///
/// The mass ratio needs no calibration (density is proportional to mass for a
/// given stain). The molar ratio additionally divides by each size.
pub fn compare(
    density_a: f64,
    size_a: Option<f64>,
    density_b: f64,
    size_b: Option<f64>,
) -> Option<RelativeResult> {
    if density_b.abs() < 1e-12 {
        return None;
    }
    let ratio = density_a / density_b;
    let molar_ratio = match (size_a, size_b) {
        (Some(sa), Some(sb)) if sa > 0.0 && sb > 0.0 => Some((density_a / sa) / (density_b / sb)),
        _ => None,
    };
    Some(RelativeResult {
        density_ratio: ratio,
        mass_ratio: ratio,
        molar_ratio,
    })
}
