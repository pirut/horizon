//! Spherical Web Mercator block grid (Earth Platform's `web-mercator-v1`).
//!
//! X points east, Z south, origin at 0° 0°. A block spans `metres_per_block` projected metres:
//! ground metres at the equator, and `metres_per_block / sec(latitude)` elsewhere. The projection
//! is conformal, so shapes are never stretched in one axis, and block coordinates are a linear map
//! of slippy-map pixel coordinates, so tiled rasters need no trigonometry per sample.
use std::f64::consts::PI;

pub const RADIUS: f64 = 6_378_137.0;
pub const MAX_LATITUDE: f64 = 85.051_128_779_806_59;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub metres_per_block: f64,
}
impl Grid {
    /// One block is one metre across at `latitude`, as an Earth Platform world's home.
    pub fn at_latitude(latitude: f64) -> Self {
        Self {
            metres_per_block: 1.0 / latitude.to_radians().cos(),
        }
    }
    /// Circumference in blocks, which is also the covered square's north-south extent.
    pub fn world_width(&self) -> f64 {
        2.0 * PI * RADIUS / self.metres_per_block
    }
    pub fn block_x(&self, longitude: f64) -> f64 {
        longitude.to_radians() * RADIUS / self.metres_per_block
    }
    pub fn block_z(&self, latitude: f64) -> f64 {
        let lat = latitude.clamp(-MAX_LATITUDE, MAX_LATITUDE).to_radians();
        -(PI / 4.0 + lat / 2.0).tan().ln() * RADIUS / self.metres_per_block
    }
    /// Latitude and longitude of a block-space position; X wraps round the globe.
    pub fn geographic(&self, x: f64, z: f64) -> (f64, f64) {
        let half = self.world_width() / 2.0;
        let z = z.clamp(-half, half);
        let lon = (x * self.metres_per_block / RADIUS).to_degrees();
        let lat =
            (2.0 * (-z * self.metres_per_block / RADIUS).exp().atan() - PI / 2.0).to_degrees();
        (lat, (lon + 180.0).rem_euclid(360.0) - 180.0)
    }
    /// Projected metres per ground metre at this Z (sec latitude).
    pub fn scale_factor(&self, z: f64) -> f64 {
        (z * self.metres_per_block / RADIUS).cosh()
    }
    /// Ground metres one block covers at this Z.
    pub fn ground_metres_per_block(&self, z: f64) -> f64 {
        self.metres_per_block / self.scale_factor(z)
    }
    /// Global slippy-map pixel position (256-pixel tiles) of block X at a zoom.
    pub fn pixel_x(&self, x: f64, zoom: u8) -> f64 {
        (x / self.world_width() + 0.5) * pixels_at(zoom)
    }
    pub fn pixel_y(&self, z: f64, zoom: u8) -> f64 {
        (z / self.world_width() + 0.5) * pixels_at(zoom)
    }
    /// Blocks one source pixel spans at a zoom: the same everywhere in this projection.
    pub fn blocks_per_pixel(&self, zoom: u8) -> f64 {
        self.world_width() / pixels_at(zoom)
    }
    /// The zoom with about one source pixel per `voxel` blocks, as Earth Platform's far levels
    /// read: level 0 (single blocks) reads like its level 1.
    pub fn zoom_for(&self, voxel: f64, coarsest: u8, finest: u8) -> u8 {
        let zoom = (self.world_width() / (256.0 * voxel.max(2.0)))
            .log2()
            .round();
        (zoom as i64).clamp(coarsest as i64, finest as i64) as u8
    }
}
pub fn pixels_at(zoom: u8) -> f64 {
    256.0 * (1u64 << zoom) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_go_there_and_back() {
        let grid = Grid::at_latitude(37.724);
        for (lat, lon) in [
            (37.724, -119.6365),
            (-3.4653, -62.2159),
            (69.0, 18.9),
            (-77.8, 166.7),
        ] {
            let (x, z) = (grid.block_x(lon), grid.block_z(lat));
            let (la, lo) = grid.geographic(x, z);
            assert!(
                (la - lat).abs() < 1e-9 && (lo - lon).abs() < 1e-9,
                "{lat} {lon} -> {la} {lo}"
            );
        }
        // A block is a metre across at the home latitude.
        let z = grid.block_z(37.724);
        assert!((grid.ground_metres_per_block(z) - 1.0).abs() < 1e-9);
        // Yosemite's blocks read zoom 15, the finest the terrain tiles have.
        assert_eq!(grid.zoom_for(1.0, 9, 15), 15);
    }
}
