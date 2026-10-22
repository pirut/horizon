//! The FEATURES status for a region whose blocks live in Java: Java copies
//! the 3×3 region in, Horizon decorates its copy and hands back every write
//! it made, in order, as the Java calls that make them. Java repeats those
//! calls on its own chunks, so block entities, ticks, heightmaps and
//! post-processing come out of vanilla's own code.
//!
//! Horizon keeps the chunks it generated or decorated ([`ChunkCache`]), under
//! a token Java holds for each chunk object, so a region usually needs no
//! copying: Java names the chunks Horizon still has as it last left them.
//!
//! Input, read in order. `ints`: per chunk, whether it is cached (0 or 1)
//! and its token (high and low int); for a chunk not cached, then
//! `decorated` (0 or 1) and a mask of the heightmaps that follow (bit `i` for
//! `HeightmapType::ALL[i]`), then per section its palette length, bits per
//! index and word count, then the palette as the caller's state ids.
//! `longs`: per chunk not cached, the present heightmaps' raw words (in mask
//! order), then each section's packed words.
//!
//! Output: records of [`RECORD`] ints, `[kind, x, y, z, a, b, c]`:
//! 1 setBlock(state a, flags b); 3 section setBlockState(state a);
//! 4 block tick(block's default state a, delay b); 5 fluid tick(fluid a as
//! `FluidKind::ALL` index, delay b); 6 markPosForPostprocessing;
//! 7 spawner entity (string a); 8 loot table (string a, seed b << 32 | c);
//! 9 the heightmap `HeightmapType::ALL[a]` of column (x, z) read, priming it;
//! 10 block entity data loaded (binary NBT at a, b bytes long); 11 entity added
//! from binary NBT (at a, b bytes long; `horizon:finalize` set when vanilla
//! calls `finalizeSpawn`).
//! Strings are joined with NUL bytes, NBT stored whole. A write Java can't
//! repeat yet (raw chunk writes) or a feature Horizon could not
//! run makes the result unsupported: Java then decorates with vanilla.
use crate::Accel;
use std::collections::HashMap;
use horizon_core::position::ChunkPos;
use horizon_world::{
    chunk::BlockSection,
    fluid::FluidKind,
    heightmap::HeightmapType,
    nbt::{self, Compound, Tag},
    registry::BlockStateId,
    worldgen::generator::{DecorationOp, ProtoChunk},
};

pub const RECORD: usize = 7;

#[derive(Default)]
pub struct Decoration {
    pub ops: Vec<i32>,
    pub strings: Vec<u8>,
    /// Writes or features Java would have to do itself; nonzero means
    /// decorate with vanilla instead.
    pub unsupported: u32,
    /// A chunk named as cached that Horizon no longer has: nothing was done,
    /// and Java sends the region again in full.
    pub missing: u32,
    /// Horizon's own time on the region, in nanoseconds: copying it in, decorating.
    pub copy_nanos: u64,
    pub decorate_nanos: u64,
}

struct Reader<'a, T> {
    data: &'a [T],
    at: usize,
}
impl<'a, T: Copy> Reader<'a, T> {
    fn take(&mut self, n: usize) -> Result<&'a [T], String> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.data.len()).ok_or("region input too short")?;
        let slice = &self.data[self.at..end];
        self.at = end;
        Ok(slice)
    }
    fn one(&mut self) -> Result<T, String> {
        Ok(self.take(1)?[0])
    }
}

/// Chunks Horizon keeps between calls, by Java's token for each chunk.
/// Each chunk serves up to nine regions (its own and its neighbours') and is
/// dropped after the ninth, or when it is the least recently used of too many.
#[derive(Default)]
pub struct ChunkCache {
    chunks: HashMap<u64, Cached>,
    clock: u64,
}

struct Cached {
    proto: ProtoChunk,
    uses: u8,
    stamp: u64,
}

/// Generation keeps chunks ahead of decoration; past this many, the oldest go.
const CACHE_CHUNKS: usize = 2048;

impl ChunkCache {
    fn take(&mut self, token: u64, pos: ChunkPos) -> Option<(ProtoChunk, u8)> {
        let cached = self.chunks.remove(&token)?;
        (cached.proto.position() == pos).then_some((cached.proto, cached.uses))
    }

    fn put(&mut self, token: u64, proto: ProtoChunk, uses: u8) {
        if uses >= 9 {
            return;
        }
        self.clock += 1;
        self.chunks.insert(token, Cached { proto, uses, stamp: self.clock });
        if self.chunks.len() > CACHE_CHUNKS {
            let mut stamps: Vec<_> = self.chunks.values().map(|c| c.stamp).collect();
            stamps.sort_unstable();
            let cut = stamps[CACHE_CHUNKS / 8];
            self.chunks.retain(|_, c| c.stamp > cut);
        }
    }

    pub fn forget(&mut self, token: u64) {
        self.chunks.remove(&token);
    }
}

impl Accel {
    /// Keeps a chunk Java just filled from [`Accel::generate`], with the
    /// heightmaps Java's copy has (`mask` and `words` as in decoration input).
    pub fn keep(&self, chunk: crate::Chunk, token: u64, mask: i32, words: &[i64]) -> Result<(), String> {
        let mut proto = chunk.proto;
        proto.terrain.clear_heightmaps();
        let words_per_map = heightmap_words(self.generator.level_range().1);
        let mut words = Reader { data: words, at: 0 };
        for (bit, kind) in HeightmapType::ALL.into_iter().enumerate() {
            if mask & (1 << bit) != 0 {
                let raw: Vec<u64> = words.take(words_per_map)?.iter().map(|&w| w as u64).collect();
                proto.terrain.set_heightmap_raw(kind, &raw).map_err(|e| e.to_string())?;
            }
        }
        self.cache.lock().unwrap().put(token, proto, 0);
        Ok(())
    }

    /// Java's state id to Horizon's, built from the mapping Java gave.
    fn inverse_states(&self) -> Result<&[i32], String> {
        let map = self.state_map.get().ok_or("states are not mapped")?;
        Ok(self.inverse_map.get_or_init(|| {
            let len = map.iter().copied().max().unwrap_or(-1).max(-1) + 1;
            let mut inverse = vec![-1; len as usize];
            for (horizon, &java) in map.iter().enumerate() {
                if java >= 0 && inverse[java as usize] < 0 {
                    inverse[java as usize] = horizon as i32;
                }
            }
            inverse
        }))
    }

    pub fn decorate(&self, cx: i32, cz: i32, ints: &[i32], longs: &[i64]) -> Result<Decoration, String> {
        let inverse = self.inverse_states()?;
        let map = self.state_map.get().ok_or("states are not mapped")?;
        let registry = self.generator.data().registry();
        let worker = self.workers.lock().unwrap().pop();
        let mut worker = worker.unwrap_or_else(|| self.generator.worker());
        let mut tokens = [0u64; 9];
        let mut uses = [0u8; 9];
        let result = (|| {
            let clock = std::time::Instant::now();
            let (mut ints, mut longs) = (Reader { data: ints, at: 0 }, Reader { data: longs, at: 0 });
            let height = self.generator.level_range().1;
            let sections = (height / 16) as usize;
            let heightmap_words = heightmap_words(height);
            let mut region = Vec::with_capacity(9);
            for i in 0..9 {
                let pos = ChunkPos::new(cx + (i % 3) as i32 - 1, cz + (i / 3) as i32 - 1);
                let cached = ints.one()? != 0;
                let token = (ints.one()? as u32 as u64) << 32 | ints.one()? as u32 as u64;
                tokens[i] = token;
                if cached {
                    match self.cache.lock().unwrap().take(token, pos) {
                        Some((chunk, used)) => {
                            uses[i] = used;
                            region.push(chunk);
                            continue;
                        }
                        None => return Ok(None),
                    }
                }
                let decorated = ints.one()? != 0;
                let mask = ints.one()?;
                let mut heightmaps = Vec::new();
                for (bit, kind) in HeightmapType::ALL.into_iter().enumerate() {
                    if mask & (1 << bit) != 0 {
                        let words = longs.take(heightmap_words)?;
                        heightmaps.push((kind, words.iter().map(|&w| w as u64).collect::<Vec<_>>()));
                    }
                }
                let mut blocks = Vec::with_capacity(sections);
                let mut palette = Vec::new();
                for _ in 0..sections {
                    let (len, bits, count) = (ints.one()? as usize, ints.one()?, ints.one()? as usize);
                    palette.clear();
                    for &java in ints.take(len)? {
                        let horizon = inverse.get(java as usize).copied().unwrap_or(-1);
                        if horizon < 0 {
                            return Err(format!("state {java} has no Horizon state"));
                        }
                        palette.push(BlockStateId(horizon as u32));
                    }
                    let words: Vec<u64> = longs.take(count)?.iter().map(|&w| w as u64).collect();
                    blocks.push(
                        BlockSection::from_packed(&palette, bits as u8, &words, registry).map_err(|e| e.to_string())?,
                    );
                }
                let heightmaps: Vec<_> = heightmaps.iter().map(|(k, w)| (*k, w.as_slice())).collect();
                let mut chunk = worker.proto_from_sections(pos, blocks, &heightmaps, i == 4).map_err(|e| e.to_string())?;
                if decorated {
                    chunk.set_decorated();
                }
                region.push(chunk);
            }
            let mut region: [ProtoChunk; 9] = region.try_into().map_err(|_| "region")?;
            let copy_nanos = clock.elapsed().as_nanos() as u64;
            let clock = std::time::Instant::now();
            let (stats, log) = worker.decorate_logged(&mut region).map_err(|e| e.to_string())?;
            if stats.unsupported + stats.failed == 0 {
                let mut cache = self.cache.lock().unwrap();
                for (i, chunk) in region.into_iter().enumerate() {
                    cache.put(tokens[i], chunk, uses[i] + 1);
                }
            }
            let mut out = Decoration {
                unsupported: (stats.unsupported + stats.failed) as u32,
                copy_nanos,
                decorate_nanos: clock.elapsed().as_nanos() as u64,
                ..Decoration::default()
            };
            let binary = |out: &mut Decoration, nbt: Compound| -> Result<(i32, i32), String> {
                let bytes = nbt::write_network(&Tag::Compound(nbt), nbt::Limits::default()).map_err(|e| format!("{e:?}"))?;
                let start = out.strings.len() as i32;
                out.strings.extend_from_slice(&bytes);
                Ok((start, bytes.len() as i32))
            };
            let debug = std::env::var_os("HORIZON_ACCEL_DEBUG").is_some();
            if debug && out.unsupported > 0 {
                eprintln!("horizon-accel: decorate {cx} {cz}: {} features failed or unsupported", out.unsupported);
            }
            let java = |s: BlockStateId| map[s.0 as usize];
            let string = |out: &mut Decoration, s: &str| {
                let start = out.strings.len() as i32;
                out.strings.extend_from_slice(s.as_bytes());
                out.strings.push(0);
                start
            };
            for op in log {
                let record: [i32; RECORD] = match op {
                    DecorationOp::SetBlock([x, y, z], s, flags) => [1, x, y, z, java(s), flags, 0],
                    DecorationOp::SetSectionBlock([x, y, z], s) => [3, x, y, z, java(s), 0, 0],
                    DecorationOp::BlockTick([x, y, z], s, delay) => [4, x, y, z, java(s), delay, 0],
                    DecorationOp::FluidTick([x, y, z], fluid, delay) => {
                        let kind = FluidKind::ALL.iter().position(|f| *f == fluid).unwrap_or(0) as i32;
                        [5, x, y, z, kind, delay, 0]
                    }
                    DecorationOp::PostProcess([x, y, z]) => [6, x, y, z, 0, 0, 0],
                    DecorationOp::Spawner([x, y, z], entity) => {
                        let a = string(&mut out, &entity);
                        [7, x, y, z, a, 0, 0]
                    }
                    DecorationOp::LootTable([x, y, z], table, seed) => {
                        let a = string(&mut out, &table);
                        [8, x, y, z, a, (seed >> 32) as i32, seed as i32]
                    }
                    DecorationOp::PrimeHeightmap([x, z], kind) => {
                        let kind = HeightmapType::ALL.iter().position(|k| *k == kind).unwrap_or(0) as i32;
                        [9, x, 0, z, kind, 0, 0]
                    }
                    DecorationOp::BlockEntity([x, y, z], nbt) => {
                        let (a, b) = binary(&mut out, nbt)?;
                        [10, x, y, z, a, b, 0]
                    }
                    DecorationOp::Entity(nbt) => {
                        let (a, b) = binary(&mut out, nbt)?;
                        [11, 0, 0, 0, a, b, 0]
                    }
                    op @ DecorationOp::SetChunkBlock(..) => {
                        if debug {
                            eprintln!("horizon-accel: decorate {cx} {cz}: unsupported {op:?}");
                        }
                        out.unsupported += 1;
                        continue;
                    }
                };
                out.ops.extend_from_slice(&record);
            }
            Ok(Some(out))
        })();
        self.workers.lock().unwrap().push(worker);
        match result {
            Ok(Some(out)) if out.unsupported == 0 => Ok(out),
            // Java decorates this region itself: what Horizon kept of it is stale.
            other => {
                let mut cache = self.cache.lock().unwrap();
                tokens.iter().for_each(|&t| cache.forget(t));
                other.map(|out| out.unwrap_or(Decoration { missing: 1, ..Decoration::default() }))
            }
        }
    }
}

/// A heightmap's raw words: 256 heights in `0..=height`, `SimpleBitStorage` layout.
fn heightmap_words(height: i32) -> usize {
    let bits = (u32::BITS - (height as u32).leading_zeros()) as usize;
    256usize.div_ceil(64 / bits)
}
