//! Horizon's world generation behind a C ABI, for the `horizon-accel`
//! Fabric mod (`docs/research/java-bridge.md`). Java calls these functions
//! through the FFM API and swaps vanilla's terrain generation for them.
//!
//! One crossing per unit of work: Java opens a generator once per
//! dimension, maps Horizon's block states and biomes to its own registry ids
//! by name, then asks for whole chunks. A chunk comes back as a handle whose
//! blocks, biomes and post-processing marks Java copies out in bulk.
//!
//! No panic crosses the boundary: each entry point catches it and reports
//! it like any other error, through [`horizon_accel_error`].
mod decorate;

pub use decorate::{Decoration, RECORD};
use horizon_core::position::ChunkPos;
use horizon_world::{
    registry::{BlockRegistry, BlockStateId},
    worldgen::{
        biome::BiomeId,
        generator::{GeneratorData, GeneratorWorker, OverworldGenerator, ProtoChunk, StageTimings},
    },
};
use std::{
    cell::RefCell,
    fs,
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    sync::{Arc, Mutex, OnceLock},
};

/// Bumped whenever a signature or a layout below changes.
pub const ABI: u32 = 4;

/// One dimension's generator, shared by every Java worldgen thread.
pub struct Accel {
    generator: OverworldGenerator,
    /// Idle workers; a thread takes one per chunk and puts it back.
    workers: Mutex<Vec<GeneratorWorker>>,
    states: Vec<String>,
    /// Horizon state id to the caller's id, once mapped.
    state_map: OnceLock<Vec<i32>>,
    biome_map: OnceLock<Vec<i32>>,
    /// The caller's state id to Horizon's, built on first decoration.
    inverse_map: OnceLock<Vec<i32>>,
    cache: Mutex<decorate::ChunkCache>,
}

pub struct Chunk {
    proto: ProtoChunk,
}

thread_local! {
    static ERROR: RefCell<String> = const { RefCell::new(String::new()) };
}

fn fail(message: String) {
    ERROR.with(|e| *e.borrow_mut() = message);
}

/// Runs `f`, turning an error or a panic into `on_error` and the thread's
/// last error.
fn guard<T>(on_error: T, f: impl FnOnce() -> Result<T, String>) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            fail(e);
            on_error
        }
        Err(panic) => {
            let text = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            fail(format!("panic: {text}"));
            on_error
        }
    }
}

/// # Safety
/// `ptr` must point to `len` readable bytes, or be null with `len` 0.
unsafe fn text<'a>(ptr: *const u8, len: usize) -> Result<&'a str, String> {
    if ptr.is_null() {
        return if len == 0 {
            Ok("")
        } else {
            Err("null text".into())
        };
    }
    // SAFETY: the caller passes a valid buffer.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    std::str::from_utf8(bytes).map_err(|e| e.to_string())
}

/// Copies `s` into `buf` (up to `cap` bytes) and returns its full length, so
/// a caller with too small a buffer can retry with the right size.
///
/// # Safety
/// `buf` must point to `cap` writable bytes, or be null with `cap` 0.
unsafe fn copy_out(s: &str, buf: *mut u8, cap: usize) -> usize {
    let n = s.len().min(cap);
    if n > 0 {
        // SAFETY: the caller passes a valid buffer of `cap` bytes.
        unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), buf, n) };
    }
    s.len()
}

/// `minecraft:oak_log[axis=y]`, as vanilla's `BlockStateParser` reads it.
fn state_string(registry: &BlockRegistry, id: u32) -> String {
    let state = registry.state(BlockStateId(id)).expect("registered state");
    if state.properties.is_empty() {
        return state.name.clone();
    }
    let props: Vec<String> = state
        .properties
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    format!("{}[{}]", state.name, props.join(","))
}

impl Accel {
    pub fn open(
        resources: &Path,
        seed: i64,
        dimension: &str,
        structures: bool,
    ) -> Result<Self, String> {
        let facts = resources.join("facts.json");
        let file = fs::File::open(&facts).map_err(|e| format!("{}: {e}", facts.display()))?;
        let registry = BlockRegistry::from_facts_json(file)
            .map_err(|e| format!("{}: {e}", facts.display()))?;
        let registry: &'static BlockRegistry = Box::leak(Box::new(registry));
        let data = GeneratorData::load(&resources.join("worldgen"), registry)
            .map_err(|e| e.to_string())?;
        Self::with_data(data, seed, dimension, structures)
    }

    pub fn with_data(
        data: Arc<GeneratorData>,
        seed: i64,
        dimension: &str,
        structures: bool,
    ) -> Result<Self, String> {
        let registry = data.registry();
        let states = (0..registry.len() as u32)
            .map(|i| state_string(registry, i))
            .collect();
        let generator =
            OverworldGenerator::for_world(data, seed, "minecraft:normal", dimension, structures)
                .map_err(|e| e.to_string())?;
        Ok(Self {
            generator,
            workers: Mutex::new(Vec::new()),
            states,
            state_map: OnceLock::new(),
            biome_map: OnceLock::new(),
            inverse_map: OnceLock::new(),
            cache: Mutex::default(),
        })
    }

    pub fn generate(&self, x: i32, z: i32) -> Result<Chunk, String> {
        let worker = self.workers.lock().unwrap().pop();
        let mut worker = worker.unwrap_or_else(|| self.generator.worker());
        let result = worker.generate_proto(ChunkPos::new(x, z));
        self.workers.lock().unwrap().push(worker);
        Ok(Chunk {
            proto: result.map_err(|e| e.to_string())?,
        })
    }

    /// Wall time spent in each stage so far, summed over the idle workers
    /// (all of them, between calls to [`Accel::generate`]).
    pub fn stage_timings(&self) -> StageTimings {
        let mut sum = StageTimings::default();
        for t in self
            .workers
            .lock()
            .unwrap()
            .iter()
            .map(|w| w.stage_timings())
        {
            sum.chunks += t.chunks;
            sum.biomes += t.biomes;
            sum.noise += t.noise;
            sum.surface += t.surface;
            sum.structures += t.structures;
            sum.carvers += t.carvers;
        }
        sum
    }

    /// Writes every block, section by section from the bottom, each section
    /// in vanilla's order (`y << 8 | z << 4 | x`), as the caller's ids.
    pub fn blocks(&self, chunk: &Chunk, out: &mut [i32]) -> Result<(), String> {
        let terrain = &chunk.proto.terrain;
        let sections = terrain.height() as usize / 16;
        if out.len() != sections * 4096 {
            return Err(format!("{} blocks, not {}", sections * 4096, out.len()));
        }
        let map = self.state_map.get();
        for (s, out) in out.chunks_exact_mut(4096).enumerate() {
            let section = terrain.section(s).ok_or("missing section")?;
            for (i, slot) in out.iter_mut().enumerate() {
                let id = section.get(i).map_err(|e| e.to_string())?.0;
                *slot = map.map_or(id as i32, |m| m[id as usize]);
            }
        }
        Ok(())
    }

    /// A section palette as the caller's ids.
    fn map_palette(&self, states: &[BlockStateId], out: &mut [i32]) {
        let map = self.state_map.get();
        for (out, &BlockStateId(id)) in out.iter_mut().zip(states) {
            *out = map.map_or(id as i32, |m| m[id as usize]);
        }
    }

    /// Writes every quart biome, 64 per section from the bottom, in vanilla's
    /// order (`y << 4 | z << 2 | x`), as the caller's ids.
    pub fn biomes(&self, chunk: &Chunk, out: &mut [i32]) -> Result<(), String> {
        let terrain = &chunk.proto.terrain;
        let sections = terrain.height() / 16;
        if out.len() != sections as usize * 64 {
            return Err(format!("{} biomes, not {}", sections * 64, out.len()));
        }
        let map = self.biome_map.get();
        let min_y = terrain.min_y();
        for (i, slot) in out.iter_mut().enumerate() {
            let i = i as i32;
            let (x, z, y) = (i & 3, (i >> 2) & 3, i >> 4);
            let BiomeId(id) = chunk.proto.biome_id_at(x * 4, min_y + y * 4, z * 4);
            *slot = map.map_or(id as i32, |m| m[id as usize]);
        }
        Ok(())
    }

    /// The chunk's biomes alone (vanilla's BIOMES status), laid out as in
    /// [`Accel::biomes`]. Horizon keeps them for generating the chunk later.
    pub fn biomes_only(&self, x: i32, z: i32, out: &mut [i32]) -> Result<(), String> {
        let (_, height) = self.generator.level_range();
        if out.len() != (height / 16) as usize * 64 {
            return Err(format!("{} biomes, not {}", height / 16 * 64, out.len()));
        }
        let worker = self.workers.lock().unwrap().pop();
        let mut worker = worker.unwrap_or_else(|| self.generator.worker());
        let result = worker.biomes(ChunkPos::new(x, z));
        self.workers.lock().unwrap().push(worker);
        let chunk = result.map_err(|e| e.to_string())?;
        let map = self.biome_map.get();
        let mut section = 0;
        let mut slots = out.chunks_exact_mut(64);
        while let (Some(biomes), Some(slots)) = (chunk.section(section), slots.next()) {
            for (slot, &BiomeId(id)) in slots.iter_mut().zip(biomes) {
                *slot = map.map_or(id as i32, |m| m[id as usize]);
            }
            section += 1;
        }
        Ok(())
    }

    pub fn biome_names(&self) -> &[Arc<str>] {
        self.generator.data().biome_names()
    }
}

/// The ABI version this library speaks.
#[unsafe(no_mangle)]
pub extern "C" fn horizon_accel_abi() -> u32 {
    ABI
}

/// Does nothing: for measuring the cost of one crossing.
#[unsafe(no_mangle)]
pub extern "C" fn horizon_accel_noop(x: i64) -> i64 {
    x
}

/// Sums `len` ints: for measuring reads of memory Java owns.
///
/// # Safety
/// `data` must point to `len` readable ints.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn horizon_accel_sum(data: *const i32, len: usize) -> i64 {
    if data.is_null() {
        return 0;
    }
    // SAFETY: the caller passes a valid buffer.
    let data = unsafe { std::slice::from_raw_parts(data, len) };
    data.iter().map(|&v| i64::from(v)).sum()
}

/// This thread's last error, copied as in [`copy_out`]; 0 if none.
///
/// # Safety
/// `buf` must point to `cap` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn horizon_accel_error(buf: *mut u8, cap: usize) -> usize {
    // SAFETY: forwarded from the caller.
    ERROR.with(|e| unsafe { copy_out(&e.borrow(), buf, cap) })
}

/// Opens a dimension's generator over a prepared resources folder (its
/// `facts.json` and `worldgen/`). Null on error.
///
/// # Safety
/// Both texts must be valid buffers of their lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn horizon_accel_open(
    resources: *const u8,
    resources_len: usize,
    seed: i64,
    dimension: *const u8,
    dimension_len: usize,
    structures: i32,
) -> *mut Accel {
    guard(std::ptr::null_mut(), || {
        // SAFETY: forwarded from the caller.
        let (resources, dimension) = unsafe {
            (
                text(resources, resources_len)?,
                text(dimension, dimension_len)?,
            )
        };
        let accel = Accel::open(Path::new(resources), seed, dimension, structures != 0)?;
        Ok(Box::into_raw(Box::new(accel)))
    })
}

/// # Safety
/// `accel` must come from [`horizon_accel_open`] and not be used after.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn horizon_accel_close(accel: *mut Accel) {
    if !accel.is_null() {
        // SAFETY: the caller hands back ownership.
        drop(unsafe { Box::from_raw(accel) });
    }
}

/// A chunk's lowest block Y and height in blocks, which size the buffers
/// below. Java checks them against its own chunk before copying.
///
/// # Safety
/// `chunk` must be live, `min_y` and `height` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn horizon_accel_chunk_bounds(
    chunk: *const Chunk,
    min_y: *mut i32,
    height: *mut i32,
) {
    // SAFETY: the caller passes a live chunk and two writable ints.
    let (chunk, min_y, height) = unsafe { (&*chunk, &mut *min_y, &mut *height) };
    (*min_y, *height) = (chunk.proto.terrain.min_y(), chunk.proto.terrain.height());
}

/// The number of Horizon block states.
///
/// # Safety
/// `accel` must be open.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn horizon_accel_state_count(accel: *const Accel) -> u32 {
    // SAFETY: the caller passes an open generator.
    unsafe { &*accel }.states.len() as u32
}

/// Block state `id` as vanilla's parser reads it, copied as in
/// [`copy_out`].
///
/// # Safety
/// `accel` must be open and `buf` hold `cap` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn horizon_accel_state_name(
    accel: *const Accel,
    id: u32,
    buf: *mut u8,
    cap: usize,
) -> usize {
    // SAFETY: the caller passes an open generator and a valid buffer.
    let accel = unsafe { &*accel };
    accel
        .states
        .get(id as usize)
        .map_or(0, |s| unsafe { copy_out(s, buf, cap) })
}

/// The number of biomes the generator knows.
///
/// # Safety
/// `accel` must be open.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn horizon_accel_biome_count(accel: *const Accel) -> u32 {
    // SAFETY: the caller passes an open generator.
    unsafe { &*accel }.biome_names().len() as u32
}

/// Biome `id`'s name (`minecraft:plains`), copied as in [`copy_out`].
///
/// # Safety
/// `accel` must be open and `buf` hold `cap` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn horizon_accel_biome_name(
    accel: *const Accel,
    id: u32,
    buf: *mut u8,
    cap: usize,
) -> usize {
    // SAFETY: the caller passes an open generator and a valid buffer.
    let accel = unsafe { &*accel };
    accel
        .biome_names()
        .get(id as usize)
        .map_or(0, |s| unsafe { copy_out(s, buf, cap) })
}

fn set_map(
    map: &OnceLock<Vec<i32>>,
    ids: &[i32],
    expected: usize,
    what: &str,
) -> Result<i32, String> {
    if ids.len() != expected {
        return Err(format!("{} {what} ids for {expected} {what}s", ids.len()));
    }
    map.set(ids.to_vec())
        .map_err(|_| format!("{what} ids are already mapped"))?;
    Ok(0)
}

/// Gives the caller's id for every Horizon block state, once, before
/// generating. 0 on success, -1 on error.
///
/// # Safety
/// `accel` must be open and `ids` hold `len` ints.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn horizon_accel_map_states(
    accel: *const Accel,
    ids: *const i32,
    len: usize,
) -> i32 {
    guard(-1, || {
        // SAFETY: the caller passes an open generator and a valid buffer.
        let (accel, ids) = unsafe { (&*accel, std::slice::from_raw_parts(ids, len)) };
        set_map(&accel.state_map, ids, accel.states.len(), "state")
    })
}

/// As [`horizon_accel_map_states`], for biomes.
///
/// # Safety
/// `accel` must be open and `ids` hold `len` ints.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn horizon_accel_map_biomes(
    accel: *const Accel,
    ids: *const i32,
    len: usize,
) -> i32 {
    guard(-1, || {
        // SAFETY: the caller passes an open generator and a valid buffer.
        let (accel, ids) = unsafe { (&*accel, std::slice::from_raw_parts(ids, len)) };
        set_map(&accel.biome_map, ids, accel.biome_names().len(), "biome")
    })
}

/// Generates chunk (`x`, `z`) up to vanilla's carvers status. Null on error.
/// Thread-safe: call it from as many worldgen threads as Java has.
///
/// # Safety
/// `accel` must be open.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn horizon_accel_generate(accel: *const Accel, x: i32, z: i32) -> *mut Chunk {
    guard(std::ptr::null_mut(), || {
        // SAFETY: the caller passes an open generator.
        let chunk = unsafe { &*accel }.generate(x, z)?;
        Ok(Box::into_raw(Box::new(chunk)))
    })
}

/// Copies a chunk's blocks into `out` (`height * 256` ints, layout in
/// [`Accel::blocks`]). 0 on success, -1 on error.
///
/// # Safety
/// `accel` and `chunk` must be live and `out` hold `len` writable ints.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn horizon_accel_chunk_blocks(
    accel: *const Accel,
    chunk: *const Chunk,
    out: *mut i32,
    len: usize,
) -> i32 {
    guard(-1, || {
        // SAFETY: the caller passes live handles and a valid buffer.
        let (accel, chunk, out) =
            unsafe { (&*accel, &*chunk, std::slice::from_raw_parts_mut(out, len)) };
        accel.blocks(chunk, out).map(|()| 0)
    })
}

/// Copies section `section` in its packed form, which the Java side hands to
/// vanilla's `PalettedContainer` as it is instead of setting 4096 blocks:
/// the palette (as the caller's ids) into `palette`, the packed indices into
/// `words` (vanilla's `SimpleBitStorage` layout, none for a section of one
/// state). `sizes` receives both lengths. Returns 0 when both fit and were
/// copied, 1 when a buffer is too small (nothing copied: retry with
/// `sizes`), -1 on error. `sizes` gets the palette's length, the words'
/// length and the bits per index (0 for a section of one state).
///
/// # Safety
/// `accel` and `chunk` must be live, `palette` and `words` hold their caps,
/// and `sizes` three writable `usize`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn horizon_accel_chunk_section(
    accel: *const Accel,
    chunk: *const Chunk,
    section: u32,
    palette: *mut i32,
    palette_cap: usize,
    words: *mut i64,
    words_cap: usize,
    sizes: *mut usize,
) -> i32 {
    guard(-1, || {
        // SAFETY: the caller passes live handles and valid buffers.
        let (accel, chunk, sizes) =
            unsafe { (&*accel, &*chunk, std::slice::from_raw_parts_mut(sizes, 3)) };
        let data = chunk
            .proto
            .terrain
            .section(section as usize)
            .ok_or_else(|| format!("no section {section}"))?;
        let (states, packed) = data.packed();
        (sizes[0], sizes[1], sizes[2]) = (states.len(), packed.len(), data.bits() as usize);
        if states.len() > palette_cap || packed.len() > words_cap {
            return Ok(1);
        }
        // SAFETY: both fit their buffers, checked above.
        let (palette, words) = unsafe {
            (
                std::slice::from_raw_parts_mut(palette, states.len()),
                std::slice::from_raw_parts_mut(words, packed.len()),
            )
        };
        accel.map_palette(states, palette);
        for (out, &word) in words.iter_mut().zip(packed) {
            *out = word as i64;
        }
        Ok(0)
    })
}

/// Writes chunk (`x`, `z`)'s biomes alone into `out` (`height * 4` ints,
/// layout in [`Accel::biomes`]), keeping them for its generation. 0 on
/// success, -1 on error. Thread-safe.
///
/// # Safety
/// `accel` must be open and `out` hold `len` writable ints.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn horizon_accel_biomes(
    accel: *const Accel,
    x: i32,
    z: i32,
    out: *mut i32,
    len: usize,
) -> i32 {
    guard(-1, || {
        // SAFETY: the caller passes an open generator and a valid buffer.
        let (accel, out) = unsafe { (&*accel, std::slice::from_raw_parts_mut(out, len)) };
        accel.biomes_only(x, z, out).map(|()| 0)
    })
}

/// Copies a chunk's quart biomes into `out` (`height * 4` ints, layout in
/// [`Accel::biomes`]). 0 on success, -1 on error.
///
/// # Safety
/// `accel` and `chunk` must be live and `out` hold `len` writable ints.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn horizon_accel_chunk_biomes(
    accel: *const Accel,
    chunk: *const Chunk,
    out: *mut i32,
    len: usize,
) -> i32 {
    guard(-1, || {
        // SAFETY: the caller passes live handles and a valid buffer.
        let (accel, chunk, out) =
            unsafe { (&*accel, &*chunk, std::slice::from_raw_parts_mut(out, len)) };
        accel.biomes(chunk, out).map(|()| 0)
    })
}

/// Section `section`'s positions marked for post-processing (vanilla's
/// `ProtoChunk.markPosForPostprocessing`, packed as in its saves), copied
/// into `out` up to `cap`. Returns how many there are.
///
/// # Safety
/// `chunk` must be live and `out` hold `cap` writable shorts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn horizon_accel_chunk_postprocessing(
    chunk: *const Chunk,
    section: u32,
    out: *mut u16,
    cap: usize,
) -> usize {
    // SAFETY: the caller passes a live chunk.
    let chunk = unsafe { &*chunk };
    let Some(marks) = chunk.proto.terrain.postprocessing().get(section as usize) else {
        return 0;
    };
    let n = marks.len().min(cap);
    if n > 0 {
        // SAFETY: the caller passes a buffer of `cap` shorts.
        unsafe { std::ptr::copy_nonoverlapping(marks.as_ptr(), out, n) };
    }
    marks.len()
}

/// Writes `[chunks, biomes, noise, surface, structures, carvers]` so far, the
/// times in nanoseconds summed over threads ([`Accel::stage_timings`]), into
/// `out`. Returns 0, or -1 when `len` is under 6.
///
/// # Safety
/// `accel` must be live, `out` must point to `len` writable `i64`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn horizon_accel_stage_timings(
    accel: *const Accel,
    out: *mut i64,
    len: usize,
) -> i32 {
    guard(-1, || {
        if len < 6 {
            return Err(format!("{len} slots, not 6"));
        }
        // SAFETY: the caller passes a live generator and a buffer of `len`.
        let (accel, out) = unsafe { (&*accel, std::slice::from_raw_parts_mut(out, len)) };
        let t = accel.stage_timings();
        let ns = |d: std::time::Duration| d.as_nanos() as i64;
        out[..6].copy_from_slice(&[
            t.chunks as i64,
            ns(t.biomes),
            ns(t.noise),
            ns(t.surface),
            ns(t.structures),
            ns(t.carvers),
        ]);
        Ok(0)
    })
}

/// # Safety
/// `chunk` must come from [`horizon_accel_generate`] and not be used after.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn horizon_accel_chunk_free(chunk: *mut Chunk) {
    if !chunk.is_null() {
        // SAFETY: the caller hands back ownership.
        drop(unsafe { Box::from_raw(chunk) });
    }
}

/// Decorates chunk (`cx`, `cz`) (vanilla's FEATURES status) on a copy of its
/// 3×3 region that Java passes in (layout in [`Decoration`]'s module), and
/// returns the writes for Java to repeat. Null on error.
///
/// # Safety
/// `accel` must be open, `ints` and `longs` hold their lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn horizon_accel_decorate(
    accel: *const Accel,
    cx: i32,
    cz: i32,
    ints: *const i32,
    ints_len: usize,
    longs: *const i64,
    longs_len: usize,
) -> *mut Decoration {
    guard(std::ptr::null_mut(), || {
        // SAFETY: the caller passes an open generator and valid buffers.
        let (accel, ints, longs) = unsafe {
            (
                &*accel,
                std::slice::from_raw_parts(ints, ints_len),
                std::slice::from_raw_parts(longs, longs_len),
            )
        };
        Ok(Box::into_raw(Box::new(
            accel.decorate(cx, cz, ints, longs)?,
        )))
    })
}

/// Writes `[op ints, string bytes, unsupported, copy ns, decorate ns,
/// missing]` for a decoration into `out` (6 slots).
///
/// # Safety
/// `decoration` must be live and `out` hold 6 writable `i64`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn horizon_accel_decoration_sizes(
    decoration: *const Decoration,
    out: *mut i64,
) {
    // SAFETY: the caller passes a live decoration and a buffer of 6.
    let (d, out) = unsafe { (&*decoration, std::slice::from_raw_parts_mut(out, 6)) };
    out.copy_from_slice(&[
        d.ops.len() as i64,
        d.strings.len() as i64,
        d.unsupported as i64,
        d.copy_nanos as i64,
        d.decorate_nanos as i64,
        d.missing as i64,
    ]);
}

/// Hands a generated chunk back to keep for decoration, under the caller's
/// `token`, with the heightmaps the caller's copy has (`mask` and `words` as
/// in [`horizon_accel_decorate`]'s input). Takes ownership of `chunk` either
/// way. 0 on success, -1 on error.
///
/// # Safety
/// `accel` must be open, `chunk` come from [`horizon_accel_generate`] and not
/// be used after, `words` hold `len` longs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn horizon_accel_chunk_keep(
    accel: *const Accel,
    chunk: *mut Chunk,
    token: u64,
    mask: i32,
    words: *const i64,
    len: usize,
) -> i32 {
    // SAFETY: the caller hands over ownership of a live chunk.
    let chunk = *unsafe { Box::from_raw(chunk) };
    guard(-1, || {
        // SAFETY: the caller passes an open generator and a valid buffer.
        let (accel, words) = unsafe { (&*accel, std::slice::from_raw_parts(words, len)) };
        accel.keep(chunk, token, mask, words).map(|()| 0)
    })
}

/// Forgets kept chunks (the caller decorated their region itself).
///
/// # Safety
/// `accel` must be open and `tokens` hold `len` tokens.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn horizon_accel_forget(accel: *const Accel, tokens: *const u64, len: usize) {
    // SAFETY: the caller passes an open generator and a valid buffer.
    let (accel, tokens) = unsafe { (&*accel, std::slice::from_raw_parts(tokens, len)) };
    let mut cache = accel.cache.lock().unwrap();
    tokens.iter().for_each(|&t| cache.forget(t));
}

/// Copies a decoration's records and strings, which must fit the buffers
/// ([`horizon_accel_decoration_sizes`]). 0 on success, -1 when they don't.
///
/// # Safety
/// `decoration` must be live, `ops` and `strings` hold their caps.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn horizon_accel_decoration_copy(
    decoration: *const Decoration,
    ops: *mut i32,
    ops_cap: usize,
    strings: *mut u8,
    strings_cap: usize,
) -> i32 {
    // SAFETY: the caller passes a live decoration.
    let d = unsafe { &*decoration };
    if d.ops.len() > ops_cap || d.strings.len() > strings_cap {
        return -1;
    }
    // SAFETY: both fit, checked above.
    unsafe {
        std::ptr::copy_nonoverlapping(d.ops.as_ptr(), ops, d.ops.len());
        std::ptr::copy_nonoverlapping(d.strings.as_ptr(), strings, d.strings.len());
    }
    0
}

/// # Safety
/// `decoration` must come from [`horizon_accel_decorate`] and not be used after.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn horizon_accel_decoration_free(decoration: *mut Decoration) {
    if !decoration.is_null() {
        // SAFETY: the caller hands back ownership.
        drop(unsafe { Box::from_raw(decoration) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_errors_without_unwinding() {
        let path = "/nonexistent/resources";
        let dim = "minecraft:overworld";
        // SAFETY: valid buffers.
        let accel =
            unsafe { horizon_accel_open(path.as_ptr(), path.len(), 1, dim.as_ptr(), dim.len(), 1) };
        assert!(accel.is_null());
        let mut buf = [0u8; 256];
        // SAFETY: a valid buffer.
        let n = unsafe { horizon_accel_error(buf.as_mut_ptr(), buf.len()) };
        let message = std::str::from_utf8(&buf[..n.min(buf.len())]).unwrap();
        assert!(message.contains("facts.json"), "{message}");
        assert_eq!(guard(7, || -> Result<i32, String> { panic!("boom") }), 7);
        let n = unsafe { horizon_accel_error(buf.as_mut_ptr(), buf.len()) };
        assert_eq!(&buf[..n], b"panic: boom");
    }

    #[test]
    fn copies_out_with_the_full_length() {
        let mut buf = [0u8; 4];
        // SAFETY: a valid buffer.
        assert_eq!(
            unsafe { copy_out("minecraft:stone", buf.as_mut_ptr(), 4) },
            15
        );
        assert_eq!(&buf, b"mine");
        assert_eq!(unsafe { copy_out("x", std::ptr::null_mut(), 0) }, 1);
        let data = [1, 2, 3, -4];
        assert_eq!(unsafe { horizon_accel_sum(data.as_ptr(), 4) }, 2);
        assert_eq!(horizon_accel_noop(9), 9);
    }

    /// Needs prepared resources: `HORIZON_RESOURCES=<client resource dir>`.
    #[test]
    #[ignore]
    fn generates_a_chunk() {
        let resources = std::env::var("HORIZON_RESOURCES").expect("HORIZON_RESOURCES");
        let accel = Accel::open(Path::new(&resources), 12345, "minecraft:overworld", true).unwrap();
        let chunk = accel.generate(0, 0).unwrap();
        let height = chunk.proto.terrain.height() as usize;
        let mut blocks = vec![0; height * 256];
        accel.blocks(&chunk, &mut blocks).unwrap();
        let bedrock = accel
            .states
            .iter()
            .position(|s| s == "minecraft:bedrock")
            .unwrap() as i32;
        assert!(blocks[..256].contains(&bedrock));
        let mut biomes = vec![0; height * 4];
        accel.biomes(&chunk, &mut biomes).unwrap();
        assert!(
            biomes
                .iter()
                .all(|&b| (b as usize) < accel.biome_names().len())
        );
    }
}
