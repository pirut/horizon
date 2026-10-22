//! zlib streams for chunk saves, chunk loads and network packets, compressed
//! and decompressed by zlib-rs instead of the JDK's zlib.
//!
//! Vanilla compresses region chunks and packets at `Deflater`'s default level
//! (6) and reads them back with `Inflater`. These do the same work in one
//! crossing per chunk or packet, into buffers Java owns. The decompressed
//! bytes are always vanilla's; the compressed bytes are a valid zlib stream of
//! the same data at the same level, though not zlib's exact bytes, which no
//! reader depends on (every zlib reader decodes either).
use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status};
use std::cell::RefCell;

/// `Deflater.DEFAULT_COMPRESSION`, which is zlib's level 6.
const LEVEL: u32 = 6;

thread_local! {
    // A compressor's state is ~270 KB: one per thread, reset between uses.
    static COMPRESS: RefCell<Compress> = RefCell::new(Compress::new(Compression::new(LEVEL), true));
    static DECOMPRESS: RefCell<Decompress> = RefCell::new(Decompress::new(true));
}

/// The output size [`deflate`] always fits in: zlib's `compressBound`.
pub fn bound(len: usize) -> usize {
    len + (len >> 12) + (len >> 14) + (len >> 25) + 13
}

/// Compresses `input` into `output` as one zlib stream; the bytes written, or
/// `None` when `output` is too small (never at [`bound`]).
pub fn deflate(input: &[u8], output: &mut [u8]) -> Option<usize> {
    COMPRESS.with(|c| {
        let mut c = c.borrow_mut();
        c.reset();
        match c.compress(input, output, FlushCompress::Finish) {
            Ok(Status::StreamEnd) => Some(c.total_out() as usize),
            _ => None,
        }
    })
}

/// How [`inflate`] ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Inflated {
    /// The whole stream, this many bytes.
    Done(usize),
    /// `output` filled before the stream ended.
    Full,
    /// Not a valid zlib stream, or it ended early: Java leaves these to the
    /// JDK, so their errors stay vanilla's.
    Bad,
}

/// Decompresses the zlib stream at the start of `input` (bytes after its end
/// are ignored, as `InflaterInputStream` ignores them) into `output`.
pub fn inflate(input: &[u8], output: &mut [u8]) -> Inflated {
    DECOMPRESS.with(|d| {
        let mut d = d.borrow_mut();
        d.reset(true);
        match d.decompress(input, output, FlushDecompress::Finish) {
            Ok(Status::StreamEnd) => Inflated::Done(d.total_out() as usize),
            Ok(_) if d.total_out() as usize == output.len() => Inflated::Full,
            _ => Inflated::Bad,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn sample() -> Vec<u8> {
        // Chunk-like: long runs with some variety.
        (0..100_000u32)
            .map(|i| {
                if i % 977 < 600 {
                    0
                } else {
                    (i * 31 % 251) as u8
                }
            })
            .collect()
    }

    #[test]
    fn round_trips_through_any_zlib_reader() {
        for data in [Vec::new(), b"x".to_vec(), sample()] {
            let mut out = vec![0; bound(data.len())];
            let n = deflate(&data, &mut out).unwrap();
            let mut back = Vec::new();
            flate2::read::ZlibDecoder::new(&out[..n])
                .read_to_end(&mut back)
                .unwrap();
            assert_eq!(back, data);
            let mut again = vec![0; data.len()];
            assert_eq!(inflate(&out[..n], &mut again), Inflated::Done(data.len()));
            assert_eq!(again, data);
        }
    }

    #[test]
    fn reads_other_encoders_streams_and_reports_what_it_cannot() {
        let data = sample();
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(&data).unwrap();
        let mut stream = encoder.finish().unwrap();
        let mut out = vec![0; data.len()];
        assert_eq!(inflate(&stream, &mut out), Inflated::Done(data.len()));
        assert_eq!(out, data);
        // Trailing bytes after the stream are ignored.
        stream.extend_from_slice(b"trailing");
        assert_eq!(inflate(&stream, &mut out), Inflated::Done(data.len()));
        let mut small = vec![0; 1000];
        assert_eq!(inflate(&stream, &mut small), Inflated::Full);
        assert_eq!(small, data[..1000]);
        assert_eq!(
            inflate(&stream[..stream.len() / 2], &mut out),
            Inflated::Bad
        );
        assert_eq!(inflate(b"not zlib at all", &mut out), Inflated::Bad);
        assert_eq!(deflate(&data, &mut [0; 10]), None);
    }
}
