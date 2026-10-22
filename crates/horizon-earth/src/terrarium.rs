//! Global elevation from Terrarium Web Mercator PNG tiles (Earth Platform's `terrarium-v1`).
//!
//! The source is the public AWS Open Data Terrain Tiles bucket (Mapzen/Tilezen's composite of
//! SRTM, GMTED, ETOPO1, 3DEP and national surveys). Each pixel encodes
//! `(R * 256 + G + B / 256) - 32768` metres. Tiles load once however many workers want them,
//! are kept decoded in memory and as their PNG bytes on disk.
use crate::fetch::{DiskCache, Flight, Http, Missing};
use std::sync::Arc;

pub const DATASET_ID: &str = "terrarium-v1";
pub const AWS_TERRAIN_TILES: &str = "https://s3.amazonaws.com/elevation-tiles-prod/terrarium/";
pub const TILE: usize = 256;
pub const MAX_ZOOM: u8 = 15;
/// Finer tiles are checked against this zoom: the finest tiles carry runs of broken pixels.
pub const REFERENCE_ZOOM: u8 = 11;
const MAX_TILE_BYTES: usize = 4 << 20;

/// Metres, row-major north to south, 256 × 256.
pub type Tile = Vec<f32>;

pub struct TileSource {
    base: String,
    http: Arc<Http>,
    cache: DiskCache,
    tiles: Flight<(u8, u32, u32), Tile>,
}
impl TileSource {
    pub fn new(base: &str, http: Arc<Http>, cache: DiskCache, tiles_in_memory: usize) -> Self {
        Self {
            base: base.to_owned(),
            http,
            cache,
            tiles: Flight::new(tiles_in_memory),
        }
    }
    pub fn http(&self) -> &Http {
        &self.http
    }
    /// The tile's metres with the composite's broken runs repaired.
    pub fn tile(&self, zoom: u8, x: u32, y: u32) -> Result<Arc<Tile>, Missing> {
        debug_assert!(zoom <= MAX_ZOOM && x < 1 << zoom && y < 1 << zoom);
        self.tiles.get(&(zoom, x, y), || {
            let mut values = self.raw(zoom, x, y)?;
            if zoom > REFERENCE_ZOOM + 1 {
                let shift = zoom - REFERENCE_ZOOM;
                // A missing reference leaves the tile as it is.
                if let Ok(reference) = self.tile(REFERENCE_ZOOM, x >> shift, y >> shift) {
                    repair(&mut values, &reference, shift, x, y);
                }
            }
            Ok(values)
        })
    }
    /// The tile if it is decoded already, without waiting.
    pub fn kept(&self, zoom: u8, x: u32, y: u32) -> Option<Arc<Tile>> {
        self.tiles.peek(&(zoom, x, y))
    }
    fn raw(&self, zoom: u8, x: u32, y: u32) -> Result<Tile, Missing> {
        let relative = format!("{DATASET_ID}/{zoom}/{x}/{y}.png");
        if let Some(bytes) = self.cache.read(&relative) {
            match decode(&bytes) {
                Ok(values) => {
                    self.http
                        .totals
                        .disk_hits
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    return Ok(values);
                }
                // Cached bytes are derived: fetch them again rather than keep a broken tile.
                Err(_) => self.cache.remove(&relative),
            }
        }
        let url = format!("{}{zoom}/{x}/{y}.png", self.base);
        let bytes = self.http.get(&url, None, MAX_TILE_BYTES)?;
        let values = decode(&bytes).map_err(|e| Missing::Failed(format!("{url}: {e}")))?;
        self.cache.write(&relative, &bytes);
        Ok(values)
    }
    /// Whether the disk holds a tile, so prefetching can leave it for later.
    pub fn on_disk(&self, zoom: u8, x: u32, y: u32) -> bool {
        self.cache
            .exists(&format!("{DATASET_ID}/{zoom}/{x}/{y}.png"))
    }
}

/// A Terrarium PNG's metres. 8-bit RGB and RGBA, as the composite serves them, and anything the
/// png crate can expand to those.
pub fn decode(png_bytes: &[u8]) -> Result<Tile, String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("image too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    if info.width as usize != TILE || info.height as usize != TILE {
        return Err(format!(
            "unexpected tile size {}x{}",
            info.width, info.height
        ));
    }
    let stride = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        other => return Err(format!("unexpected colour type {other:?}")),
    };
    let mut out: Tile = vec![0f32; TILE * TILE];
    for (i, px) in buf[..TILE * TILE * stride].chunks_exact(stride).enumerate() {
        out[i] = ((px[0] as f64 * 256.0 + px[1] as f64 + px[2] as f64 / 256.0) - 32768.0) as f32;
    }
    Ok(out)
}

/// Metres a fine pixel may stray from the reference, beyond the reference's own relief nearby,
/// before it counts as broken. Real cliffs and gorges stay inside that relief; broken runs don't.
const BROKEN_METRES: f32 = 150.0;
/// Fine pixels either side along the row and column whose detail a repaired pixel borrows.
const REPAIR_REACH: i32 = 4;

/// Repairs runs of wildly wrong pixels (-8,161 m beside 6,040 m on Aconcagua) in a tile `shift`
/// zooms finer than its reference tile, against the reference's bilinear surface: a broken pixel
/// takes the reference plus the detail its sound neighbours along the row and column show.
/// Exact zeros mark open sea and are left for the bathymetry fill. (Earth Platform's TileRepair.)
pub fn repair(values: &mut Tile, reference: &Tile, shift: u8, x: u32, y: u32) {
    let scale = 1.0 / (1u32 << shift) as f64;
    let span = (TILE >> shift) as u32;
    let mask = (1u32 << shift) - 1;
    let (ox, oy) = ((x & mask) * span, (y & mask) * span);
    let mut expected = vec![0f32; TILE * TILE];
    let mut broken = vec![false; TILE * TILE];
    let mut any = false;
    let limit = TILE as f64 - 1.001;
    for py in 0..TILE {
        let ry = (oy as f64 + (py as f64 + 0.5) * scale - 0.5).clamp(0.0, limit);
        let y0 = ry as usize;
        let fy = ry - y0 as f64;
        for px in 0..TILE {
            let rx = (ox as f64 + (px as f64 + 0.5) * scale - 0.5).clamp(0.0, limit);
            let x0 = rx as usize;
            let fx = rx - x0 as f64;
            let at = y0 * TILE + x0;
            let i = py * TILE + px;
            let r = |k: usize| reference[k] as f64;
            expected[i] = ((r(at) * (1.0 - fx) + r(at + 1) * fx) * (1.0 - fy)
                + (r(at + TILE) * (1.0 - fx) + r(at + TILE + 1) * fx) * fy)
                as f32;
            let (mut low, mut high) = (f32::MAX, f32::MIN);
            for j in y0.saturating_sub(1)..=(y0 + 2).min(TILE - 1) {
                for k in x0.saturating_sub(1)..=(x0 + 2).min(TILE - 1) {
                    let v = reference[j * TILE + k];
                    low = low.min(v);
                    high = high.max(v);
                }
            }
            broken[i] =
                values[i] != 0.0 && (values[i] - expected[i]).abs() > BROKEN_METRES + high - low;
            any |= broken[i];
        }
    }
    if !any {
        return;
    }
    let sound = |values: &Tile, i: usize| !broken[i] && values[i] != 0.0;
    let mut fixed = vec![0f32; TILE * TILE];
    for i in 0..TILE * TILE {
        if !broken[i] {
            continue;
        }
        let (px, py) = ((i % TILE) as i32, (i / TILE) as i32);
        let (mut detail, mut n) = (0f64, 0);
        for d in -REPAIR_REACH..=REPAIR_REACH {
            if d == 0 {
                continue;
            }
            let (qx, qy) = (px + d, py + d);
            if (0..TILE as i32).contains(&qx) {
                let k = (py * TILE as i32 + qx) as usize;
                if sound(values, k) {
                    detail += (values[k] - expected[k]) as f64;
                    n += 1;
                }
            }
            if (0..TILE as i32).contains(&qy) {
                let k = (qy * TILE as i32 + px) as usize;
                if sound(values, k) {
                    detail += (values[k] - expected[k]) as f64;
                    n += 1;
                }
            }
        }
        let value = (expected[i] as f64 + if n == 0 { 0.0 } else { detail / n as f64 }) as f32;
        fixed[i] = if value == 0.0 {
            f32::from_bits(1)
        } else {
            value
        };
    }
    for i in 0..TILE * TILE {
        if broken[i] {
            values[i] = fixed[i];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(metres: impl Fn(usize) -> f64) -> Vec<u8> {
        let mut rgb = Vec::with_capacity(TILE * TILE * 3);
        for i in 0..TILE * TILE {
            let v = metres(i) + 32768.0;
            rgb.extend([
                (v / 256.0) as u8,
                (v as u32 % 256) as u8,
                ((v - v.floor()) * 256.0) as u8,
            ]);
        }
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, TILE as u32, TILE as u32);
        encoder.set_color(png::ColorType::Rgb);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&rgb)
            .unwrap();
        out
    }

    #[test]
    fn tiles_decode_to_metres() {
        let tile = decode(&encode(|i| i as f64 / 7.0 - 400.0)).unwrap();
        for i in [0, 1, 300, 65535] {
            assert!((tile[i] as f64 - (i as f64 / 7.0 - 400.0)).abs() < 1.0 / 256.0 + 1e-6);
        }
    }

    #[test]
    fn broken_runs_take_the_reference_and_cliffs_stay() {
        let reference: Tile = vec![1000f32; TILE * TILE];
        let mut fine: Tile = vec![1000f32; TILE * TILE];
        fine[40 * TILE + 40] = -8000.0;
        fine[41 * TILE + 40] = 1100.0;
        repair(&mut fine, &reference, 4, 3, 5);
        assert!((fine[40 * TILE + 40] - 1000.0).abs() < 50.0);
        assert_eq!(fine[41 * TILE + 40], 1100.0);
    }
}
