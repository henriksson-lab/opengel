//! # warp
//!
//! A gel-warp model: a tensor-product cubic B-spline surface (a NURBS with unit
//! weights) `S(u,v) -> (x, y)` mapping the parametric unit square onto
//! image-pixel coordinates.
//!
//! The axes are aligned with the gel:
//!
//! * `u ∈ [0,1]` runs along the **lane / horizontal** axis, and
//! * `v ∈ [0,1]` runs along the **migration / vertical** axis.
//!
//! Clamped (open-uniform) knot vectors are used, so the surface interpolates the
//! four corner control points. The per-axis polynomial degree is
//! `min(3, n - 1)`, so a grid size of 2 gives a linear axis, 3 a quadratic axis
//! and >= 4 a cubic axis. Control points are stored as a row-major `nv × nu`
//! grid of `(x, y)` pairs (row index is `v`, column index is `u`).

/// A single fit constraint: the parametric point `(u, v)` should map to the
/// image point `(x, y)`.
#[derive(Debug, Clone, Copy)]
pub struct Anchor {
    pub u: f64,
    pub v: f64,
    pub x: f64,
    pub y: f64,
}

/// A tensor-product cubic B-spline warp surface.
#[derive(Debug, Clone)]
pub struct Warp {
    /// Number of control points along the lane (u) axis.
    nu: usize,
    /// Number of control points along the migration (v) axis.
    nv: usize,
    /// Polynomial degree along u, `min(3, nu - 1)`.
    du: usize,
    /// Polynomial degree along v, `min(3, nv - 1)`.
    dv: usize,
    /// Clamped knot vector along u, length `nu + du + 1`.
    ku: Vec<f64>,
    /// Clamped knot vector along v, length `nv + dv + 1`.
    kv: Vec<f64>,
    /// Row-major `nv × nu` control lattice; index `iv * nu + iu`.
    cp: Vec<(f64, f64)>,
}

// ---------------------------------------------------------------------------
// B-spline basis (Cox-de Boor) helpers
// ---------------------------------------------------------------------------

/// Build a clamped / open-uniform knot vector for `n` control points of degree
/// `p`. The first and last `p + 1` knots are `0` and `1` respectively, with the
/// remaining interior knots spaced uniformly on `(0, 1)`.
fn make_knots(n: usize, p: usize) -> Vec<f64> {
    let m = n + p + 1;
    let mut k = vec![0.0_f64; m];
    let num_interior = if n > p + 1 { n - p - 1 } else { 0 };
    for (i, ki) in k.iter_mut().enumerate() {
        if i <= p {
            *ki = 0.0;
        } else if i >= n {
            *ki = 1.0;
        } else {
            // interior index: i in (p, n), map to j in 1..=num_interior
            let j = i - p;
            *ki = j as f64 / (num_interior as f64 + 1.0);
        }
    }
    k
}

/// Find the knot span index `i` such that `knots[i] <= u < knots[i+1]`
/// (with the usual clamping at the right endpoint). `n_last` is the largest
/// control-point index (`n - 1`).
fn find_span(n_last: usize, p: usize, u: f64, knots: &[f64]) -> usize {
    if u >= knots[n_last + 1] {
        return n_last;
    }
    if u <= knots[p] {
        return p;
    }
    let mut low = p;
    let mut high = n_last + 1;
    let mut mid = (low + high) / 2;
    while u < knots[mid] || u >= knots[mid + 1] {
        if u < knots[mid] {
            high = mid;
        } else {
            low = mid;
        }
        mid = (low + high) / 2;
    }
    mid
}

/// The `p + 1` non-vanishing basis functions at span `span` (NURBS book A2.2).
fn basis_funs(span: usize, u: f64, p: usize, knots: &[f64]) -> Vec<f64> {
    let mut nb = vec![0.0_f64; p + 1];
    let mut left = vec![0.0_f64; p + 1];
    let mut right = vec![0.0_f64; p + 1];
    nb[0] = 1.0;
    for j in 1..=p {
        left[j] = u - knots[span + 1 - j];
        right[j] = knots[span + j] - u;
        let mut saved = 0.0;
        for r in 0..j {
            let denom = right[r + 1] + left[j - r];
            let temp = if denom != 0.0 { nb[r] / denom } else { 0.0 };
            nb[r] = saved + right[r + 1] * temp;
            saved = left[j - r] * temp;
        }
        nb[j] = saved;
    }
    nb
}

/// Basis functions and their derivatives up to order `n_deriv` at `span`
/// (NURBS book A2.3). Returns `ders[k][j]` for derivative order `k` and local
/// basis index `j` (`0..=p`).
fn ders_basis_funs(span: usize, u: f64, p: usize, n_deriv: usize, knots: &[f64]) -> Vec<Vec<f64>> {
    let mut ndu = vec![vec![0.0_f64; p + 1]; p + 1];
    let mut left = vec![0.0_f64; p + 1];
    let mut right = vec![0.0_f64; p + 1];
    ndu[0][0] = 1.0;
    for j in 1..=p {
        left[j] = u - knots[span + 1 - j];
        right[j] = knots[span + j] - u;
        let mut saved = 0.0;
        for r in 0..j {
            let denom = right[r + 1] + left[j - r];
            ndu[j][r] = denom;
            let temp = if denom != 0.0 { ndu[r][j - 1] / denom } else { 0.0 };
            ndu[r][j] = saved + right[r + 1] * temp;
            saved = left[j - r] * temp;
        }
        ndu[j][j] = saved;
    }

    let mut ders = vec![vec![0.0_f64; p + 1]; n_deriv + 1];
    for j in 0..=p {
        ders[0][j] = ndu[j][p];
    }

    // `a` holds the two most recent rows of the coefficient table.
    let mut a = vec![vec![0.0_f64; p + 1]; 2];
    for r in 0..=p {
        let mut s1 = 0usize;
        let mut s2 = 1usize;
        a[0][0] = 1.0;
        for k in 1..=n_deriv {
            let mut d = 0.0_f64;
            let rk = r as isize - k as isize;
            let pk = p as isize - k as isize;
            if r >= k {
                a[s2][0] = a[s1][0] / ndu[(pk + 1) as usize][rk as usize];
                d = a[s2][0] * ndu[rk as usize][pk as usize];
            }
            let j1 = if rk >= -1 { 1 } else { (-rk) as usize };
            let j2 = if (r as isize - 1) <= pk { k - 1 } else { p - r };
            for j in j1..=j2 {
                a[s2][j] = (a[s1][j] - a[s1][j - 1])
                    / ndu[(pk + 1) as usize][(rk + j as isize) as usize];
                d += a[s2][j] * ndu[(rk + j as isize) as usize][pk as usize];
            }
            if (r as isize) <= pk {
                a[s2][k] = -a[s1][k - 1] / ndu[(pk + 1) as usize][r];
                d += a[s2][k] * ndu[r][pk as usize];
            }
            ders[k][r] = d;
            std::mem::swap(&mut s1, &mut s2);
        }
    }

    // Multiply through by the falling-factorial correction factors.
    let mut fac = p;
    for k in 1..=n_deriv {
        for j in 0..=p {
            ders[k][j] *= fac as f64;
        }
        fac = fac.saturating_mul(p.saturating_sub(k));
    }
    ders
}

/// Full-length (`n`) vector of basis values at `t`.
fn basis_all(t: f64, n: usize, p: usize, knots: &[f64]) -> Vec<f64> {
    let mut out = vec![0.0_f64; n];
    if n == 0 {
        return out;
    }
    let tc = t.clamp(0.0, 1.0);
    let span = find_span(n - 1, p, tc, knots);
    let nb = basis_funs(span, tc, p, knots);
    for (j, &val) in nb.iter().enumerate() {
        out[span - p + j] = val;
    }
    out
}

/// Full-length (`n`) vector of first-derivative basis values at `t`.
fn deriv_all(t: f64, n: usize, p: usize, knots: &[f64]) -> Vec<f64> {
    let mut out = vec![0.0_f64; n];
    if n == 0 || p == 0 {
        return out;
    }
    let tc = t.clamp(0.0, 1.0);
    let span = find_span(n - 1, p, tc, knots);
    let ders = ders_basis_funs(span, tc, p, 1, knots);
    for j in 0..=p {
        out[span - p + j] = ders[1][j];
    }
    out
}

// ---------------------------------------------------------------------------
// Small dense linear algebra (no external deps)
// ---------------------------------------------------------------------------

/// Cholesky factorization `M = L Lᵀ` returning the lower-triangular `L`, or
/// `None` if `M` is not (numerically) positive definite.
fn cholesky(m: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    let mut l = vec![vec![0.0_f64; n]; n];
    for i in 0..n {
        for j in 0..=i {
            let mut sum = m[i][j];
            for k in 0..j {
                sum -= l[i][k] * l[j][k];
            }
            if i == j {
                if sum <= 0.0 {
                    return None;
                }
                l[i][j] = sum.sqrt();
            } else {
                l[i][j] = sum / l[j][j];
            }
        }
    }
    Some(l)
}

/// Solve `L Lᵀ x = b` given the Cholesky factor `L`.
fn chol_solve(l: &[Vec<f64>], b: &[f64]) -> Vec<f64> {
    let n = l.len();
    // Forward: L y = b
    let mut y = vec![0.0_f64; n];
    for i in 0..n {
        let mut sum = b[i];
        for k in 0..i {
            sum -= l[i][k] * y[k];
        }
        y[i] = sum / l[i][i];
    }
    // Backward: Lᵀ x = y
    let mut x = vec![0.0_f64; n];
    for i in (0..n).rev() {
        let mut sum = y[i];
        for k in (i + 1)..n {
            sum -= l[k][i] * x[k];
        }
        x[i] = sum / l[i][i];
    }
    x
}

/// Gaussian elimination with partial pivoting (robust fallback).
fn gauss_solve(m: &[Vec<f64>], b: &[f64]) -> Vec<f64> {
    let n = m.len();
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut rhs = b.to_vec();
    for col in 0..n {
        // pivot
        let mut piv = col;
        let mut best = a[col][col].abs();
        for r in (col + 1)..n {
            if a[r][col].abs() > best {
                best = a[r][col].abs();
                piv = r;
            }
        }
        if piv != col {
            a.swap(col, piv);
            rhs.swap(col, piv);
        }
        let d = a[col][col];
        if d.abs() < 1e-300 {
            continue;
        }
        for r in (col + 1)..n {
            let f = a[r][col] / d;
            if f == 0.0 {
                continue;
            }
            for c in col..n {
                a[r][c] -= f * a[col][c];
            }
            rhs[r] -= f * rhs[col];
        }
    }
    let mut x = vec![0.0_f64; n];
    for i in (0..n).rev() {
        let mut sum = rhs[i];
        for c in (i + 1)..n {
            sum -= a[i][c] * x[c];
        }
        let d = a[i][i];
        x[i] = if d.abs() < 1e-300 { 0.0 } else { sum / d };
    }
    x
}

/// Solve the SPD system `M x = b` for each right-hand side, preferring Cholesky
/// and falling back to Gaussian elimination if factorization fails.
fn solve_spd(m: &[Vec<f64>], rhs: &[Vec<f64>]) -> Vec<Vec<f64>> {
    if let Some(l) = cholesky(m) {
        rhs.iter().map(|b| chol_solve(&l, b)).collect()
    } else {
        rhs.iter().map(|b| gauss_solve(m, b)).collect()
    }
}

// ---------------------------------------------------------------------------
// Warp
// ---------------------------------------------------------------------------

impl Warp {
    /// Identity warp: an `nu × nv` control lattice regularly spaced over
    /// `[0,w] × [0,h]`. `eval(u,v)` then returns `(u*w, v*h)` up to spline
    /// reproduction of the lattice (exact at the corners and, for `nu = nv = 4`,
    /// at the midpoint as well).
    pub fn identity(w: f64, h: f64, nu: usize, nv: usize) -> Warp {
        let nu = nu.max(1);
        let nv = nv.max(1);
        let du = 3.min(nu.saturating_sub(1));
        let dv = 3.min(nv.saturating_sub(1));
        let ku = make_knots(nu, du);
        let kv = make_knots(nv, dv);
        let denom_u = (nu.saturating_sub(1)).max(1) as f64;
        let denom_v = (nv.saturating_sub(1)).max(1) as f64;
        let mut cp = Vec::with_capacity(nu * nv);
        for iv in 0..nv {
            let y = iv as f64 / denom_v * h;
            for iu in 0..nu {
                let x = iu as f64 / denom_u * w;
                cp.push((x, y));
            }
        }
        Warp {
            nu,
            nv,
            du,
            dv,
            ku,
            kv,
            cp,
        }
    }

    /// Grid size as `(nu, nv)`.
    pub fn grid_size(&self) -> (usize, usize) {
        (self.nu, self.nv)
    }

    /// Control point at lattice index `(iu, iv)`.
    pub fn control_point(&self, iu: usize, iv: usize) -> (f64, f64) {
        self.cp[iv * self.nu + iu]
    }

    /// Set the control point at lattice index `(iu, iv)`.
    pub fn set_control_point(&mut self, iu: usize, iv: usize, x: f64, y: f64) {
        self.cp[iv * self.nu + iu] = (x, y);
    }

    /// Surface point `S(u,v)`; `u, v` are clamped to `[0,1]`.
    pub fn eval(&self, u: f64, v: f64) -> (f64, f64) {
        let bu = basis_all(u, self.nu, self.du, &self.ku);
        let bv = basis_all(v, self.nv, self.dv, &self.kv);
        self.combine(&bu, &bv)
    }

    /// Partial derivative `∂S/∂u` (image-space tangent along lanes).
    pub fn deriv_u(&self, u: f64, v: f64) -> (f64, f64) {
        let bu = deriv_all(u, self.nu, self.du, &self.ku);
        let bv = basis_all(v, self.nv, self.dv, &self.kv);
        self.combine(&bu, &bv)
    }

    /// Partial derivative `∂S/∂v` (image-space tangent along migration).
    pub fn deriv_v(&self, u: f64, v: f64) -> (f64, f64) {
        let bu = basis_all(u, self.nu, self.du, &self.ku);
        let bv = deriv_all(v, self.nv, self.dv, &self.kv);
        self.combine(&bu, &bv)
    }

    /// Contract the tensor-product basis vectors against the control lattice.
    fn combine(&self, bu: &[f64], bv: &[f64]) -> (f64, f64) {
        let mut x = 0.0_f64;
        let mut y = 0.0_f64;
        for iv in 0..self.nv {
            let wv = bv[iv];
            if wv == 0.0 {
                continue;
            }
            let row = iv * self.nu;
            for iu in 0..self.nu {
                let w = wv * bu[iu];
                if w == 0.0 {
                    continue;
                }
                let (px, py) = self.cp[row + iu];
                x += w * px;
                y += w * py;
            }
        }
        (x, y)
    }

    /// Invert an image point to `(u, v)` via Newton iteration on the `2×2`
    /// Jacobian `[∂S/∂u, ∂S/∂v]`, seeded from a coarse grid search and clamped
    /// to `[0,1]²`. Falls back to the nearest coarse sample if Newton stalls or
    /// the Jacobian is singular.
    pub fn invert(&self, x: f64, y: f64) -> (f64, f64) {
        // Coarse grid seed.
        const G: usize = 16;
        let mut best_u = 0.0_f64;
        let mut best_v = 0.0_f64;
        let mut best_d = f64::INFINITY;
        for i in 0..=G {
            let uu = i as f64 / G as f64;
            for j in 0..=G {
                let vv = j as f64 / G as f64;
                let (px, py) = self.eval(uu, vv);
                let d = (px - x) * (px - x) + (py - y) * (py - y);
                if d < best_d {
                    best_d = d;
                    best_u = uu;
                    best_v = vv;
                }
            }
        }

        let mut u = best_u;
        let mut v = best_v;
        for _ in 0..60 {
            let (px, py) = self.eval(u, v);
            let rx = x - px;
            let ry = y - py;
            if rx * rx + ry * ry < 1e-18 {
                break;
            }
            let (jux, juy) = self.deriv_u(u, v);
            let (jvx, jvy) = self.deriv_v(u, v);
            let det = jux * jvy - jvx * juy;
            if det.abs() < 1e-14 {
                break;
            }
            let du = (rx * jvy - jvx * ry) / det;
            let dv = (jux * ry - juy * rx) / det;
            u = (u + du).clamp(0.0, 1.0);
            v = (v + dv).clamp(0.0, 1.0);
        }

        // Guard against Newton making things worse than the coarse seed.
        let (px, py) = self.eval(u, v);
        let dn = (px - x) * (px - x) + (py - y) * (py - y);
        if dn > best_d {
            return (best_u, best_v);
        }
        (u, v)
    }

    /// Least-squares fit of the control points to `anchors`, on a grid
    /// `nu × nv` spanning `[0,w] × [0,h]`, with Tikhonov + 2nd-difference
    /// smoothing of weight `lambda`. The smoothing plus a small pull toward the
    /// identity lattice keep the normal matrix SPD, so the solve is well-posed
    /// (unique, no panics) even for few or degenerate anchors — e.g. a single
    /// lane leaving the u-axis under-constrained.
    pub fn fit(anchors: &[Anchor], w: f64, h: f64, nu: usize, nv: usize, lambda: f64) -> Warp {
        // Start from the identity lattice — it defines both the spline
        // structure (degrees, knots) and the identity-pull target.
        let base = Warp::identity(w, h, nu, nv);
        let (nu, nv) = (base.nu, base.nv);
        let n = nu * nv;

        // Normal matrix M and the two RHS vectors (x and y coordinates).
        let mut m = vec![vec![0.0_f64; n]; n];
        let mut bx = vec![0.0_f64; n];
        let mut by = vec![0.0_f64; n];

        // Data term: MᵀM and Aᵀb from the anchors.
        for a in anchors {
            let bu = basis_all(a.u, nu, base.du, &base.ku);
            let bv = basis_all(a.v, nv, base.dv, &base.kv);
            // Full tensor-product weight row.
            let mut row = vec![0.0_f64; n];
            for iv in 0..nv {
                let wv = bv[iv];
                if wv == 0.0 {
                    continue;
                }
                for iu in 0..nu {
                    row[iv * nu + iu] = wv * bu[iu];
                }
            }
            for i in 0..n {
                let ri = row[i];
                if ri == 0.0 {
                    continue;
                }
                bx[i] += ri * a.x;
                by[i] += ri * a.y;
                for j in 0..n {
                    let rj = row[j];
                    if rj != 0.0 {
                        m[i][j] += ri * rj;
                    }
                }
            }
        }

        // Smoothing term: lambda * (Duᵀ Du + Dvᵀ Dv), where D is the discrete
        // 2nd difference along each axis. Its null space is the affine
        // functions, which the identity pull below pins down.
        if lambda != 0.0 {
            // Along u (for each row of constant v).
            for iv in 0..nv {
                for iu in 1..nu.saturating_sub(1) {
                    let idx = [iv * nu + (iu - 1), iv * nu + iu, iv * nu + (iu + 1)];
                    let coeff = [1.0_f64, -2.0, 1.0];
                    add_stencil(&mut m, &idx, &coeff, lambda);
                }
            }
            // Along v (for each column of constant u).
            for iu in 0..nu {
                for iv in 1..nv.saturating_sub(1) {
                    let idx = [(iv - 1) * nu + iu, iv * nu + iu, (iv + 1) * nu + iu];
                    let coeff = [1.0_f64, -2.0, 1.0];
                    add_stencil(&mut m, &idx, &coeff, lambda);
                }
            }
        }

        // Identity pull: eps * (c - c_identity). Small, but it removes the
        // remaining null space (affine directions unconstrained by data /
        // smoothing) and guarantees a unique SPD solution.
        let eps = 1e-6_f64;
        for i in 0..n {
            m[i][i] += eps;
            let (cx, cy) = base.cp[i];
            bx[i] += eps * cx;
            by[i] += eps * cy;
        }

        let sol = solve_spd(&m, &[bx, by]);
        let (sx, sy) = (&sol[0], &sol[1]);
        let mut cp = Vec::with_capacity(n);
        for i in 0..n {
            cp.push((sx[i], sy[i]));
        }

        Warp { cp, ..base }
    }

    /// Polyline (`n` points) of the iso-parameter curve at constant `v`
    /// (horizontal-ish: constant migration sampled across lanes).
    pub fn iso_v(&self, v: f64, n: usize) -> Vec<(f64, f64)> {
        let mut out = Vec::with_capacity(n);
        if n == 0 {
            return out;
        }
        if n == 1 {
            out.push(self.eval(0.5, v));
            return out;
        }
        for i in 0..n {
            let u = i as f64 / (n - 1) as f64;
            out.push(self.eval(u, v));
        }
        out
    }

    /// Polyline (`n` points) of the iso-parameter curve at constant `u`
    /// (a single lane's path down the gel).
    pub fn iso_u(&self, u: f64, n: usize) -> Vec<(f64, f64)> {
        let mut out = Vec::with_capacity(n);
        if n == 0 {
            return out;
        }
        if n == 1 {
            out.push(self.eval(u, 0.5));
            return out;
        }
        for i in 0..n {
            let v = i as f64 / (n - 1) as f64;
            out.push(self.eval(u, v));
        }
        out
    }
}

/// Accumulate `scale * (stencil stencilᵀ)` into `m` for the given indices and
/// coefficients (a rank-1 update per difference row).
fn add_stencil(m: &mut [Vec<f64>], idx: &[usize], coeff: &[f64], scale: f64) {
    for a in 0..idx.len() {
        for b in 0..idx.len() {
            m[idx[a]][idx[b]] += scale * coeff[a] * coeff[b];
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    fn approx_pt(p: (f64, f64), q: (f64, f64), tol: f64) -> bool {
        approx(p.0, q.0, tol) && approx(p.1, q.1, tol)
    }

    #[test]
    fn identity_reproduces_linear_map() {
        let w = 640.0;
        let h = 480.0;
        let warp = Warp::identity(w, h, 4, 4);
        assert!(approx_pt(warp.eval(0.0, 0.0), (0.0, 0.0), 1e-6));
        assert!(approx_pt(warp.eval(1.0, 1.0), (w, h), 1e-6));
        assert!(approx_pt(warp.eval(1.0, 0.0), (w, 0.0), 1e-6));
        assert!(approx_pt(warp.eval(0.0, 1.0), (0.0, h), 1e-6));
        // For nu = nv = 4 the regular lattice equals the Greville abscissae, so
        // the linear map is reproduced exactly at the midpoint too.
        assert!(approx_pt(warp.eval(0.5, 0.5), (w / 2.0, h / 2.0), 1e-6));
        assert!(approx_pt(warp.eval(0.25, 0.75), (w * 0.25, h * 0.75), 1e-6));
    }

    #[test]
    fn grid_and_control_point_access() {
        let mut warp = Warp::identity(100.0, 200.0, 5, 3);
        assert_eq!(warp.grid_size(), (5, 3));
        assert!(approx_pt(warp.control_point(0, 0), (0.0, 0.0), 1e-9));
        assert!(approx_pt(warp.control_point(4, 2), (100.0, 200.0), 1e-9));
        warp.set_control_point(2, 1, 55.0, 111.0);
        assert!(approx_pt(warp.control_point(2, 1), (55.0, 111.0), 1e-9));
    }

    #[test]
    fn derivatives_match_finite_differences() {
        let mut warp = Warp::identity(300.0, 200.0, 4, 4);
        // Perturb an interior control point so the surface is genuinely curved.
        warp.set_control_point(1, 2, warp.control_point(1, 2).0 + 25.0, warp.control_point(1, 2).1 - 15.0);
        let eps = 1e-5;
        for &(u, v) in &[(0.3, 0.4), (0.6, 0.2), (0.5, 0.5)] {
            let (dux, duy) = warp.deriv_u(u, v);
            let (dvx, dvy) = warp.deriv_v(u, v);
            let fp = warp.eval(u + eps, v);
            let fm = warp.eval(u - eps, v);
            assert!(approx((fp.0 - fm.0) / (2.0 * eps), dux, 1e-2));
            assert!(approx((fp.1 - fm.1) / (2.0 * eps), duy, 1e-2));
            let gp = warp.eval(u, v + eps);
            let gm = warp.eval(u, v - eps);
            assert!(approx((gp.0 - gm.0) / (2.0 * eps), dvx, 1e-2));
            assert!(approx((gp.1 - gm.1) / (2.0 * eps), dvy, 1e-2));
        }
    }

    #[test]
    fn invert_round_trips_identity() {
        let warp = Warp::identity(500.0, 400.0, 4, 4);
        for &(u, v) in &[(0.2, 0.3), (0.5, 0.5), (0.7, 0.15), (0.9, 0.85), (0.05, 0.6)] {
            let (x, y) = warp.eval(u, v);
            let (iu, iv) = warp.invert(x, y);
            assert!(approx(iu, u, 1e-3), "u {} vs {}", iu, u);
            assert!(approx(iv, v, 1e-3), "v {} vs {}", iv, v);
        }
    }

    #[test]
    fn invert_round_trips_perturbed() {
        let mut warp = Warp::identity(500.0, 400.0, 5, 5);
        warp.set_control_point(2, 2, warp.control_point(2, 2).0 + 40.0, warp.control_point(2, 2).1 + 30.0);
        warp.set_control_point(1, 3, warp.control_point(1, 3).0 - 20.0, warp.control_point(1, 3).1 + 10.0);
        for &(u, v) in &[(0.25, 0.35), (0.5, 0.5), (0.6, 0.7), (0.8, 0.2)] {
            let (x, y) = warp.eval(u, v);
            let (iu, iv) = warp.invert(x, y);
            let back = warp.eval(iu, iv);
            // The map may not be globally injective under large perturbation, so
            // verify the recovered parameters reproduce the image point.
            assert!(approx_pt(back, (x, y), 1e-3), "roundtrip {:?} vs {:?}", back, (x, y));
            assert!(approx(iu, u, 1e-3) && approx(iv, v, 1e-3), "param {:?} vs {:?}", (iu, iv), (u, v));
        }
    }

    #[test]
    fn fit_recovers_smooth_smile() {
        let w = 200.0;
        let h = 150.0;
        let amp = 18.0;
        // Synthetic generator: straight lanes in x, a quadratic "smile" in the
        // migration direction (representable exactly by a cubic tensor spline).
        let gen = |u: f64, v: f64| -> (f64, f64) {
            let x = u * w;
            let y = v * h + amp * (u - 0.5) * (u - 0.5);
            (x, y)
        };
        let mut anchors = Vec::new();
        for i in 0..7 {
            for j in 0..7 {
                let u = i as f64 / 6.0;
                let v = j as f64 / 6.0;
                let (x, y) = gen(u, v);
                anchors.push(Anchor { u, v, x, y });
            }
        }
        let warp = Warp::fit(&anchors, w, h, 4, 4, 0.0);
        for &(u, v) in &[(0.2, 0.3), (0.5, 0.5), (0.35, 0.8), (0.9, 0.1), (0.65, 0.45)] {
            let got = warp.eval(u, v);
            let want = gen(u, v);
            assert!(approx_pt(got, want, 0.5), "at ({},{}) got {:?} want {:?}", u, v, got, want);
        }
    }

    #[test]
    fn fit_single_lane_is_stable_and_near_identity_in_u() {
        let w = 300.0;
        let h = 240.0;
        // All anchors share u = 0.5: a single lane, so the u-axis is
        // under-constrained. y follows a slightly nonlinear migration.
        let mut anchors = Vec::new();
        for j in 0..9 {
            let v = j as f64 / 8.0;
            let x = 0.5 * w;
            let y = v * h + 6.0 * (v - 0.5) * (v - 0.5);
            anchors.push(Anchor { u: 0.5, v, x, y });
        }
        // Must not panic on this rank-deficient input.
        let warp = Warp::fit(&anchors, w, h, 4, 4, 1.0);

        // Along the unconstrained u-axis the fit should stay close to identity.
        for &v in &[0.25_f64, 0.5, 0.75] {
            for &u in &[0.0_f64, 0.25, 0.5, 0.75, 1.0] {
                let got = warp.eval(u, v);
                assert!(
                    approx(got.0, u * w, 1e-2 * w + 1.0),
                    "x at ({},{}) = {} not near identity {}",
                    u,
                    v,
                    got.0,
                    u * w
                );
            }
        }
    }

    #[test]
    fn iso_curves_lie_on_surface() {
        let mut warp = Warp::identity(400.0, 300.0, 4, 4);
        warp.set_control_point(1, 1, warp.control_point(1, 1).0 + 20.0, warp.control_point(1, 1).1 + 12.0);
        warp.set_control_point(2, 2, warp.control_point(2, 2).0 - 15.0, warp.control_point(2, 2).1 + 8.0);

        let n = 11;
        let v0 = 0.4;
        let poly_v = warp.iso_v(v0, n);
        assert_eq!(poly_v.len(), n);
        for (i, &p) in poly_v.iter().enumerate() {
            let u = i as f64 / (n - 1) as f64;
            assert!(approx_pt(p, warp.eval(u, v0), 1e-9));
        }

        let u0 = 0.7;
        let poly_u = warp.iso_u(u0, n);
        assert_eq!(poly_u.len(), n);
        for (i, &p) in poly_u.iter().enumerate() {
            let v = i as f64 / (n - 1) as f64;
            assert!(approx_pt(p, warp.eval(u0, v), 1e-9));
        }
    }
}
