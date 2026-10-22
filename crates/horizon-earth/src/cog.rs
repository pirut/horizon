//! Just enough classic TIFF to read cloud-optimised GeoTIFFs with range requests: each image
//! directory's size, layout (tiles or strips), encoding and where its pieces are.
use std::fmt;

#[derive(Debug, PartialEq, Eq)]
pub enum TiffError {
    /// The header runs past the bytes read so far: read at least this many and parse again.
    TooShort(usize),
    Unsupported(String),
}
impl fmt::Display for TiffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort(n) => write!(f, "header needs {n} bytes"),
            Self::Unsupported(why) => f.write_str(why),
        }
    }
}

/// One image file directory.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Directory {
    pub width: u32,
    pub height: u32,
    pub bits: u32,
    pub samples: u32,
    pub compression: u32,
    pub predictor: u32,
    pub format: u32,
    pub subfile: u32,
    pub tile_width: u32,
    pub tile_height: u32,
    pub rows_per_strip: u32,
    /// Tile or strip offsets and byte counts.
    pub offsets: Vec<u64>,
    pub counts: Vec<u64>,
    pub pixel_scale: Vec<f64>,
    pub tie_point: Vec<f64>,
}
impl Directory {
    pub fn deflated(&self) -> bool {
        matches!(self.compression, 8 | 32946)
    }
}

pub struct Tiff {
    pub little_endian: bool,
    pub directories: Vec<Directory>,
}

struct Reader<'a> {
    bytes: &'a [u8],
    little: bool,
}
impl Reader<'_> {
    fn need(&self, at: usize, len: usize) -> Result<&[u8], TiffError> {
        self.bytes
            .get(at..at + len)
            .ok_or(TiffError::TooShort((at + len).max(self.bytes.len() * 2)))
    }
    fn u16(&self, at: usize) -> Result<u16, TiffError> {
        let b: [u8; 2] = self.need(at, 2)?.try_into().unwrap();
        Ok(if self.little {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    }
    fn u32(&self, at: usize) -> Result<u32, TiffError> {
        let b: [u8; 4] = self.need(at, 4)?.try_into().unwrap();
        Ok(if self.little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }
    fn f64(&self, at: usize) -> Result<f64, TiffError> {
        let b: [u8; 8] = self.need(at, 8)?.try_into().unwrap();
        Ok(if self.little {
            f64::from_le_bytes(b)
        } else {
            f64::from_be_bytes(b)
        })
    }
    /// An entry's values as integers (SHORT or LONG) or doubles, inline or at their offset.
    fn values(&self, entry: usize) -> Result<Values, TiffError> {
        let kind = self.u16(entry + 2)?;
        let count = self.u32(entry + 4)? as usize;
        if count > 1 << 22 {
            return Err(TiffError::Unsupported("oversized TIFF array".into()));
        }
        let size = match kind {
            3 => 2,
            4 => 4,
            12 => 8,
            _ => 1,
        };
        let at = if count * size <= 4 {
            entry + 8
        } else {
            self.u32(entry + 8)? as usize
        };
        self.need(at, count * size)?;
        Ok(match kind {
            3 => Values::Int(
                (0..count)
                    .map(|i| self.u16(at + i * 2).map(u64::from))
                    .collect::<Result<_, _>>()?,
            ),
            4 => Values::Int(
                (0..count)
                    .map(|i| self.u32(at + i * 4).map(u64::from))
                    .collect::<Result<_, _>>()?,
            ),
            12 => Values::Float(
                (0..count)
                    .map(|i| self.f64(at + i * 8))
                    .collect::<Result<_, _>>()?,
            ),
            _ => Values::Int(Vec::new()),
        })
    }
}
enum Values {
    Int(Vec<u64>),
    Float(Vec<f64>),
}
impl Values {
    fn ints(self) -> Vec<u64> {
        match self {
            Self::Int(v) => v,
            Self::Float(v) => v.into_iter().map(|f| f as u64).collect(),
        }
    }
    fn first(self) -> u32 {
        self.ints().first().copied().unwrap_or(0) as u32
    }
    fn floats(self) -> Vec<f64> {
        match self {
            Self::Float(v) => v,
            Self::Int(v) => v.into_iter().map(|i| i as f64).collect(),
        }
    }
}

/// Parses up to `most` directories of a classic TIFF from its first bytes.
pub fn parse(bytes: &[u8], most: usize) -> Result<Tiff, TiffError> {
    if bytes.len() < 8 {
        return Err(TiffError::TooShort(8));
    }
    let little = bytes[0] == b'I';
    let r = Reader { bytes, little };
    if r.u16(2)? != 42 {
        return Err(TiffError::Unsupported("not a classic TIFF".into()));
    }
    let mut next = r.u32(4)? as usize;
    let mut directories = Vec::new();
    while next != 0 && directories.len() < most {
        let entries = r.u16(next)? as usize;
        r.need(next, 2 + entries * 12 + 4)?;
        let mut d = Directory {
            compression: 1,
            predictor: 1,
            format: 1,
            samples: 1,
            ..Default::default()
        };
        for e in 0..entries {
            let at = next + 2 + e * 12;
            match r.u16(at)? {
                254 => d.subfile = r.values(at)?.first(),
                256 => d.width = r.values(at)?.first(),
                257 => d.height = r.values(at)?.first(),
                258 => d.bits = r.values(at)?.first(),
                259 => d.compression = r.values(at)?.first(),
                273 | 324 => d.offsets = r.values(at)?.ints(),
                277 => d.samples = r.values(at)?.first(),
                278 => d.rows_per_strip = r.values(at)?.first(),
                279 | 325 => d.counts = r.values(at)?.ints(),
                317 => d.predictor = r.values(at)?.first(),
                322 => d.tile_width = r.values(at)?.first(),
                323 => d.tile_height = r.values(at)?.first(),
                339 => d.format = r.values(at)?.first(),
                33550 => d.pixel_scale = r.values(at)?.floats(),
                33922 => d.tie_point = r.values(at)?.floats(),
                _ => {}
            }
        }
        next = r.u32(next + 2 + entries * 12)? as usize;
        directories.push(d);
    }
    Ok(Tiff {
        little_endian: little,
        directories,
    })
}

/// Inflates a zlib (TIFF deflate) piece into exactly `size` bytes.
pub fn inflate(raw: &[u8], size: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut out = Vec::with_capacity(size);
    flate2::read::ZlibDecoder::new(raw)
        .take(size as u64)
        .read_to_end(&mut out)
        .map_err(|e| e.to_string())?;
    if out.len() != size {
        return Err(format!("inflated {} of {size} bytes", out.len()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_headers_ask_for_more() {
        // "II", 42, first directory at 8 with 1 entry, cut off.
        let bytes = [b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0];
        assert!(matches!(parse(&bytes, 4), Err(TiffError::TooShort(_))));
        let mut full = bytes.to_vec();
        // Entry: width (256), SHORT, 1, 1024; then no next directory.
        full.extend([0, 1, 3, 0, 1, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0]);
        let tiff = parse(&full, 4).unwrap();
        assert_eq!(tiff.directories[0].width, 1024);
    }
}
