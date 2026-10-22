//! Small deterministic noises for detail the 10 m to 1 km data is too coarse to hold: relief a
//! few blocks high, and patches of ground cover tens of metres across.

/// A 64-bit mix of a seed and a position (splitmix64's finaliser).
pub fn mix(seed: u64, x: i64, z: i64) -> u64 {
    let mut h = seed
        ^ (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (z as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^ (h >> 31)
}

/// 2D gradient noise in about -1..1, smooth, with features one unit across.
#[derive(Clone, Copy, Debug)]
pub struct Noise {
    seed: u64,
}
impl Noise {
    pub fn new(seed: u64) -> Self {
        Self { seed }
    }
    fn gradient(&self, ix: i64, iz: i64, fx: f64, fz: f64) -> f64 {
        // Eight directions, unit length.
        const H: f64 = std::f64::consts::FRAC_1_SQRT_2;
        const D: [(f64, f64); 8] = [
            (1.0, 0.0),
            (-1.0, 0.0),
            (0.0, 1.0),
            (0.0, -1.0),
            (H, H),
            (-H, H),
            (H, -H),
            (-H, -H),
        ];
        let (gx, gz) = D[(mix(self.seed, ix, iz) >> 61) as usize];
        gx * fx + gz * fz
    }
    pub fn at(&self, x: f64, z: f64) -> f64 {
        let (ix, iz) = (x.floor() as i64, z.floor() as i64);
        let (fx, fz) = (x - ix as f64, z - iz as f64);
        let fade = |t: f64| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
        let (u, v) = (fade(fx), fade(fz));
        let a = self.gradient(ix, iz, fx, fz);
        let b = self.gradient(ix + 1, iz, fx - 1.0, fz);
        let c = self.gradient(ix, iz + 1, fx, fz - 1.0);
        let d = self.gradient(ix + 1, iz + 1, fx - 1.0, fz - 1.0);
        let top = a + (b - a) * u;
        let bottom = c + (d - c) * u;
        (top + (bottom - top) * v) * 1.414
    }
    /// Octaves halving in size and strength.
    pub fn fbm(&self, x: f64, z: f64, octaves: u32) -> f64 {
        let (mut sum, mut amplitude, mut frequency) = (0.0, 1.0, 1.0);
        for o in 0..octaves {
            sum += amplitude
                * self.at(
                    x * frequency + o as f64 * 17.3,
                    z * frequency - o as f64 * 9.1,
                );
            amplitude *= 0.5;
            frequency *= 2.0;
        }
        sum
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_smooth_bounded_and_repeatable() {
        let n = Noise::new(7);
        let mut max: f64 = 0.0;
        for i in 0..2000 {
            let (x, z) = (i as f64 * 0.37, i as f64 * -0.21);
            let v = n.at(x, z);
            assert_eq!(v, Noise::new(7).at(x, z));
            max = max.max(v.abs());
            assert!((n.at(x + 0.001, z) - v).abs() < 0.01);
        }
        assert!(max <= 1.5 && max > 0.3, "{max}");
    }
}
