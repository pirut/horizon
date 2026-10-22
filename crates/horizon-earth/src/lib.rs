//! The real Earth as a Horizon world, ported from Earth Platform
//! (<https://github.com/pirut/earth-platform>): one block is one metre across, elevation, land
//! cover and climate stream from open data on first visit and stay cached on disk.
//!
//! [`Earth::region`] turns a square of block columns into ground heights, water and surfaces in
//! one pass: one elevation window, one land cover grid and a climate lattice per region, instead
//! of a cache lookup per sample. [`Earth::prepare`] fetches everything a place needs at once,
//! every source in parallel, before the first chunk asks. docs/earth.md has the design.
pub mod climate;
pub mod cog;
pub mod cube;
pub mod elevation;
pub mod fetch;
pub mod grid;
pub mod noise;
pub mod surface;
pub mod terrarium;
pub mod worldcover;

use climate::{ClimateSource, Zone};
use elevation::Elevation;
use fetch::{DiskCache, Flight, Http, Missing};
use grid::Grid;
use noise::{Noise, mix};
use std::{path::PathBuf, sync::Arc, time::Duration};
use surface::{Block, Ground, Local, Surface};
use terrarium::TileSource;
use worldcover::WorldCover;

/// The sea's top water block, as in Earth Platform: ground at 0 m is level with it.
pub const SEA: i32 = 62;
/// Coarsest and finest elevation zooms read.
const COARSEST_ZOOM: u8 = 9;
const FINEST_ZOOM: u8 = terrarium::MAX_ZOOM;

#[derive(Clone, Debug)]
pub struct Settings {
    pub latitude: f64,
    pub longitude: f64,
    pub seed: u64,
    /// The world's lowest block and its height in blocks.
    pub min_y: i32,
    pub height: i32,
    /// Earth Platform's cache layout; None keeps everything in memory.
    pub cache: Option<PathBuf>,
    pub allow_network: bool,
    /// Downloads at once.
    pub transfers: usize,
}
impl Settings {
    /// El Capitan Meadow in Yosemite Valley, Earth Platform's default home.
    pub fn yosemite() -> Self {
        Self::at(37.724, -119.6365)
    }
    pub fn at(latitude: f64, longitude: f64) -> Self {
        Self {
            latitude,
            longitude,
            seed: 1234,
            min_y: cube::MIN_Y,
            height: cube::HEIGHT,
            cache: None,
            allow_network: true,
            transfers: 48,
        }
    }
}

/// Real heights to world Y: true metres up to a knee, compressed above it (and below a knee
/// under the sea) so the planet's peaks and trenches fit the world's height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertical {
    up: (f64, f64),
    down: (f64, f64),
}
impl Vertical {
    /// For a world `min_y .. min_y + height`, keeping room for trees above the highest peak.
    pub fn new(min_y: i32, height: i32) -> Self {
        let top = (min_y + height - 1 - SEA - 32) as f64;
        let bottom = (SEA - min_y - 6) as f64;
        Self {
            up: curve(top, 8_848.0),
            down: curve(bottom, 10_935.0),
        }
    }
    /// Blocks above the sea's top water block for a height in metres.
    pub fn blocks(&self, metres: f64) -> f64 {
        if metres >= 0.0 {
            squash(metres, self.up)
        } else {
            -squash(-metres, self.down)
        }
    }
    /// How many blocks a metre is at this height (the curve's slope).
    pub fn rate(&self, metres: f64) -> f64 {
        let (knee, c) = if metres >= 0.0 { self.up } else { self.down };
        let m = metres.abs();
        if m <= knee { 1.0 } else { c / (c + m - knee) }
    }
}
/// Knee and softness for a span of `room` blocks that must hold `most` metres.
fn curve(room: f64, most: f64) -> (f64, f64) {
    if room >= most {
        return (most, 1.0);
    }
    let knee = if room >= 1900.0 { 1500.0 } else { room * 0.45 };
    // Solve c ln(1 + (most - knee) / c) = room - knee for c by bisection.
    let target = room - knee;
    let (mut lo, mut hi) = (1e-3, most);
    for _ in 0..100 {
        let c = (lo + hi) / 2.0;
        if c * (1.0 + (most - knee) / c).ln() > target {
            hi = c;
        } else {
            lo = c;
        }
    }
    (knee, (lo + hi) / 2.0)
}
fn squash(m: f64, (knee, c): (f64, f64)) -> f64 {
    if m <= knee {
        m
    } else {
        knee + c * (1.0 + (m - knee) / c).ln()
    }
}

/// One column of the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Column {
    /// Y of the top solid block.
    pub ground: i32,
    /// Y of the top water block, if any.
    pub water: Option<i32>,
    pub surface: Surface,
    /// Water that freezes over.
    pub frozen: bool,
}

/// Fetch report for the benchmark.
#[derive(Clone, Debug, Default)]
pub struct Prepared {
    pub pieces: usize,
    pub failed: usize,
    pub first_failure: Option<String>,
}

pub struct Earth {
    settings: Settings,
    grid: Grid,
    vertical: Vertical,
    http: Arc<Http>,
    tiles: Arc<TileSource>,
    elevation: Vec<Elevation>,
    cover: WorldCover,
    climate: ClimateSource,
    relief: Noise,
    patches: Noise,
    warp: Noise,
    lakes: Flight<(i32, i32, u8), Option<f64>>,
}

/// Blocks across a lake cell: one water level per cell.
const LAKE_CELL: i32 = 64;

impl Earth {
    pub fn new(settings: Settings) -> Self {
        let grid = Grid::at_latitude(settings.latitude);
        let http = Arc::new(Http::new(
            settings.transfers,
            Duration::from_secs(20),
            settings.allow_network,
        ));
        let cache = DiskCache::new(settings.cache.clone());
        let tiles = Arc::new(TileSource::new(
            terrarium::AWS_TERRAIN_TILES,
            http.clone(),
            cache.clone(),
            1024,
        ));
        let elevation = (0..=FINEST_ZOOM)
            .map(|z| Elevation::new(grid, tiles.clone(), z))
            .collect();
        Self {
            vertical: Vertical::new(settings.min_y, settings.height),
            cover: WorldCover::new(worldcover::AWS_WORLDCOVER, http.clone(), cache.clone(), 96),
            climate: ClimateSource::new(climate::CHELSA, http.clone(), cache, 512),
            relief: Noise::new(settings.seed ^ 0x0045_4152_5448_5631),
            patches: Noise::new(settings.seed ^ 0x0043_4F56_4552),
            warp: Noise::new(settings.seed ^ 0x5741_5250),
            lakes: Flight::new(1 << 16),
            grid,
            http,
            tiles,
            elevation,
            settings,
        }
    }
    pub fn settings(&self) -> &Settings {
        &self.settings
    }
    pub fn grid(&self) -> Grid {
        self.grid
    }
    pub fn vertical(&self) -> Vertical {
        self.vertical
    }
    pub fn http(&self) -> &Http {
        &self.http
    }
    /// Block X and Z of a latitude and longitude.
    pub fn block(&self, latitude: f64, longitude: f64) -> (i32, i32) {
        (
            self.grid.block_x(longitude).floor() as i32,
            self.grid.block_z(latitude).floor() as i32,
        )
    }
    pub fn home(&self) -> (i32, i32) {
        self.block(self.settings.latitude, self.settings.longitude)
    }
    fn zoom(&self, voxel: i32) -> u8 {
        self.grid.zoom_for(voxel as f64, COARSEST_ZOOM, FINEST_ZOOM)
    }
    /// Land cover overview for voxels of this many blocks: about one 10 m pixel per voxel.
    fn overview(&self, voxel: i32, z: f64) -> u8 {
        let metres = voxel as f64 * self.grid.ground_metres_per_block(z);
        ((metres / 10.0).log2().floor().max(0.0) as u8).min(worldcover::MAX_OVERVIEW)
    }

    /// `side × side` columns from (`x0`, `z0`), `stride` blocks apart; each column reads the
    /// data at its centre (a far-terrain voxel reads coarser data, about one pixel per voxel).
    pub fn region(&self, x0: i32, z0: i32, side: usize, stride: i32) -> Vec<Column> {
        let stride = stride.max(1);
        let centre = |i: usize, o: i32| o as f64 + i as f64 * stride as f64 + stride as f64 / 2.0;
        let xs: Vec<f64> = (0..side).map(|i| centre(i, x0)).collect();
        let zs: Vec<f64> = (0..side).map(|j| centre(j, z0)).collect();
        let (xl, xh, zl, zh) = (xs[0], xs[side - 1], zs[0], zs[side - 1]);
        let dem = &self.elevation[self.zoom(stride) as usize];
        let window = dem.window(xl, zl, xh, zh);

        // Latitude depends on Z alone and longitude on X alone in this projection.
        let latitudes: Vec<f64> = zs.iter().map(|&z| self.grid.geographic(0.0, z).0).collect();
        let longitudes: Vec<f64> = xs.iter().map(|&x| self.grid.geographic(x, 0.0).1).collect();
        let overview = self.overview(stride, zl);
        let mut codes = vec![worldcover::NO_DATA; side * side];
        if let Err(e) = self
            .cover
            .codes(&latitudes, &longitudes, overview, &mut codes)
        {
            log::warn!("Earth: land cover at {x0} {z0}: {e}");
        }

        // Climate on a lattice no finer than 32 blocks, sea-level temperatures, interpolated.
        let lattice = Lattice::new(self, xl, zl, xh, zh, stride.max(32) as f64);

        let mut out = Vec::with_capacity(side * side);
        for (j, &z) in zs.iter().enumerate() {
            for (i, &x) in xs.iter().enumerate() {
                let sample = window.sample(x, z);
                let code = codes[j * side + i];
                let sea_level = lattice.at(x, z);
                out.push(self.column(x, z, stride, sample, code, sea_level, latitudes[j]));
            }
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn column(
        &self,
        x: f64,
        z: f64,
        voxel: i32,
        sample: elevation::Sample,
        code: u8,
        climate: Option<SeaClimate>,
        latitude: f64,
    ) -> Column {
        let (xi, zi) = (x.floor() as i64, z.floor() as i64);
        let metres = sample.metres;
        let climate = climate.unwrap_or(SeaClimate {
            koppen: 0,
            mean: 27.0 - 0.6 * latitude.abs(),
            warm: 32.0 - 0.45 * latitude.abs(),
            rain: 800.0,
        });
        let at_height = |m: f64| -climate::LAPSE_RATE * m.max(0.0);
        let temperature = climate.mean + at_height(metres);
        let warmest = climate.warm + at_height(metres);
        let local = Local {
            zone: Zone::of(climate.koppen, temperature, warmest),
            temperature,
            warmest,
            rain: climate.rain,
            latitude,
        };
        let v = self.vertical;
        // The open sea: the tiles' zeros, filled with bathymetry.
        if sample.sea > 0.5 {
            let bed = (SEA as f64 + 0.5 + v.blocks(metres.min(-1.0))).floor() as i32;
            let bed = bed.clamp(self.settings.min_y + 2, SEA - 1);
            let wobble = (self.warp.at(x / 97.0, z / 97.0)
                + 0.4 * self.warp.at(x / 41.0 + 9.0, z / 41.0))
                * 4.0;
            let sea_temperature = climate.mean;
            return Column {
                ground: bed,
                water: Some(SEA),
                surface: surface::sea(sea_temperature, SEA - bed, latitude, wobble),
                frozen: sea_temperature < -6.0,
            };
        }
        let rate = v.rate(metres);
        let steep = ((sample.dx * sample.dx + sample.dz * sample.dz).sqrt() * rate).min(2.5);
        // Lakes and rivers lie level at their cell's surveyed surface.
        if code == worldcover::WATER {
            let level = self.lake_level(x, z, voxel);
            if let Some(level) = level.filter(|_| steep <= 0.7) {
                let joins_sea = level <= 2.0 && sample.sea > 0.02 || level < 0.5;
                let surface_y = if joins_sea {
                    SEA
                } else {
                    (SEA as f64 + 0.5 + v.blocks(level)).floor() as i32
                };
                let depth = (3.0 + 3.0 * self.patches.at(x / 23.0, z / 23.0)).round() as i32;
                let surveyed = (SEA as f64 + 0.5 + v.blocks(metres)).floor() as i32;
                let bed = (surface_y - depth.max(1)).min(surveyed);
                return Column {
                    ground: bed.max(self.settings.min_y + 2),
                    water: Some(surface_y),
                    surface: if joins_sea {
                        surface::sea(temperature, surface_y - bed, latitude, 0.0)
                    } else {
                        surface::lake(temperature)
                    },
                    frozen: temperature < -2.0,
                };
            }
        }
        // Small relief the source is too coarse to hold: more on steep ground, none on beaches.
        let amplitude = ((metres - 0.5) / 4.0).clamp(0.0, 1.0) * (0.6 + 1.8 * steep);
        let mut detail = 0.0;
        if voxel <= 8 {
            detail += self.relief.fbm(x / 27.0, z / 27.0, 3) * amplitude;
        }
        let mut y = (SEA as f64 + 0.5 + v.blocks(metres) + detail).floor() as i32;
        if sample.sea > 0.02 && y < SEA {
            y = SEA;
        }
        let top_limit = self.settings.min_y + self.settings.height - 2;
        y = y.clamp(self.settings.min_y + 2, top_limit);
        let roll = (mix(self.settings.seed, xi, zi) % 100) as u32;
        let (wx, wz) = (
            x / 31.0 + 0.7 * self.patches.at(x / 70.0 + 41.3, z / 70.0 - 7.7),
            z / 31.0 + 0.7 * self.patches.at(x / 70.0 - 13.9, z / 70.0 + 28.1),
        );
        let patch = 0.8 * self.patches.at(wx, wz)
            + 0.25 * self.patches.at(x / 7.0 + 5.5, z / 7.0 - 2.2)
            + ((mix(self.settings.seed ^ 0x5041_5443, xi, zi) & 1023) as f64 / 511.5 - 1.0) * 0.18;
        let surface = surface::ground(&Ground {
            code,
            local,
            steep,
            above_sea: y - SEA,
            shore: sample.sea > 0.02,
            roll,
            patch,
        });
        Column {
            ground: y,
            water: None,
            surface,
            frozen: false,
        }
    }

    /// The level, in metres, of the inland water in a cell: the lowest surveyed height of the
    /// water the land cover maps in it, so a lake lies flat. None if the cell maps no water.
    fn lake_level(&self, x: f64, z: f64, voxel: i32) -> Option<f64> {
        let cell = (LAKE_CELL * voxel.clamp(1, 64)).max(LAKE_CELL);
        let (cx, cz) = ((x as i32).div_euclid(cell), (z as i32).div_euclid(cell));
        let zoom = self.zoom(voxel);
        let level = self.lakes.get(&(cx, cz, zoom), || {
            const N: usize = 16;
            let step = cell / N as i32;
            let (x0, z0) = (cx * cell, cz * cell);
            let xs: Vec<f64> = (0..N)
                .map(|i| (x0 + i as i32 * step + step / 2) as f64)
                .collect();
            let zs: Vec<f64> = (0..N)
                .map(|j| (z0 + j as i32 * step + step / 2) as f64)
                .collect();
            let lats: Vec<f64> = zs.iter().map(|&z| self.grid.geographic(0.0, z).0).collect();
            let lons: Vec<f64> = xs.iter().map(|&x| self.grid.geographic(x, 0.0).1).collect();
            let mut codes = vec![0u8; N * N];
            let overview = self.overview(step.max(1), zs[0]);
            if self
                .cover
                .codes(&lats, &lons, overview, &mut codes)
                .is_err()
            {
                return Ok(None);
            }
            let window = self.elevation[zoom as usize].window(xs[0], zs[0], xs[N - 1], zs[N - 1]);
            let mut heights: Vec<f64> = Vec::new();
            for j in 0..N {
                for i in 0..N {
                    if codes[j * N + i] == worldcover::WATER {
                        heights.push(window.sample(xs[i], zs[j]).metres);
                    }
                }
            }
            if heights.is_empty() {
                return Ok(None);
            }
            // A low quantile, not the lowest: one bad pixel shouldn't drain a lake.
            heights.sort_by(f64::total_cmp);
            Ok(Some(heights[heights.len() / 10]))
        });
        level.ok().and_then(|l| *l)
    }

    /// A dry, gentle place to arrive near the home point.
    pub fn spawn(&self) -> (i32, i32, i32) {
        let (hx, hz) = self.home();
        for ring in 0..=64 {
            let r = ring * 32;
            let points: Vec<(i32, i32)> = if r == 0 {
                vec![(0, 0)]
            } else {
                (0..(ring * 8))
                    .map(|k| {
                        let a = k as f64 / (ring * 8) as f64 * std::f64::consts::TAU;
                        ((a.cos() * r as f64) as i32, (a.sin() * r as f64) as i32)
                    })
                    .collect()
            };
            for (dx, dz) in points {
                let (x, z) = (hx + dx, hz + dz);
                let c = self.region(x, z, 1, 1)[0];
                if c.water.is_none() && c.surface.top != Block::Stone {
                    return (x, c.ground + 1, z);
                }
            }
        }
        let c = self.region(hx, hz, 1, 1)[0];
        (hx, c.ground.max(c.water.unwrap_or(c.ground)) + 1, hz)
    }

    /// Fetches what the chunks within `near` blocks of (`x`, `z`) and far terrain out to `far`
    /// blocks read, every source at once, so generation never waits on a download.
    pub fn prepare(&self, x: i32, z: i32, near: i32, far: i32) -> Prepared {
        let near_first = self.prepare_area(x, z, near, near);
        let mut rest = self.prepare_area(x, z, near, far);
        rest.pieces += near_first.pieces;
        rest.failed += near_first.failed;
        rest.first_failure = near_first.first_failure.or(rest.first_failure);
        rest
    }

    /// Fetches only what the chunks within `near` blocks read, so a player can arrive before the
    /// far terrain's data is in; [`Earth::prepare`] does this first and then the rest.
    pub fn prepare_near(&self, x: i32, z: i32, near: i32) -> Prepared {
        self.prepare_area(x, z, near, near)
    }

    fn prepare_area(&self, x: i32, z: i32, near: i32, far: i32) -> Prepared {
        let (x, z) = (x as f64, z as f64);
        let mut tiles = Vec::new();
        // Each far level's zoom out to the reach it is drawn at, as Earth Platform's prepare.
        let mut voxel = 1;
        while voxel <= 512 {
            let reach = if voxel == 1 {
                near as f64
            } else {
                (near as f64).max(275.0 * voxel as f64).min(far as f64)
            };
            let dem = &self.elevation[self.zoom(voxel) as usize];
            tiles.extend(dem.tiles_over(x - reach, z - reach, x + reach, z + reach));
            if voxel == 1 && dem.zoom() > terrarium::REFERENCE_ZOOM + 1 {
                tiles.extend(
                    self.elevation[terrarium::REFERENCE_ZOOM as usize].tiles_over(
                        x - reach,
                        z - reach,
                        x + reach,
                        z + reach,
                    ),
                );
            }
            voxel *= 2;
        }
        let bathy = (far as f64).min(8800.0).max(near as f64);
        tiles.extend(
            self.elevation[elevation::BATHYMETRY_ZOOM as usize].tiles_over(
                x - bathy,
                z - bathy,
                x + bathy,
                z + bathy,
            ),
        );
        tiles.sort_unstable();
        tiles.dedup();
        let mut cover = Vec::new();
        for (overview, reach) in [
            (0u8, near as f64 + 64.0),
            (2, (far as f64).min(7000.0)),
            (4, (far as f64).min(48000.0)),
        ] {
            if reach <= near as f64 && overview > 0 {
                continue;
            }
            let (north, west) = self.grid.geographic(x - reach, z - reach);
            let (south, east) = self.grid.geographic(x + reach, z + reach);
            for p in self.cover.blocks_over(south, west, north, east, overview) {
                cover.push((overview, p));
            }
        }
        let failures = std::sync::Mutex::new((0usize, None::<String>));
        let fail = |what: String, e: Missing| {
            if e != Missing::Absent {
                let mut f = failures.lock().unwrap();
                f.0 += 1;
                f.1.get_or_insert(format!("{what}: {e}"));
            }
        };
        let fail = &fail;
        let mut jobs: Vec<Box<dyn FnOnce() + Send + '_>> = Vec::new();
        let climate_reach = (far as f64).min(24_000.0).max(near as f64 + 64.0);
        let (north, west) = self.grid.geographic(x - climate_reach, z - climate_reach);
        let (south, east) = self.grid.geographic(x + climate_reach, z + climate_reach);
        let threads = self.settings.transfers;
        jobs.push(Box::new(move || {
            if let Err(e) = self.climate.prefetch(south, west, north, east, threads) {
                fail("climate".into(), e);
            }
        }));
        for (overview, (lat, lon)) in cover.iter().copied() {
            jobs.push(Box::new(move || {
                if let Err(e) = self.cover.code(lat, lon, overview) {
                    fail("land cover".into(), e);
                }
            }));
        }
        // Finest tiles first: the chunks round the player read them.
        tiles.sort_by_key(|t| std::cmp::Reverse(t.0));
        for (zz, tx, ty) in tiles.iter().copied() {
            jobs.push(Box::new(move || {
                if let Err(e) = self.tiles.tile(zz, tx, ty) {
                    fail(format!("elevation tile {zz}/{tx}/{ty}"), e);
                }
            }));
        }
        let pieces = jobs.len();
        // Jobs run from the back of the list.
        jobs.reverse();
        fetch::run_all(jobs, threads);
        let (failed, first_failure) = failures.into_inner().unwrap();
        Prepared {
            pieces,
            failed,
            first_failure,
        }
    }
}

/// Sea-level climate of a point.
#[derive(Clone, Copy, Debug)]
struct SeaClimate {
    koppen: u16,
    mean: f64,
    warm: f64,
    rain: f64,
}
/// Climate at sea level on a square lattice over a region, bilinear between its points.
struct Lattice {
    x0: f64,
    z0: f64,
    step: f64,
    across: usize,
    points: Vec<Option<SeaClimate>>,
}
impl Lattice {
    fn new(earth: &Earth, xl: f64, zl: f64, xh: f64, zh: f64, step: f64) -> Self {
        let x0 = (xl / step).floor() * step;
        let z0 = (zl / step).floor() * step;
        let across = ((xh - x0) / step).floor() as usize + 2;
        let down = ((zh - z0) / step).floor() as usize + 2;
        let mut points = Vec::with_capacity(across * down);
        for j in 0..down {
            for i in 0..across {
                let (lat, lon) = earth
                    .grid
                    .geographic(x0 + i as f64 * step, z0 + j as f64 * step);
                let c = earth.climate.at(lat, lon, 0.0).unwrap_or_else(|e| {
                    log::warn!("Earth: climate at {lat:.3} {lon:.3}: {e}");
                    None
                });
                points.push(c.map(|c| SeaClimate {
                    koppen: c.koppen,
                    mean: c.mean,
                    warm: c.warmest_quarter,
                    rain: c.rain,
                }));
            }
        }
        Self {
            x0,
            z0,
            step,
            across,
            points,
        }
    }
    fn at(&self, x: f64, z: f64) -> Option<SeaClimate> {
        let (fx, fz) = ((x - self.x0) / self.step, (z - self.z0) / self.step);
        let (i, j) = (fx.floor() as usize, fz.floor() as usize);
        let (tx, tz) = (fx - i as f64, fz - j as f64);
        let (mut w, mut mean, mut warm, mut rain) = (0.0, 0.0, 0.0, 0.0);
        let mut nearest = (f64::MIN, 0);
        for (dj, dz) in [(0, 1.0 - tz), (1, tz)] {
            for (di, dx) in [(0, 1.0 - tx), (1, tx)] {
                let weight = dz * dx;
                if let Some(Some(c)) = self.points.get((j + dj) * self.across + i + di) {
                    w += weight;
                    mean += weight * c.mean;
                    warm += weight * c.warm;
                    rain += weight * c.rain;
                    if weight > nearest.0 {
                        nearest = (weight, c.koppen);
                    }
                }
            }
        }
        (w > 0.0).then(|| SeaClimate {
            koppen: nearest.1,
            mean: mean / w,
            warm: warm / w,
            rain: rain / w,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heights_fit_the_world() {
        let v = Vertical::new(-64, 384);
        assert_eq!(v.blocks(50.0), 50.0);
        let everest = v.blocks(8_848.0);
        assert!(
            (everest - (320.0 - 1.0 - 62.0 - 32.0)).abs() < 0.5,
            "{everest}"
        );
        assert!(v.blocks(4_000.0) < everest && v.blocks(4_000.0) > v.blocks(2_000.0));
        assert!(SEA as f64 + v.blocks(-10_935.0) >= -64.0 + 5.0);
        let tall = Vertical::new(-2032, 4064);
        assert_eq!(tall.blocks(1_400.0), 1_400.0);
        assert!(SEA as f64 + tall.blocks(8_848.0) <= 2031.0 - 31.0);
    }
}
