//! A small deterministic PRNG (PCG32) with the distributions the simulator
//! needs. Seedable and reproducible so ground truth is stable per seed.

pub struct Rng {
    state: u64,
    inc: u64,
    // Cache for the second Box–Muller gaussian.
    gauss_cache: Option<f64>,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut rng = Rng {
            state: 0,
            inc: (seed << 1) | 1,
            gauss_cache: None,
        };
        rng.next_u32();
        rng.state = rng.state.wrapping_add(seed);
        rng.next_u32();
        rng
    }

    fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(6364136223846793005).wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// Uniform in `[0, 1)`.
    pub fn unit(&mut self) -> f64 {
        self.next_u32() as f64 / (u32::MAX as f64 + 1.0)
    }

    /// Uniform in `[lo, hi)`.
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    /// Standard normal via Box–Muller (cached pair).
    pub fn gaussian(&mut self) -> f64 {
        if let Some(g) = self.gauss_cache.take() {
            return g;
        }
        let u1 = self.unit().max(1e-12);
        let u2 = self.unit();
        let r = (-2.0 * u1.ln()).sqrt();
        let (s, c) = (std::f64::consts::TAU * u2).sin_cos();
        self.gauss_cache = Some(r * s);
        r * c
    }

    /// Poisson sample with mean `lambda`. Knuth for small lambda; a normal
    /// approximation for large lambda (both non-negative).
    pub fn poisson(&mut self, lambda: f64) -> f64 {
        if lambda <= 0.0 {
            return 0.0;
        }
        if lambda > 30.0 {
            let v = lambda + lambda.sqrt() * self.gaussian();
            return v.max(0.0).round();
        }
        let l = (-lambda).exp();
        let mut k = 0.0;
        let mut p = 1.0;
        loop {
            k += 1.0;
            p *= self.unit();
            if p <= l {
                return k - 1.0;
            }
        }
    }
}
