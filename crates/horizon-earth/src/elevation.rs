//! Continuous elevation over the whole grid, bicubic (Catmull-Rom) through the tiles' pixel
//! centres, as Earth Platform's GlobalElevation.
//!
//! The tiles carry ocean depths only up to zoom [`BATHYMETRY_ZOOM`]; finer zooms hold open sea as
//! exactly 0 m. Each such pixel takes the bicubic depth of the bathymetry zoom instead, before the
//! detail kernel runs, so coasts blend. Sampling works in slippy-map pixel space, which is linear
//! in block coordinates: tiles join without seams and X wraps across the antimeridian.
//!
//! Unlike Earth Platform, which looks up a tile per sample, a chunk or far-terrain grid first
//! gathers the pixel window it covers ([`Elevation::window`]) and then evaluates every column
//! from that flat array, so the tile cache is touched a few times per chunk, not 4,096.
use crate::{
    fetch::Missing,
    grid::Grid,
    terrarium::{MAX_ZOOM, TILE, Tile, TileSource},
};
use std::sync::Arc;

pub const BATHYMETRY_ZOOM: u8 = 10;

pub struct Elevation {
    grid: Grid,
    tiles: Arc<TileSource>,
    zoom: u8,
    bathymetry: Option<Box<Elevation>>,
}
/// Height and slope at a point.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    pub metres: f64,
    /// Rise in metres per block along X and Z.
    pub dx: f64,
    pub dz: f64,
    /// Interpolation weight of open-sea pixels: about 1 offshore, 0 inland, 0.5 on the coast.
    /// Always 0 at or below the bathymetry zoom.
    pub sea: f64,
}
/// Source pixels around an area, open sea filled with bathymetry.
pub struct Window {
    x0: i64,
    y0: i64,
    width: usize,
    metres: Vec<f32>,
    sea: Vec<bool>,
    pixels_per_block: f64,
    zoom: u8,
    grid: Grid,
}
impl Elevation {
    pub fn new(grid: Grid, tiles: Arc<TileSource>, zoom: u8) -> Self {
        assert!(zoom <= MAX_ZOOM);
        let bathymetry = (zoom > BATHYMETRY_ZOOM)
            .then(|| Box::new(Elevation::new(grid, tiles.clone(), BATHYMETRY_ZOOM)));
        Self {
            grid,
            tiles,
            zoom,
            bathymetry,
        }
    }
    pub fn zoom(&self) -> u8 {
        self.zoom
    }
    /// A tile, or when it can't be had now, the coarser tile above it resampled, so generation
    /// goes on with less detail rather than leaving a hole.
    fn tile(&self, zoom: u8, tx: u32, ty: u32) -> Arc<Tile> {
        match self.tiles.tile(zoom, tx, ty) {
            Ok(tile) => tile,
            Err(missing) => {
                log::warn!(
                    "Earth: elevation tile {zoom}/{tx}/{ty}: {missing}; using a coarser one"
                );
                if zoom == 0 {
                    return Arc::new(vec![0f32; TILE * TILE]);
                }
                let parent = self.tile(zoom - 1, tx / 2, ty / 2);
                let (ox, oy) = ((tx % 2) as usize * 128, (ty % 2) as usize * 128);
                let mut out: Tile = vec![0f32; TILE * TILE];
                for y in 0..TILE {
                    for x in 0..TILE {
                        out[y * TILE + x] = parent[(oy + y / 2) * TILE + ox + x / 2];
                    }
                }
                Arc::new(out)
            }
        }
    }
    /// Raw pixels `x0 .. x0 + width`, `y0 .. y0 + height` of this zoom, X wrapped and Y clamped.
    fn gather(&self, x0: i64, y0: i64, width: usize, height: usize) -> Vec<f32> {
        let size = (TILE as i64) << self.zoom;
        let t = TILE as i64;
        let mut out = vec![0f32; width * height];
        let mut last: Option<((u32, u32), Arc<Tile>)> = None;
        for row in 0..height {
            let gy = (y0 + row as i64).clamp(0, size - 1);
            let (ty, py) = ((gy / t) as u32, (gy % t) as usize);
            let mut col = 0;
            while col < width {
                let gx = (x0 + col as i64).rem_euclid(size);
                let tx = (gx / t) as u32;
                let run = ((t - gx % t) as usize).min(width - col);
                let tile = match &last {
                    Some((key, tile)) if *key == (tx, ty) => tile.clone(),
                    _ => {
                        let tile = self.tile(self.zoom, tx, ty);
                        last = Some(((tx, ty), tile.clone()));
                        tile
                    }
                };
                let src = &tile[py * TILE + (gx % t) as usize..][..run];
                out[row * width + col..][..run].copy_from_slice(src);
                col += run;
            }
        }
        out
    }
    /// The pixels the bicubic kernel reads for every block position in `x0..x1`, `z0..z1`.
    pub fn window(&self, x0: f64, z0: f64, x1: f64, z1: f64) -> Window {
        let px0 = (self.grid.pixel_x(x0, self.zoom) - 0.5).floor() as i64 - 1;
        let py0 = (self.grid.pixel_y(z0, self.zoom) - 0.5).floor() as i64 - 1;
        let px1 = (self.grid.pixel_x(x1, self.zoom) - 0.5).floor() as i64 + 2;
        let py1 = (self.grid.pixel_y(z1, self.zoom) - 0.5).floor() as i64 + 2;
        let (width, height) = ((px1 - px0 + 1) as usize, (py1 - py0 + 1) as usize);
        let mut metres = self.gather(px0, py0, width, height);
        let mut sea = vec![false; metres.len()];
        if let Some(bathymetry) = &self.bathymetry
            && metres.contains(&0.0)
        {
            let size = (TILE as i64) << self.zoom;
            let scale = (1u64 << BATHYMETRY_ZOOM) as f64 / (1u64 << self.zoom) as f64;
            // The bathymetry pixels under this window, in the bathymetry's own pixel space.
            let at = |g: i64, clamp: bool| {
                let g = if clamp {
                    g.clamp(0, size - 1)
                } else {
                    g.rem_euclid(size)
                };
                (g as f64 + 0.5) * scale - 0.5
            };
            let (bx0, by0) = (at(px0, false), at(py0, true));
            let coarse = bathymetry.pixel_window(
                bx0.floor() as i64 - 1,
                by0.floor() as i64 - 1,
                (width as f64 * scale).ceil() as usize + 4,
                (height as f64 * scale).ceil() as usize + 4,
            );
            for j in 0..height {
                for i in 0..width {
                    let k = j * width + i;
                    if metres[k] == 0.0 {
                        sea[k] = true;
                        // Unwrapped X keeps the window's X contiguous across the antimeridian.
                        let bx = bx0 + i as f64 * scale;
                        let by = at(py0 + j as i64, true);
                        metres[k] = coarse.bicubic(bx, by).min(0.0) as f32;
                    }
                }
            }
        }
        Window {
            x0: px0,
            y0: py0,
            width,
            metres,
            sea,
            pixels_per_block: 1.0 / self.grid.blocks_per_pixel(self.zoom),
            zoom: self.zoom,
            grid: self.grid,
        }
    }
    /// Raw pixels as a window in pixel space, without the bathymetry fill.
    fn pixel_window(&self, x0: i64, y0: i64, width: usize, height: usize) -> Window {
        Window {
            x0,
            y0,
            width,
            metres: self.gather(x0, y0, width, height),
            sea: Vec::new(),
            pixels_per_block: 1.0 / self.grid.blocks_per_pixel(self.zoom),
            zoom: self.zoom,
            grid: self.grid,
        }
    }
    /// Height and slope at one block-space position.
    pub fn sample(&self, x: f64, z: f64) -> Sample {
        self.window(x, z, x, z).sample(x, z)
    }
    /// The tiles a block-space square reads at this zoom, for prefetching.
    pub fn tiles_over(&self, x0: f64, z0: f64, x1: f64, z1: f64) -> Vec<(u8, u32, u32)> {
        let n = 1i64 << self.zoom;
        let tx0 = (self.grid.pixel_x(x0, self.zoom) / TILE as f64).floor() as i64;
        let tx1 = (self.grid.pixel_x(x1, self.zoom) / TILE as f64).floor() as i64;
        let ty0 = ((self.grid.pixel_y(z0, self.zoom) / TILE as f64).floor() as i64).max(0);
        let ty1 = ((self.grid.pixel_y(z1, self.zoom) / TILE as f64).floor() as i64).min(n - 1);
        let mut out = Vec::new();
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1.min(tx0 + n - 1) {
                out.push((self.zoom, tx.rem_euclid(n) as u32, ty as u32));
            }
        }
        out
    }
}
impl Window {
    fn pixel(&self, i: i64, j: i64) -> f64 {
        let (i, j) = ((i - self.x0) as usize, (j - self.y0) as usize);
        self.metres[j * self.width + i] as f64
    }
    /// Bicubic through pixel centres at a pixel-space position (centres at whole numbers).
    fn bicubic(&self, px: f64, py: f64) -> f64 {
        let (ix, iy) = (px.floor() as i64, py.floor() as i64);
        let (fx, fy) = (px - ix as f64, py - iy as f64);
        let mut result = 0.0;
        for j in -1..=2 {
            let mut row = 0.0;
            for i in -1..=2 {
                row += self.pixel(ix + i, iy + j) * weight(i as f64 - fx);
            }
            result += row * weight(j as f64 - fy);
        }
        result
    }
    /// Height, slope and sea share at a block-space position inside the window's area.
    pub fn sample(&self, x: f64, z: f64) -> Sample {
        let px = self.grid.pixel_x(x, self.zoom) - 0.5;
        let py = self.grid.pixel_y(z, self.zoom) - 0.5;
        let (ix, iy) = (px.floor() as i64, py.floor() as i64);
        let (fx, fy) = (px - ix as f64, py - iy as f64);
        let (mut value, mut dx, mut dy, mut sea) = (0.0, 0.0, 0.0, 0.0);
        let wx: [f64; 4] = std::array::from_fn(|i| weight(i as f64 - 1.0 - fx));
        let dwx: [f64; 4] = std::array::from_fn(|i| -slope(i as f64 - 1.0 - fx));
        for j in 0..4 {
            let wy = weight(j as f64 - 1.0 - fy);
            let dwy = -slope(j as f64 - 1.0 - fy);
            let row = ((iy + j - 1 - self.y0) as usize) * self.width;
            for i in 0..4 {
                let k = row + (ix + i as i64 - 1 - self.x0) as usize;
                let p = self.metres[k] as f64;
                value += p * wx[i] * wy;
                dx += p * dwx[i] * wy;
                dy += p * wx[i] * dwy;
                if self.sea.get(k).copied().unwrap_or(false) {
                    sea += wx[i] * wy;
                }
            }
        }
        Sample {
            metres: value,
            dx: dx * self.pixels_per_block,
            dz: dy * self.pixels_per_block,
            sea,
        }
    }
}

/// Catmull-Rom: exact through pixel centres, with continuous first derivatives.
fn weight(t: f64) -> f64 {
    let t = t.abs();
    if t < 1.0 {
        1.5 * t * t * t - 2.5 * t * t + 1.0
    } else if t < 2.0 {
        -0.5 * t * t * t + 2.5 * t * t - 4.0 * t + 2.0
    } else {
        0.0
    }
}
fn slope(t: f64) -> f64 {
    let (a, sign) = (t.abs(), t.signum());
    if a < 1.0 {
        sign * (4.5 * a * a - 5.0 * a)
    } else if a < 2.0 {
        sign * (-1.5 * a * a + 5.0 * a - 4.0)
    } else {
        0.0
    }
}

/// Prefetches every tile in `tiles` that isn't decoded yet, `threads` at a time.
pub fn prefetch(
    source: &TileSource,
    tiles: &[(u8, u32, u32)],
    threads: usize,
) -> Result<(), Missing> {
    let failed = std::sync::Mutex::new(None);
    let jobs: Vec<Box<dyn FnOnce() + Send + '_>> = tiles
        .iter()
        .filter(|&&(z, x, y)| source.kept(z, x, y).is_none())
        .map(|&(z, x, y)| {
            let failed = &failed;
            Box::new(move || {
                if let Err(e) = source.tile(z, x, y) {
                    failed.lock().unwrap().get_or_insert(e);
                }
            }) as Box<dyn FnOnce() + Send + '_>
        })
        .collect();
    crate::fetch::run_all(jobs, threads);
    match failed.into_inner().unwrap() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kernel_passes_through_pixel_centres() {
        assert_eq!(weight(0.0), 1.0);
        assert_eq!(weight(1.0), 0.0);
        assert_eq!(weight(2.0), 0.0);
        let sum: f64 = (-1..=2).map(|i| weight(i as f64 - 0.3)).sum();
        assert!((sum - 1.0).abs() < 1e-12);
    }
}
