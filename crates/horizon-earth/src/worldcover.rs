//! Global land cover from ESA WorldCover 2021 v200 (Earth Platform's `esa-worldcover-v200`): one
//! class per pixel of 1/12000 degree, about 10 m.
//!
//! One cloud-optimised GeoTIFF per 3 × 3 degree square, tiled in 1024-pixel deflated blocks with
//! six halving overviews. Only the header and the blocks touched are read, with range requests;
//! open ocean has no files at all.
use crate::{
    cog,
    fetch::{DiskCache, Flight, Http, Missing},
};
use std::sync::Arc;

pub const DATASET_ID: &str = "esa-worldcover-v200";
pub const AWS_WORLDCOVER: &str =
    "https://esa-worldcover.s3.eu-central-1.amazonaws.com/v200/2021/map/";
pub const TREE_COVER: u8 = 10;
pub const SHRUBLAND: u8 = 20;
pub const GRASSLAND: u8 = 30;
pub const CROPLAND: u8 = 40;
pub const BUILT_UP: u8 = 50;
pub const BARE: u8 = 60;
pub const SNOW_AND_ICE: u8 = 70;
pub const WATER: u8 = 80;
pub const WETLAND: u8 = 90;
pub const MANGROVES: u8 = 95;
pub const MOSS_AND_LICHEN: u8 = 100;
pub const NO_DATA: u8 = 0;
pub const PIXELS_PER_DEGREE: f64 = 12000.0;
const SQUARE: i32 = 3;
pub const MAX_OVERVIEW: u8 = 6;
const BLOCK: usize = 1024;
const HEADER_BYTES: usize = 65536;
const MAX_HEADER_BYTES: usize = 1 << 20;
const MAX_BLOCK_BYTES: usize = 4 << 20;

struct Level {
    width: u32,
    offsets: Vec<u64>,
    counts: Vec<u64>,
}
struct Header {
    levels: Vec<Level>,
    deflate: bool,
}

pub struct WorldCover {
    base: String,
    http: Arc<Http>,
    cache: DiskCache,
    headers: Flight<i32, Option<Header>>,
    blocks: Flight<u64, Vec<u8>>,
}
fn square_key(south: i32, west: i32) -> i32 {
    (south + 90) * 1000 + (west + 180)
}
fn name(south: i32, west: i32) -> String {
    format!(
        "ESA_WorldCover_10m_2021_v200_{}{:02}{}{:03}_Map",
        if south < 0 { 'S' } else { 'N' },
        south.abs(),
        if west < 0 { 'W' } else { 'E' },
        west.abs()
    )
}
fn wrap(longitude: f64) -> f64 {
    let w = (longitude + 180.0).rem_euclid(360.0) - 180.0;
    if w >= 180.0 { -180.0 } else { w }
}
impl WorldCover {
    pub fn new(base: &str, http: Arc<Http>, cache: DiskCache, blocks_in_memory: usize) -> Self {
        Self {
            base: base.to_owned(),
            http,
            cache,
            headers: Flight::new(4096),
            blocks: Flight::new(blocks_in_memory),
        }
    }
    fn header(&self, south: i32, west: i32) -> Result<Arc<Option<Header>>, Missing> {
        self.headers.get(&square_key(south, west), || {
            let stem = name(south, west);
            let (cached, none) = (
                format!("{DATASET_ID}/{stem}.tif.header"),
                format!("{DATASET_ID}/{stem}.tif.none"),
            );
            if self.cache.exists(&none) {
                return Ok(None);
            }
            if let Some(bytes) = self.cache.read(&cached) {
                if let Ok(header) = parse(&bytes) {
                    return Ok(Some(header));
                }
                self.cache.remove(&cached);
            }
            let url = format!("{}{stem}.tif", self.base);
            let mut size = HEADER_BYTES;
            loop {
                let bytes = match self.http.get(&url, Some((0, size as u64)), size) {
                    Err(Missing::Absent) => {
                        self.cache.write(&none, &[]);
                        return Ok(None);
                    }
                    other => other?,
                };
                match parse(&bytes) {
                    Ok(header) => {
                        self.cache.write(&cached, &bytes);
                        return Ok(Some(header));
                    }
                    Err(cog::TiffError::TooShort(_))
                        if bytes.len() == size && size < MAX_HEADER_BYTES =>
                    {
                        size *= 4
                    }
                    Err(e) => return Err(Missing::Failed(format!("land cover {stem}: {e}"))),
                }
            }
        })
    }
    /// The decoded block at `row`, `column` of an overview of a square; empty where there is no data.
    fn block(
        &self,
        south: i32,
        west: i32,
        overview: u8,
        row: u32,
        column: u32,
    ) -> Result<Arc<Vec<u8>>, Missing> {
        let key = (square_key(south, west) as u64) << 32
            | (overview as u64) << 28
            | (row as u64) << 14
            | column as u64;
        if let Some(hit) = self.blocks.peek(&key) {
            return Ok(hit);
        }
        let header = self.header(south, west)?;
        self.blocks.get(&key, || {
            let Some(header) = header.as_ref() else {
                return Ok(Vec::new());
            };
            let level = &header.levels[overview as usize];
            let across = level.width.div_ceil(BLOCK as u32);
            let index = (row * across + column) as usize;
            if index >= level.offsets.len() {
                return Ok(Vec::new());
            }
            let stem = name(south, west);
            let cached = format!("{DATASET_ID}/{stem}/{overview}/{index}.z");
            if let Some(raw) = self.cache.read(&cached) {
                if let Ok(decoded) = decode(&raw, header.deflate) {
                    return Ok(decoded);
                }
                self.cache.remove(&cached);
            }
            let (offset, count) = (level.offsets[index], level.counts[index]);
            if offset == 0 || count == 0 {
                return Ok(vec![NO_DATA; BLOCK * BLOCK]);
            }
            if count as usize > MAX_BLOCK_BYTES {
                return Err(Missing::Failed(format!(
                    "oversized land cover block in {stem}"
                )));
            }
            let raw = self.http.get(
                &format!("{}{stem}.tif", self.base),
                Some((offset, count)),
                count as usize,
            )?;
            let decoded = decode(&raw, header.deflate)
                .map_err(|e| Missing::Failed(format!("land cover {stem}: {e}")))?;
            self.cache.write(&cached, &raw);
            Ok(decoded)
        })
    }
    /// Where a position falls: its square and the pixel in an overview of it.
    fn locate(latitude: f64, longitude: f64, overview: u8) -> (i32, i32, u64, u64) {
        let longitude = wrap(longitude);
        let mut south = (latitude.floor() as i32).div_euclid(SQUARE) * SQUARE;
        let west = (longitude.floor() as i32).div_euclid(SQUARE) * SQUARE;
        if south >= 90 {
            south = 90 - SQUARE;
        }
        let full = (SQUARE as f64 * PIXELS_PER_DEGREE) as u64;
        let fx = (longitude - west as f64) * PIXELS_PER_DEGREE;
        let fy = (south as f64 + SQUARE as f64 - latitude) * PIXELS_PER_DEGREE;
        // Overviews round down: the last full pixels fold into the last overview pixel.
        let last = (full >> overview) - 1;
        let px = last.min((fx.max(0.0) as u64).min(full - 1) >> overview);
        let py = last.min((fy.max(0.0) as u64).min(full - 1) >> overview);
        (south, west, px, py)
    }
    /// The class at a position, read at an overview (0 is full resolution, each step halves it).
    pub fn code(&self, latitude: f64, longitude: f64, overview: u8) -> Result<u8, Missing> {
        let (south, west, px, py) = Self::locate(latitude, longitude, overview);
        let block = self.block(south, west, overview, (py >> 10) as u32, (px >> 10) as u32)?;
        Ok(if block.is_empty() {
            NO_DATA
        } else {
            block[((py & 1023) << 10 | (px & 1023)) as usize]
        })
    }
    /// The classes over a grid: `latitudes` by row, `longitudes` by column, into `out` row by row.
    /// A block is looked up only where the grid crosses into one.
    pub fn codes(
        &self,
        latitudes: &[f64],
        longitudes: &[f64],
        overview: u8,
        out: &mut [u8],
    ) -> Result<(), Missing> {
        let mut last: Option<((i32, i32, u64, u64), Arc<Vec<u8>>)> = None;
        for (j, &lat) in latitudes.iter().enumerate() {
            for (i, &lon) in longitudes.iter().enumerate() {
                let (south, west, px, py) = Self::locate(lat, lon, overview);
                let key = (south, west, py >> 10, px >> 10);
                let block = match &last {
                    Some((k, b)) if *k == key => b.clone(),
                    _ => {
                        let b = self.block(south, west, overview, key.2 as u32, key.3 as u32)?;
                        last = Some((key, b.clone()));
                        b
                    }
                };
                out[j * longitudes.len() + i] = if block.is_empty() {
                    NO_DATA
                } else {
                    block[((py & 1023) << 10 | (px & 1023)) as usize]
                };
            }
        }
        Ok(())
    }
    /// Every block an area needs at an overview, as jobs for a prefetch.
    pub fn blocks_over(
        &self,
        south: f64,
        west: f64,
        north: f64,
        east: f64,
        overview: u8,
    ) -> Vec<(f64, f64)> {
        let step = BLOCK as f64 * (1u32 << overview) as f64 / PIXELS_PER_DEGREE;
        let mut points = Vec::new();
        let mut lat = south.max(-89.99);
        while lat <= north.min(89.99) + step {
            let mut lon = west;
            while lon <= east + step {
                points.push((lat.min(north.min(89.99)), lon.min(east)));
                lon += step;
            }
            lat += step;
        }
        points
    }
}
fn decode(raw: &[u8], deflate: bool) -> Result<Vec<u8>, String> {
    if deflate {
        cog::inflate(raw, BLOCK * BLOCK)
    } else if raw.len() == BLOCK * BLOCK {
        Ok(raw.to_vec())
    } else {
        Err("unexpected block size".into())
    }
}
fn parse(bytes: &[u8]) -> Result<Header, cog::TiffError> {
    let tiff = cog::parse(bytes, MAX_OVERVIEW as usize + 1)?;
    let mut levels = Vec::new();
    let mut deflate = true;
    for d in tiff.directories {
        if d.bits != 8
            || d.samples != 1
            || d.predictor != 1
            || !matches!(d.compression, 1 | 8 | 32946)
        {
            return Err(cog::TiffError::Unsupported(
                "unsupported land cover encoding".into(),
            ));
        }
        let blocks = d.width.div_ceil(1024) as usize * d.height.div_ceil(1024) as usize;
        if d.tile_width != 1024
            || d.tile_height != 1024
            || d.offsets.len() != blocks
            || d.counts.len() != blocks
        {
            return Err(cog::TiffError::Unsupported(
                "unsupported land cover layout".into(),
            ));
        }
        deflate = d.compression != 1;
        levels.push(Level {
            width: d.width,
            offsets: d.offsets,
            counts: d.counts,
        });
    }
    if levels.len() <= MAX_OVERVIEW as usize {
        return Err(cog::TiffError::TooShort(bytes.len() * 4));
    }
    Ok(Header { levels, deflate })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squares_are_named_as_the_bucket_names_them() {
        assert_eq!(name(36, -120), "ESA_WorldCover_10m_2021_v200_N36W120_Map");
        assert_eq!(name(-6, -63), "ESA_WorldCover_10m_2021_v200_S06W063_Map");
        let (south, west, px, py) = WorldCover::locate(37.724, -119.6365, 0);
        assert_eq!((south, west), (36, -120));
        assert_eq!(px, ((-119.6365f64 + 120.0) * 12000.0) as u64);
        assert_eq!(py, ((39.0 - 37.724) * 12000.0) as u64);
    }
}
