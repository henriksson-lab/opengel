//! The gel warp surface: a tensor-product B-spline that maps the *rectified*
//! gel coordinate space `(u, v) ∈ [0,1]²` to image pixel coordinates `(x, y)`.
//!
//! Real gels are not perfect rectangles: lanes bend, bands "smile" (curve at the
//! edges where migration is faster/slower), and off-axis capture adds keystone.
//! Modelling all of that with one smooth surface lets the rest of the pipeline
//! think in a clean rectangle:
//!
//! * `u` — the **cross-lane** axis (left → right). A lane is a `u`-interval.
//! * `v` — the **migration** axis (wells at `v=0` → running front at `v=1`).
//!   A band is an iso-migration curve `S(·, v_center)`; **Rf ≡ v** by construction.
//!
//! The surface is fitted *from* detected lanes/bands (see [`GelWarp::fit_grid`]),
//! so detection always runs first in raw pixel space — the warp is an output of
//! detection, never a prerequisite for it.
//!
//! Non-rational for now (all `weights = 1`), but the rational term is kept in the
//! evaluation so a later keystone/perspective upgrade only has to populate real
//! weights.

use serde::{Deserialize, Serialize};

use crate::core::GrayF32;

/// A tensor-product B-spline surface `S(u, v) → (x, y)` in image pixels.
///
/// Control points are stored row-major with `v` as the outer axis:
/// `ctrl[iv * nu + iu]`. Knot vectors are clamped (repeated end knots) so the
/// surface interpolates its corner control points and `(u, v)` range over
/// `[0, 1]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GelWarp {
    pub degree_u: usize,
    pub degree_v: usize,
    /// Control grid width (count along `u`).
    pub nu: usize,
    /// Control grid height (count along `v`).
    pub nv: usize,
    /// `nu * nv` control points `[x, y]`, row-major (`iv * nu + iu`).
    pub ctrl: Vec<[f64; 2]>,
    /// Per-control-point rational weights (all `1.0` while non-rational).
    pub weights: Vec<f64>,
    /// Clamped knot vector along `u`, length `nu + degree_u + 1`.
    pub knots_u: Vec<f64>,
    /// Clamped knot vector along `v`, length `nv + degree_v + 1`.
    pub knots_v: Vec<f64>,
}

/// A single fit constraint for [`GelWarp::fit`]: the parametric point `(u, v)`
/// should map to the image point `(x, y)`.
#[derive(Debug, Clone, Copy)]
pub struct Anchor {
    pub u: f64,
    pub v: f64,
    pub x: f64,
    pub y: f64,
}

/// Accumulate `scale * (stencil · stencilᵀ)` into `m` — a rank-1 update per
/// 2nd-difference row, used by [`GelWarp::fit`]'s smoothing term.
fn add_stencil(m: &mut [Vec<f64>], idx: &[usize], coeff: &[f64], scale: f64) {
    for a in 0..idx.len() {
        for b in 0..idx.len() {
            m[idx[a]][idx[b]] += scale * coeff[a] * coeff[b];
        }
    }
}

/// Cox–de Boor basis functions of `degree` that are non-zero at parameter `t`,
/// for a clamped knot vector. Returns `(span, values)` where `values[k]` is the
/// basis function of control point `span - degree + k`.
fn basis_functions(t: f64, degree: usize, knots: &[f64], n_ctrl: usize) -> (usize, Vec<f64>) {
    // Locate the knot span index such that knots[span] <= t < knots[span+1],
    // clamped to the valid control-point range.
    let t = t.clamp(0.0, 1.0);
    let mut span = degree;
    while span < n_ctrl - 1 && t >= knots[span + 1] {
        span += 1;
    }

    // Standard triangular basis evaluation (The NURBS Book, A2.2).
    let mut left = vec![0.0; degree + 1];
    let mut right = vec![0.0; degree + 1];
    let mut n = vec![0.0; degree + 1];
    n[0] = 1.0;
    for j in 1..=degree {
        left[j] = t - knots[span + 1 - j];
        right[j] = knots[span + j] - t;
        let mut saved = 0.0;
        for r in 0..j {
            let denom = right[r + 1] + left[j - r];
            let temp = if denom.abs() < 1e-12 { 0.0 } else { n[r] / denom };
            n[r] = saved + right[r + 1] * temp;
            saved = left[j - r] * temp;
        }
        n[j] = saved;
    }
    (span, n)
}

/// The full length-`n_ctrl` basis vector at `t` (mostly zeros), expanding the
/// local [`basis_functions`] result across all control points.
fn full_basis(t: f64, degree: usize, knots: &[f64], n_ctrl: usize) -> Vec<f64> {
    let (span, vals) = basis_functions(t, degree, knots, n_ctrl);
    let mut out = vec![0.0; n_ctrl];
    for (k, val) in vals.iter().enumerate() {
        out[span - degree + k] = *val;
    }
    out
}

/// Solve the dense linear system `a·x = b` by Gaussian elimination with partial
/// pivoting. `a` is `n×n` row-major; both are consumed. Returns `x` (all zeros
/// if singular). `n` is small here (control-grid size), so O(n³) is fine.
pub(crate) fn solve_linear(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Vec<f64> {
    let n = b.len();
    for col in 0..n {
        // Partial pivot.
        let mut piv = col;
        for r in (col + 1)..n {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        if a[piv][col].abs() < 1e-12 {
            continue; // singular column; regularization should prevent this
        }
        a.swap(col, piv);
        b.swap(col, piv);
        // Eliminate below.
        for r in (col + 1)..n {
            let f = a[r][col] / a[col][col];
            if f == 0.0 {
                continue;
            }
            for c in col..n {
                a[r][c] -= f * a[col][c];
            }
            b[r] -= f * b[col];
        }
    }
    // Back-substitute.
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        if a[i][i].abs() < 1e-12 {
            continue;
        }
        let mut s = b[i];
        for c in (i + 1)..n {
            s -= a[i][c] * x[c];
        }
        x[i] = s / a[i][i];
    }
    x
}

/// A clamped, (approximately) uniform knot vector for `n_ctrl` control points
/// of the given `degree`, spanning `[0, 1]`.
fn clamped_knots(n_ctrl: usize, degree: usize) -> Vec<f64> {
    let m = n_ctrl + degree + 1;
    let mut k = vec![0.0; m];
    let interior = n_ctrl - degree; // number of unique interior segments
    for (i, ki) in k.iter_mut().enumerate() {
        *ki = if i <= degree {
            0.0
        } else if i >= n_ctrl {
            1.0
        } else {
            (i - degree) as f64 / interior as f64
        };
    }
    k
}

impl GelWarp {
    /// The identity warp for a `width × height` image: `S(u, v) = (u·W, v·H)`.
    /// This is the degenerate 2×2 bilinear grid — the "naive rectangle" gel
    /// model, and the backward-compatible default when no warp has been fitted.
    pub fn identity(width: u32, height: u32) -> Self {
        let (w, h) = (width as f64, height as f64);
        GelWarp {
            degree_u: 1,
            degree_v: 1,
            nu: 2,
            nv: 2,
            ctrl: vec![[0.0, 0.0], [w, 0.0], [0.0, h], [w, h]],
            weights: vec![1.0; 4],
            knots_u: clamped_knots(2, 1),
            knots_v: clamped_knots(2, 1),
        }
    }

    /// Build a control grid of the requested size from a closure giving the
    /// pixel position of the grid node at normalized `(u, v)`. Degrees default
    /// to cubic, clamped to fit small grids.
    pub fn from_grid(nu: usize, nv: usize, node: impl FnMut(f64, f64) -> [f64; 2]) -> Self {
        let du = 3.min(nu.max(2) - 1);
        let dv = 3.min(nv.max(2) - 1);
        GelWarp::from_grid_with_degree(nu, nv, du, dv, node)
    }

    /// [`from_grid`](Self::from_grid) with explicit degrees. Degree 1 makes the
    /// surface interpolate its nodes exactly (piecewise-bilinear) — the faithful
    /// choice when baking a known distortion into a dense grid.
    pub fn from_grid_with_degree(
        nu: usize,
        nv: usize,
        degree_u: usize,
        degree_v: usize,
        mut node: impl FnMut(f64, f64) -> [f64; 2],
    ) -> Self {
        let nu = nu.max(2);
        let nv = nv.max(2);
        let degree_u = degree_u.clamp(1, nu - 1);
        let degree_v = degree_v.clamp(1, nv - 1);
        let mut ctrl = Vec::with_capacity(nu * nv);
        for iv in 0..nv {
            let v = iv as f64 / (nv - 1) as f64;
            for iu in 0..nu {
                let u = iu as f64 / (nu - 1) as f64;
                ctrl.push(node(u, v));
            }
        }
        GelWarp {
            degree_u,
            degree_v,
            nu,
            nv,
            weights: vec![1.0; nu * nv],
            knots_u: clamped_knots(nu, degree_u),
            knots_v: clamped_knots(nv, degree_v),
            ctrl,
        }
    }

    /// Forward map: rectified `(u, v)` → image pixel `(x, y)`.
    pub fn eval(&self, u: f64, v: f64) -> (f64, f64) {
        let (su, nu_b) = basis_functions(u, self.degree_u, &self.knots_u, self.nu);
        let (sv, nv_b) = basis_functions(v, self.degree_v, &self.knots_v, self.nv);
        let (mut x, mut y, mut wsum) = (0.0, 0.0, 0.0);
        for a in 0..=self.degree_v {
            let iv = sv - self.degree_v + a;
            for b in 0..=self.degree_u {
                let iu = su - self.degree_u + b;
                let idx = iv * self.nu + iu;
                let wgt = self.weights[idx] * nv_b[a] * nu_b[b];
                x += wgt * self.ctrl[idx][0];
                y += wgt * self.ctrl[idx][1];
                wsum += wgt;
            }
        }
        if wsum.abs() < 1e-12 {
            (x, y)
        } else {
            (x / wsum, y / wsum)
        }
    }

    /// Inverse map: image pixel `(x, y)` → rectified `(u, v)`, by Newton descent
    /// on `‖S(u,v) − (x,y)‖²` seeded from a coarse grid search. Returns the
    /// best `(u, v)` clamped to `[0, 1]²` (nearest surface point if `(x,y)` lies
    /// outside the gel).
    pub fn invert(&self, x: f64, y: f64) -> (f64, f64) {
        // Coarse seed.
        let (mut bu, mut bv, mut bd) = (0.5, 0.5, f64::INFINITY);
        let steps = 16;
        for i in 0..=steps {
            let u = i as f64 / steps as f64;
            for j in 0..=steps {
                let v = j as f64 / steps as f64;
                let (sx, sy) = self.eval(u, v);
                let d = (sx - x).powi(2) + (sy - y).powi(2);
                if d < bd {
                    bd = d;
                    bu = u;
                    bv = v;
                }
            }
        }
        // Newton refinement using finite-difference derivatives.
        let eps = 1e-4;
        for _ in 0..12 {
            let (sx, sy) = self.eval(bu, bv);
            let (rx, ry) = (sx - x, sy - y);
            let (sxu, syu) = self.eval((bu + eps).min(1.0), bv);
            let (sxv, syv) = self.eval(bu, (bv + eps).min(1.0));
            let jxu = (sxu - sx) / eps;
            let jyu = (syu - sy) / eps;
            let jxv = (sxv - sx) / eps;
            let jyv = (syv - sy) / eps;
            // Solve J^T J step = -J^T r (Gauss–Newton on the 2×2 system).
            let a = jxu * jxu + jyu * jyu;
            let b = jxu * jxv + jyu * jyv;
            let c = b;
            let d = jxv * jxv + jyv * jyv;
            let g0 = jxu * rx + jyu * ry;
            let g1 = jxv * rx + jyv * ry;
            let det = a * d - b * c;
            if det.abs() < 1e-12 {
                break;
            }
            let du = -(d * g0 - b * g1) / det;
            let dv = -(a * g1 - c * g0) / det;
            bu = (bu + du).clamp(0.0, 1.0);
            bv = (bv + dv).clamp(0.0, 1.0);
            if du.abs() + dv.abs() < 1e-6 {
                break;
            }
        }
        (bu, bv)
    }

    /// Resample `img` through the warp into a rectified raster of size
    /// `out_w × out_h`, where output pixel `(i, j)` samples the source at
    /// `S(i/out_w, j/out_h)`. In the rectified image lanes are vertical and
    /// bands horizontal, so the existing densitometry code (`column_profile`,
    /// `lane_row_profile`, …) applies unchanged.
    pub fn rectify(&self, img: &GrayF32, out_w: usize, out_h: usize) -> GrayF32 {
        let mut out = GrayF32::new(out_w, out_h);
        if out_w == 0 || out_h == 0 {
            return out;
        }
        for j in 0..out_h {
            let v = j as f64 / out_h as f64;
            for i in 0..out_w {
                let u = i as f64 / out_w as f64;
                let (x, y) = self.eval(u, v);
                out.data[[j, i]] = img.sample_bilinear(x as f32, y as f32);
            }
        }
        out
    }

    /// Refine this grid's control points to fit `(u, v) → (x, y)`
    /// correspondences by regularized least squares, keeping the grid size,
    /// knots and degrees fixed. `lambda` Tikhonov-regularizes each control point
    /// *toward its current position*, so points no correspondence constrains
    /// stay put (unconstrained regions keep the prior shape) — this is how smile
    /// is introduced only where matched ladder rungs across lanes pin it.
    pub fn refine_least_squares(&self, corr: &[(f64, f64, f64, f64)], lambda: f64) -> GelWarp {
        let n = self.nu * self.nv;
        let mut a = vec![vec![0.0; n]; n];
        let mut bx = vec![0.0; n];
        let mut by = vec![0.0; n];
        for &(u, v, x, y) in corr {
            let bu = full_basis(u, self.degree_u, &self.knots_u, self.nu);
            let bv = full_basis(v, self.degree_v, &self.knots_v, self.nv);
            // Sparse weight vector w[iv*nu+iu] = bv[iv]·bu[iu]; collect nonzeros.
            let mut nz: Vec<(usize, f64)> = Vec::new();
            for iv in 0..self.nv {
                if bv[iv] == 0.0 {
                    continue;
                }
                for iu in 0..self.nu {
                    let wgt = bv[iv] * bu[iu];
                    if wgt != 0.0 {
                        nz.push((iv * self.nu + iu, wgt));
                    }
                }
            }
            for &(i, wi) in &nz {
                bx[i] += wi * x;
                by[i] += wi * y;
                for &(j, wj) in &nz {
                    a[i][j] += wi * wj;
                }
            }
        }
        // Regularize toward the current control points (the prior).
        for i in 0..n {
            a[i][i] += lambda;
            bx[i] += lambda * self.ctrl[i][0];
            by[i] += lambda * self.ctrl[i][1];
        }
        let cx = solve_linear(a.clone(), bx);
        let cy = solve_linear(a, by);
        let ctrl = (0..n).map(|i| [cx[i], cy[i]]).collect();
        GelWarp {
            ctrl,
            ..self.clone()
        }
    }

    /// Fit a warp grid whose iso-`u` lines follow detected lane centerlines.
    ///
    /// `lane_centers_px` are the x pixel positions of each detected lane center
    /// (ascending). The grid's `u` columns are placed at those centers (so a
    /// lane at pixel `x` maps to a stable `u`), while `v` maps linearly to `y`.
    /// This corrects lane spacing/ordering today; smile (per-column `v` warp)
    /// is layered on later from matched ladder rungs — the correspondence
    /// machinery in [`GelWarp::from_grid`] already supports it.
    /// Uniform identity lattice of size `nu × nv` spanning `[0,w] × [0,h]`
    /// (origin-compatible constructor: `eval(u,v) ≈ (u·w, v·h)`). Used by the
    /// renderer and the GUI warp-grid editor.
    pub fn identity_grid(w: f64, h: f64, nu: usize, nv: usize) -> Self {
        GelWarp::from_grid(nu.max(2), nv.max(2), |u, v| [u * w, v * h])
    }

    /// Grid size as `(nu, nv)`.
    pub fn grid_size(&self) -> (usize, usize) {
        (self.nu, self.nv)
    }

    /// Control point at lattice index `(iu, iv)` as `(x, y)`.
    pub fn control_point(&self, iu: usize, iv: usize) -> (f64, f64) {
        let c = self.ctrl[iv * self.nu + iu];
        (c[0], c[1])
    }

    /// Set the control point at lattice index `(iu, iv)`.
    pub fn set_control_point(&mut self, iu: usize, iv: usize, x: f64, y: f64) {
        self.ctrl[iv * self.nu + iu] = [x, y];
    }

    /// Polyline (`n` points) of the iso-`v` curve (constant migration, sampled
    /// across lanes).
    pub fn iso_v(&self, v: f64, n: usize) -> Vec<(f64, f64)> {
        if n == 0 {
            return Vec::new();
        }
        if n == 1 {
            return vec![self.eval(0.5, v)];
        }
        (0..n).map(|i| self.eval(i as f64 / (n - 1) as f64, v)).collect()
    }

    /// Polyline (`n` points) of the iso-`u` curve (one lane's path down the gel).
    pub fn iso_u(&self, u: f64, n: usize) -> Vec<(f64, f64)> {
        if n == 0 {
            return Vec::new();
        }
        if n == 1 {
            return vec![self.eval(u, 0.5)];
        }
        (0..n).map(|i| self.eval(u, i as f64 / (n - 1) as f64)).collect()
    }

    /// Least-squares fit of an `nu × nv` control lattice to `(u,v)→(x,y)`
    /// [`Anchor`]s, with 2nd-difference smoothing (`lambda`) plus a small pull
    /// toward the identity lattice so the normal matrix stays SPD even when the
    /// anchors under-constrain an axis (e.g. a single lane).
    pub fn fit(anchors: &[Anchor], w: f64, h: f64, nu: usize, nv: usize, lambda: f64) -> Self {
        let base = GelWarp::identity_grid(w, h, nu, nv);
        let (nu, nv) = (base.nu, base.nv);
        let n = nu * nv;
        let mut m = vec![vec![0.0; n]; n];
        let mut bx = vec![0.0; n];
        let mut by = vec![0.0; n];

        // Data term.
        for a in anchors {
            let bu = full_basis(a.u, base.degree_u, &base.knots_u, nu);
            let bv = full_basis(a.v, base.degree_v, &base.knots_v, nv);
            let mut nz: Vec<(usize, f64)> = Vec::new();
            for iv in 0..nv {
                if bv[iv] == 0.0 {
                    continue;
                }
                for iu in 0..nu {
                    let wgt = bv[iv] * bu[iu];
                    if wgt != 0.0 {
                        nz.push((iv * nu + iu, wgt));
                    }
                }
            }
            for &(i, wi) in &nz {
                bx[i] += wi * a.x;
                by[i] += wi * a.y;
                for &(j, wj) in &nz {
                    m[i][j] += wi * wj;
                }
            }
        }

        // 2nd-difference smoothing along u and v.
        if lambda != 0.0 {
            for iv in 0..nv {
                for iu in 1..nu.saturating_sub(1) {
                    let idx = [iv * nu + (iu - 1), iv * nu + iu, iv * nu + (iu + 1)];
                    add_stencil(&mut m, &idx, &[1.0, -2.0, 1.0], lambda);
                }
            }
            for iu in 0..nu {
                for iv in 1..nv.saturating_sub(1) {
                    let idx = [(iv - 1) * nu + iu, iv * nu + iu, (iv + 1) * nu + iu];
                    add_stencil(&mut m, &idx, &[1.0, -2.0, 1.0], lambda);
                }
            }
        }

        // Identity pull (removes any remaining null space).
        let eps = 1e-6;
        for i in 0..n {
            m[i][i] += eps;
            bx[i] += eps * base.ctrl[i][0];
            by[i] += eps * base.ctrl[i][1];
        }

        let cx = solve_linear(m.clone(), bx);
        let cy = solve_linear(m, by);
        let ctrl = (0..n).map(|i| [cx[i], cy[i]]).collect();
        GelWarp { ctrl, ..base }
    }

    pub fn fit_grid(lane_centers_px: &[f64], width: u32, height: u32) -> Self {
        let (w, h) = (width as f64, height as f64);
        if lane_centers_px.len() < 2 {
            return GelWarp::identity(width, height);
        }
        // Build interior columns at the lane centers, plus the two gel edges so
        // the surface spans the full image width.
        let mut xs = Vec::with_capacity(lane_centers_px.len() + 2);
        xs.push(0.0);
        xs.extend_from_slice(lane_centers_px);
        xs.push(w);
        let nu = xs.len();
        GelWarp::from_grid(nu, 2, |u, v| {
            // Piecewise-linear placement of x along the (non-uniform) columns.
            let f = u * (nu - 1) as f64;
            let i0 = (f.floor() as usize).min(nu - 1);
            let i1 = (i0 + 1).min(nu - 1);
            let frac = f - i0 as f64;
            let x = xs[i0] + (xs[i1] - xs[i0]) * frac;
            [x, v * h]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_round_trips() {
        let warp = GelWarp::identity(200, 300);
        let (x, y) = warp.eval(0.25, 0.5);
        assert!((x - 50.0).abs() < 1e-6);
        assert!((y - 150.0).abs() < 1e-6);
        let (u, v) = warp.invert(50.0, 150.0);
        assert!((u - 0.25).abs() < 1e-3, "u={u}");
        assert!((v - 0.5).abs() < 1e-3, "v={v}");
    }

    #[test]
    fn refine_recovers_smile() {
        // Three lanes at x = 40, 100, 160 in a 200×300 image. One iso-migration
        // front smiles: the edge lanes run to y=170 while the center is at 150.
        let (w, h) = (200u32, 300u32);
        let lane_x = [40.0, 100.0, 160.0];
        let xs = [0.0, lane_x[0], lane_x[1], lane_x[2], w as f64];
        let nu = xs.len();
        let prior = GelWarp::from_grid(nu, 3, |u, v| {
            let f = u * (nu - 1) as f64;
            let i0 = (f.floor() as usize).min(nu - 1);
            let i1 = (i0 + 1).min(nu - 1);
            let frac = f - i0 as f64;
            [xs[i0] + (xs[i1] - xs[i0]) * frac, v * h as f64]
        });
        // Front points (u, v, x, y); v is the shared mean migration.
        let ys = [170.0, 150.0, 170.0];
        let v = (ys[0] + ys[1] + ys[2]) / 3.0 / h as f64;
        let us = [1.0 / 4.0, 2.0 / 4.0, 3.0 / 4.0];
        let corr: Vec<(f64, f64, f64, f64)> = (0..3)
            .map(|i| (us[i], v, lane_x[i], ys[i]))
            .collect();
        let warp = prior.refine_least_squares(&corr, 1e-3);

        // The surface reproduces the smile at the front's v.
        for i in 0..3 {
            let (x, y) = warp.eval(us[i], v);
            assert!((x - lane_x[i]).abs() < 3.0, "x[{i}]={x}");
            assert!((y - ys[i]).abs() < 3.0, "y[{i}]={y}");
        }
        // All three smiled points invert to (nearly) the same migration v —
        // i.e. the smile is removed in rectified coordinates.
        let vs: Vec<f64> = (0..3).map(|i| warp.invert(lane_x[i], ys[i]).1).collect();
        let spread = vs.iter().cloned().fold(f64::MIN, f64::max)
            - vs.iter().cloned().fold(f64::MAX, f64::min);
        assert!(spread < 0.02, "v spread after rectification = {spread}");
    }

    #[test]
    fn fit_grid_maps_lane_centers() {
        let warp = GelWarp::fit_grid(&[40.0, 100.0, 160.0], 200, 300);
        // A point at a lane center inverts to a stable interior u and back.
        let (u, _) = warp.invert(100.0, 150.0);
        let (x, _) = warp.eval(u, 0.5);
        assert!((x - 100.0).abs() < 1.0, "x={x}");
    }
}
