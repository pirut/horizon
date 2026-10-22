//! Present-day climate from CHELSA V2.1 1981-2010 (Earth Platform's `chelsa-v2.1-1981-2010`):
//! 30 arc-second (about 1 km) climatologies, CC0. For a place it gives the Köppen-Geiger class,
//! mean yearly and warmest-quarter temperatures and yearly precipitation, temperatures carried
//! from each pixel's own height to the one asked for at the standard lapse rate.
//!
//! One deflated GeoTIFF per variable with a strip per pixel row, and CHELSA's elevation grid as
//! raw floats. Only the headers and rows touched are read, rows close together in one request.
use crate::{
    cog,
    fetch::{DiskCache, Flight, Http, Missing},
};
use std::sync::Arc;

pub const DATASET_ID: &str = "chelsa-v2.1-1981-2010";
pub const CHELSA: &str = "https://os.zhdk.cloud.switch.ch/chelsav2/GLOBAL/";
pub const LAPSE_RATE: f64 = 0.0065;
const PIXELS_PER_DEGREE: f64 = 120.0;
const WIDTH: usize = 43200;
const HEIGHT: usize = 20880;
const NORTH: f64 = 84.0;
const WEST: f64 = -180.0;
/// Columns kept together: a row is 43,200 pixels round the globe.
const CHUNK: usize = 2048;
const GRAVITY: f64 = 9.80665;
const HEADER_BYTES: usize = 256 << 10;
const MAX_HEADER_BYTES: usize = 2 << 20;
const MAX_ROW_BYTES: u64 = 1 << 20;
const MAX_RUN_BYTES: u64 = 8 << 20;
const MAX_GAP_BYTES: u64 = 256 << 10;
const DEM_FILE: &str = "input/dem_geo.sdat";

/// Köppen-Geiger classes as CHELSA numbers them.
pub mod koppen {
    pub const AF: u16 = 1;
    pub const AM: u16 = 2;
    pub const AS: u16 = 3;
    pub const AW: u16 = 4;
    pub const BWK: u16 = 5;
    pub const BWH: u16 = 6;
    pub const BSK: u16 = 7;
    pub const BSH: u16 = 8;
    pub const CFA: u16 = 9;
    pub const CFB: u16 = 10;
    pub const CFC: u16 = 11;
    pub const CSA: u16 = 12;
    pub const CSB: u16 = 13;
    pub const CSC: u16 = 14;
    pub const CWA: u16 = 15;
    pub const CWB: u16 = 16;
    pub const CWC: u16 = 17;
    pub const DFA: u16 = 18;
    pub const DFB: u16 = 19;
    pub const DFC: u16 = 20;
    pub const DFD: u16 = 21;
    pub const DSA: u16 = 22;
    pub const DSB: u16 = 23;
    pub const DSC: u16 = 24;
    pub const DSD: u16 = 25;
    pub const DWA: u16 = 26;
    pub const DWB: u16 = 27;
    pub const DWC: u16 = 28;
    pub const DWD: u16 = 29;
    pub const ET: u16 = 30;
    pub const EF: u16 = 31;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Layer {
    Koppen,
    Mean,
    Warm,
    Rain,
}
const LAYERS: [Layer; 4] = [Layer::Koppen, Layer::Mean, Layer::Warm, Layer::Rain];
impl Layer {
    fn stem(self) -> &'static str {
        match self {
            Self::Koppen => "CHELSA_kg2_1981-2010_V.2.1",
            Self::Mean => "CHELSA_bio1_1981-2010_V.2.1",
            Self::Warm => "CHELSA_bio10_1981-2010_V.2.1",
            Self::Rain => "CHELSA_bio12_1981-2010_V.2.1",
        }
    }
    fn file(self) -> String {
        format!("climatologies/1981-2010/bio/{}.tif", self.stem())
    }
}

/// Climate at one place and height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Climate {
    pub koppen: u16,
    /// Degrees C.
    pub mean: f64,
    pub warmest_quarter: f64,
    /// mm a year.
    pub rain: f64,
}

struct Header {
    offsets: Vec<u64>,
    counts: Vec<u64>,
    deflate: bool,
    predictor: bool,
    little: bool,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Piece {
    Row(Layer, u32, u32),
    Dem(u32, u32),
}

pub struct ClimateSource {
    base: String,
    http: Arc<Http>,
    cache: DiskCache,
    headers: Flight<Layer, Header>,
    rows: Flight<Piece, Vec<u16>>,
    dem: Flight<Piece, Vec<f32>>,
}
fn wrap(longitude: f64) -> f64 {
    let w = (longitude + 180.0).rem_euclid(360.0) - 180.0;
    if w >= 180.0 { -180.0 } else { w }
}
impl ClimateSource {
    pub fn new(base: &str, http: Arc<Http>, cache: DiskCache, rows_in_memory: usize) -> Self {
        Self {
            base: base.to_owned(),
            http,
            cache,
            headers: Flight::new(8),
            rows: Flight::new(rows_in_memory * 4),
            dem: Flight::new(rows_in_memory),
        }
    }
    fn header(&self, layer: Layer) -> Result<Arc<Header>, Missing> {
        self.headers.get(&layer, || {
            let cached = format!("{DATASET_ID}/{}.header", layer.stem());
            if let Some(bytes) = self.cache.read(&cached) {
                if let Ok(header) = parse(&bytes) {
                    return Ok(header);
                }
                self.cache.remove(&cached);
            }
            let url = format!("{}{}", self.base, layer.file());
            let mut size = HEADER_BYTES;
            loop {
                let bytes = self.http.get(&url, Some((0, size as u64)), size)?;
                match parse(&bytes) {
                    Ok(header) => {
                        self.cache.write(&cached, &bytes);
                        return Ok(header);
                    }
                    Err(cog::TiffError::TooShort(_))
                        if bytes.len() == size && size < MAX_HEADER_BYTES =>
                    {
                        size *= 2
                    }
                    Err(e) => {
                        return Err(Missing::Failed(format!("climate {}: {e}", layer.stem())));
                    }
                }
            }
        })
    }
    fn row_cache(layer: Layer, row: u32) -> String {
        format!("{DATASET_ID}/{}/{row}.z", layer.stem())
    }
    /// Columns `chunk * CHUNK ..` of a row of a layer.
    fn chunk(&self, layer: Layer, row: u32, chunk: u32) -> Result<Arc<Vec<u16>>, Missing> {
        let key = Piece::Row(layer, row, chunk);
        if let Some(hit) = self.rows.peek(&key) {
            return Ok(hit);
        }
        self.rows.get(&key, || {
            let header = self.header(layer)?;
            let cached = Self::row_cache(layer, row);
            let decoded = match self.cache.read(&cached).map(|raw| decode(&raw, &header)) {
                Some(Ok(decoded)) => decoded,
                other => {
                    if other.is_some() {
                        self.cache.remove(&cached);
                    }
                    let (offset, count) =
                        (header.offsets[row as usize], header.counts[row as usize]);
                    if count == 0 || count > MAX_ROW_BYTES {
                        return Err(Missing::Failed(format!(
                            "unreadable climate row {row} of {}",
                            layer.stem()
                        )));
                    }
                    let url = format!("{}{}", self.base, layer.file());
                    let raw = self.http.get(&url, Some((offset, count)), count as usize)?;
                    let decoded = decode(&raw, &header).map_err(Missing::Failed)?;
                    self.cache.write(&cached, &raw);
                    decoded
                }
            };
            let from = chunk as usize * CHUNK;
            Ok(decoded[from..(from + CHUNK).min(WIDTH)].to_vec())
        })
    }
    fn value(&self, layer: Layer, row: u32, column: u32) -> Result<u16, Missing> {
        let chunk = column / CHUNK as u32;
        Ok(self.chunk(layer, row, chunk)?[(column - chunk * CHUNK as u32) as usize])
    }
    /// Height in metres of a pixel in CHELSA's elevation grid (sea level where it has none).
    fn height(&self, row: u32, column: u32) -> Result<f64, Missing> {
        let chunk = column / CHUNK as u32;
        let key = Piece::Dem(row, chunk);
        let values = self.dem.get(&key, || {
            let from = chunk as usize * CHUNK;
            let size = (from + CHUNK).min(WIDTH) - from;
            let cached = format!("{DATASET_ID}/dem/{row}/{chunk}.f");
            let raw = match self.cache.read(&cached).filter(|r| r.len() == size * 4) {
                Some(raw) => raw,
                None => {
                    // The grid is stored from its south edge up.
                    let offset = ((HEIGHT - 1 - row as usize) * WIDTH + from) as u64 * 4;
                    let raw = self.http.get(
                        &format!("{}{DEM_FILE}", self.base),
                        Some((offset, size as u64 * 4)),
                        size * 4,
                    )?;
                    if raw.len() != size * 4 {
                        return Err(Missing::Failed(format!(
                            "climate elevation row {row} is short"
                        )));
                    }
                    self.cache.write(&cached, &raw);
                    raw
                }
            };
            Ok(raw
                .chunks_exact(4)
                .map(|b| {
                    let geopotential = f32::from_le_bytes(b.try_into().unwrap());
                    if geopotential <= -99998.0 || !geopotential.is_finite() {
                        0.0
                    } else {
                        (geopotential as f64 / GRAVITY) as f32
                    }
                })
                .collect())
        })?;
        Ok(values[(column - chunk * CHUNK as u32) as usize] as f64)
    }
    /// The climate at a position, `metres` above sea level, or None where the data has a gap.
    pub fn at(
        &self,
        latitude: f64,
        longitude: f64,
        metres: f64,
    ) -> Result<Option<Climate>, Missing> {
        let longitude = wrap(longitude);
        let fx = (longitude - WEST) * PIXELS_PER_DEGREE - 0.5;
        let fy = (NORTH - latitude) * PIXELS_PER_DEGREE - 0.5;
        let (x0, y0) = (fx.floor() as i64, fy.floor() as i64);
        let (tx, ty) = (fx - x0 as f64, fy - y0 as f64);
        let (mut weights, mut mean, mut warm, mut rain) = (0.0, 0.0, 0.0, 0.0);
        for dy in 0..2 {
            let row = (y0 + dy).clamp(0, HEIGHT as i64 - 1) as u32;
            for dx in 0..2 {
                let column = (x0 + dx).rem_euclid(WIDTH as i64) as u32;
                let m = self.value(Layer::Mean, row, column)?;
                let w = self.value(Layer::Warm, row, column)?;
                let r = self.value(Layer::Rain, row, column)?;
                if m == 0 || m == 0xFFFF || w == 0 || w == 0xFFFF || r == 0xFFFF {
                    continue;
                }
                let weight =
                    (if dx == 0 { 1.0 - tx } else { tx }) * (if dy == 0 { 1.0 - ty } else { ty });
                if weight <= 0.0 {
                    continue;
                }
                // Each pixel's temperatures brought to sea level, to be lifted to the height asked.
                let sea = LAPSE_RATE * self.height(row, column)?;
                weights += weight;
                mean += weight * (kelvin(m) + sea);
                warm += weight * (kelvin(w) + sea);
                rain += weight * r as f64 * 0.1;
            }
        }
        if weights <= 0.0 {
            return Ok(None);
        }
        let row = (fy + 0.5).floor().clamp(0.0, HEIGHT as f64 - 1.0) as u32;
        let column = ((fx + 0.5).floor() as i64).rem_euclid(WIDTH as i64) as u32;
        let mut class = self.value(Layer::Koppen, row, column)?;
        if class > koppen::EF {
            class = 0;
        }
        Ok(Some(Climate {
            koppen: class,
            mean: mean / weights - LAPSE_RATE * metres,
            warmest_quarter: warm / weights - LAPSE_RATE * metres,
            rain: rain / weights,
        }))
    }
    /// Reads every row an area needs into the disk cache, rows close together in a file in one
    /// request, then decodes the pieces the area reads. Needs a disk cache to batch; without one
    /// rows load one by one as they are read.
    pub fn prefetch(
        &self,
        south: f64,
        west: f64,
        north: f64,
        east: f64,
        threads: usize,
    ) -> Result<(), Missing> {
        let top = (((NORTH - north.min(90.0)) * PIXELS_PER_DEGREE).floor() as i64 - 1)
            .clamp(0, HEIGHT as i64 - 1) as u32;
        let bottom = (((NORTH - south.max(-90.0)) * PIXELS_PER_DEGREE).floor() as i64 + 1)
            .clamp(0, HEIGHT as i64 - 1) as u32;
        let first = ((wrap(west) - WEST) * PIXELS_PER_DEGREE).floor() as i64 - 1;
        let columns = ((east - west).min(360.0) * PIXELS_PER_DEGREE).ceil() as i64 + 3;
        let mut chunks: Vec<u32> = (0..columns)
            .step_by(CHUNK / 2)
            .chain([columns - 1])
            .map(|c| ((first + c).rem_euclid(WIDTH as i64) as usize / CHUNK) as u32)
            .collect();
        chunks.sort_unstable();
        chunks.dedup();
        // The four headers at once: each is a round trip before its rows can be read.
        let loaded: Vec<_> = std::thread::scope(|s| {
            let handles: Vec<_> = LAYERS
                .map(|layer| s.spawn(move || (layer, self.header(layer))))
                .into();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let mut headers = Vec::new();
        for (layer, header) in loaded {
            headers.push((layer, header?));
        }
        // Runs of rows missing from the disk, each one request.
        let mut runs = Vec::new();
        for (layer, header) in &headers {
            let mut missing: Vec<u32> = (top..=bottom)
                .filter(|&row| {
                    let count = header.counts[row as usize];
                    count > 0
                        && count <= MAX_ROW_BYTES
                        && !self.cache.exists(&Self::row_cache(*layer, row))
                })
                .collect();
            if self.cache.path("").is_none() {
                missing.clear();
            }
            missing.sort_by_key(|&row| header.offsets[row as usize]);
            let mut i = 0;
            while i < missing.len() {
                let begin = i;
                let start = header.offsets[missing[i] as usize];
                let mut end = start + header.counts[missing[i] as usize];
                i += 1;
                while i < missing.len() {
                    let offset = header.offsets[missing[i] as usize];
                    let next = end.max(offset + header.counts[missing[i] as usize]);
                    if offset.saturating_sub(end) > MAX_GAP_BYTES || next - start > MAX_RUN_BYTES {
                        break;
                    }
                    end = next;
                    i += 1;
                }
                runs.push((
                    *layer,
                    header.clone(),
                    missing[begin..i].to_vec(),
                    start,
                    end,
                ));
            }
        }
        let failed = std::sync::Mutex::new(None);
        let mut jobs: Vec<Box<dyn FnOnce() + Send + '_>> = Vec::new();
        for (layer, header, rows, start, end) in runs {
            let failed = &failed;
            jobs.push(Box::new(move || {
                let url = format!("{}{}", self.base, layer.file());
                match self
                    .http
                    .get(&url, Some((start, end - start)), (end - start) as usize)
                {
                    Ok(run) if run.len() as u64 == end - start => {
                        for row in rows {
                            let from = (header.offsets[row as usize] - start) as usize;
                            let raw = &run[from..from + header.counts[row as usize] as usize];
                            self.cache.write(&Self::row_cache(layer, row), raw);
                        }
                    }
                    Ok(_) => {
                        failed
                            .lock()
                            .unwrap()
                            .get_or_insert(Missing::Failed("short climate run".into()));
                    }
                    Err(e) => {
                        failed.lock().unwrap().get_or_insert(e);
                    }
                }
            }));
        }
        for row in top..=bottom {
            for &chunk in &chunks {
                let failed = &failed;
                jobs.push(Box::new(move || {
                    if let Err(e) = self.height(row, chunk * CHUNK as u32) {
                        failed.lock().unwrap().get_or_insert(e);
                    }
                }));
            }
        }
        crate::fetch::run_all(jobs, threads);
        if let Some(e) = failed.into_inner().unwrap() {
            return Err(e);
        }
        Ok(())
    }
}
fn kelvin(value: u16) -> f64 {
    value as f64 * 0.1 - 273.15
}
fn decode(raw: &[u8], header: &Header) -> Result<Vec<u16>, String> {
    let bytes = if header.deflate {
        cog::inflate(raw, WIDTH * 2)?
    } else if raw.len() == WIDTH * 2 {
        raw.to_vec()
    } else {
        return Err("unexpected climate row size".into());
    };
    let mut previous = 0u16;
    Ok(bytes
        .chunks_exact(2)
        .map(|b| {
            let b = [b[0], b[1]];
            let mut v = if header.little {
                u16::from_le_bytes(b)
            } else {
                u16::from_be_bytes(b)
            };
            // Horizontal differencing stores each value as the step from the one before.
            if header.predictor {
                v = v.wrapping_add(previous);
            }
            previous = v;
            v
        })
        .collect())
}
fn parse(bytes: &[u8]) -> Result<Header, cog::TiffError> {
    let tiff = cog::parse(bytes, 1)?;
    let d = tiff
        .directories
        .into_iter()
        .next()
        .ok_or(cog::TiffError::Unsupported("no image".into()))?;
    let unsupported = |why: &str| Err(cog::TiffError::Unsupported(why.into()));
    if d.bits != 16
        || d.samples != 1
        || !matches!(d.predictor, 1 | 2)
        || !matches!(d.compression, 1 | 8 | 32946)
    {
        return unsupported("unsupported climate encoding");
    }
    if d.width as usize != WIDTH
        || d.height as usize != HEIGHT
        || d.rows_per_strip != 1
        || d.offsets.len() != HEIGHT
        || d.counts.len() != HEIGHT
    {
        return unsupported("unsupported climate layout");
    }
    let (scale, tie) = (&d.pixel_scale, &d.tie_point);
    if scale.len() < 2
        || tie.len() < 6
        || (scale[0] * PIXELS_PER_DEGREE - 1.0).abs() > 1e-6
        || (scale[1] * PIXELS_PER_DEGREE - 1.0).abs() > 1e-6
        || (tie[3] - WEST).abs() > 1e-3
        || (tie[4] - NORTH).abs() > 1e-3
    {
        return unsupported("unexpected climate grid");
    }
    Ok(Header {
        deflate: d.deflated(),
        predictor: d.predictor == 2,
        offsets: d.offsets,
        counts: d.counts,
        little: tiff.little_endian,
    })
}
impl Clone for Header {
    fn clone(&self) -> Self {
        Self {
            offsets: self.offsets.clone(),
            counts: self.counts.clone(),
            deflate: self.deflate,
            predictor: self.predictor,
            little: self.little,
        }
    }
}

/// Broad climate zones the Earth's biomes are chosen by: the Köppen class of a place, moved to
/// the zone its own height gives where that differs from its 1 km pixel's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Rainforest,
    TropicalDry,
    HotDesert,
    ColdDesert,
    HotSteppe,
    ColdSteppe,
    Mediterranean,
    Subtropical,
    Oceanic,
    SubpolarOceanic,
    Continental,
    Subarctic,
    Tundra,
    Ice,
}
impl Zone {
    pub fn of(class: u16, mean: f64, warmest: f64) -> Self {
        use koppen::*;
        if warmest < 0.0 {
            return Self::Ice;
        }
        if warmest < 9.0 {
            return Self::Tundra;
        }
        match class {
            AF | AM => {
                if mean < 18.0 {
                    Self::Oceanic
                } else {
                    Self::Rainforest
                }
            }
            AS | AW => {
                if mean < 18.0 {
                    Self::Oceanic
                } else {
                    Self::TropicalDry
                }
            }
            BWH | BWK => {
                if mean >= 18.0 {
                    Self::HotDesert
                } else {
                    Self::ColdDesert
                }
            }
            BSH | BSK => {
                if mean >= 18.0 {
                    Self::HotSteppe
                } else {
                    Self::ColdSteppe
                }
            }
            CSA | CSB | CSC => Self::Mediterranean,
            DSA | DSB => {
                if mean < 3.0 {
                    Self::Subarctic
                } else {
                    Self::Mediterranean
                }
            }
            CFA | CWA => Self::Subtropical,
            CFB | CWB => Self::Oceanic,
            CFC | CWC => Self::SubpolarOceanic,
            DFA | DFB | DWA | DWB => Self::Continental,
            DFC | DFD | DSC | DSD | DWC | DWD | ET | EF => Self::Subarctic,
            _ if mean >= 18.0 => Self::TropicalDry,
            _ if mean >= 10.0 => Self::Subtropical,
            _ if mean >= 0.0 => Self::Continental,
            _ => Self::Subarctic,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heights_move_zones() {
        assert_eq!(Zone::of(koppen::CSB, 12.0, 20.0), Zone::Mediterranean);
        assert_eq!(Zone::of(koppen::CSB, -2.0, 6.0), Zone::Tundra);
        assert_eq!(Zone::of(koppen::AF, 25.0, 27.0), Zone::Rainforest);
        assert_eq!(Zone::of(koppen::BWH, 12.0, 22.0), Zone::ColdDesert);
    }
}
