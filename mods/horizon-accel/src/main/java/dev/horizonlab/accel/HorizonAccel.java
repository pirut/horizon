package dev.horizonlab.accel;

import java.lang.foreign.Arena;
import java.lang.foreign.FunctionDescriptor;
import java.lang.foreign.Linker;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.SymbolLookup;
import java.lang.foreign.ValueLayout;
import java.lang.invoke.MethodHandle;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;

/**
 * The native side of horizon-accel ({@code crates/horizon-accel}), through Java's FFM API.
 * It needs nothing from Minecraft: the mod's mixins call it with plain arrays.
 *
 * <p>Short calls that copy into Java arrays are linked as critical, so they take heap arrays with no copy
 * and no thread-state transition. Chunk generation takes milliseconds, so it is linked normally and the
 * garbage collector keeps running meanwhile.
 */
public final class HorizonAccel {
    public static final int ABI = 5;

    private static final ValueLayout.OfInt I32 = ValueLayout.JAVA_INT;
    private static final ValueLayout.OfLong I64 = ValueLayout.JAVA_LONG;
    private static final ValueLayout SIZE = ValueLayout.JAVA_LONG;
    private static final ValueLayout PTR = ValueLayout.ADDRESS;

    final MethodHandle abi, noop, noopCritical, sum, error, open, close, stateCount, stateName,
            biomeCount, biomeName, mapStates, mapBiomes, generate, chunkBounds, chunkBlocks, chunkSection, chunkBiomes,
            chunkPostprocessing, chunkFree, stageTimings, decorate, decorationSizes, decorationCopy, decorationFree, chunkKeep, forget, biomes,
            deflate, deflateBound, inflate;

    private HorizonAccel(SymbolLookup lookup) {
        abi = link(lookup, "horizon_accel_abi", FunctionDescriptor.of(I32), false);
        noop = link(lookup, "horizon_accel_noop", FunctionDescriptor.of(I64, I64), false);
        noopCritical = link(lookup, "horizon_accel_noop", FunctionDescriptor.of(I64, I64), true);
        sum = link(lookup, "horizon_accel_sum", FunctionDescriptor.of(I64, PTR, SIZE), true);
        error = link(lookup, "horizon_accel_error", FunctionDescriptor.of(SIZE, PTR, SIZE), true);
        open = link(lookup, "horizon_accel_open", FunctionDescriptor.of(PTR, PTR, SIZE, I64, PTR, SIZE, I32), false);
        close = link(lookup, "horizon_accel_close", FunctionDescriptor.ofVoid(PTR), false);
        stateCount = link(lookup, "horizon_accel_state_count", FunctionDescriptor.of(I32, PTR), true);
        stateName = link(lookup, "horizon_accel_state_name", FunctionDescriptor.of(SIZE, PTR, I32, PTR, SIZE), true);
        biomeCount = link(lookup, "horizon_accel_biome_count", FunctionDescriptor.of(I32, PTR), true);
        biomeName = link(lookup, "horizon_accel_biome_name", FunctionDescriptor.of(SIZE, PTR, I32, PTR, SIZE), true);
        mapStates = link(lookup, "horizon_accel_map_states", FunctionDescriptor.of(I32, PTR, PTR, SIZE), true);
        mapBiomes = link(lookup, "horizon_accel_map_biomes", FunctionDescriptor.of(I32, PTR, PTR, SIZE), true);
        generate = link(lookup, "horizon_accel_generate", FunctionDescriptor.of(PTR, PTR, I32, I32), false);
        chunkBounds = link(lookup, "horizon_accel_chunk_bounds", FunctionDescriptor.ofVoid(PTR, PTR, PTR), true);
        chunkBlocks = link(lookup, "horizon_accel_chunk_blocks", FunctionDescriptor.of(I32, PTR, PTR, PTR, SIZE), true);
        chunkSection = link(lookup, "horizon_accel_chunk_section",
                FunctionDescriptor.of(I32, PTR, PTR, I32, PTR, SIZE, PTR, SIZE, PTR), true);
        chunkBiomes = link(lookup, "horizon_accel_chunk_biomes", FunctionDescriptor.of(I32, PTR, PTR, PTR, SIZE), true);
        chunkPostprocessing = link(lookup, "horizon_accel_chunk_postprocessing",
                FunctionDescriptor.of(SIZE, PTR, I32, PTR, SIZE), true);
        chunkFree = link(lookup, "horizon_accel_chunk_free", FunctionDescriptor.ofVoid(PTR), false);
        stageTimings = link(lookup, "horizon_accel_stage_timings", FunctionDescriptor.of(I32, PTR, PTR, SIZE), true);
        decorate = link(lookup, "horizon_accel_decorate", FunctionDescriptor.of(PTR, PTR, I32, I32, PTR, SIZE, PTR, SIZE), false);
        decorationSizes = link(lookup, "horizon_accel_decoration_sizes", FunctionDescriptor.ofVoid(PTR, PTR), true);
        decorationCopy = link(lookup, "horizon_accel_decoration_copy", FunctionDescriptor.of(I32, PTR, PTR, SIZE, PTR, SIZE), true);
        decorationFree = link(lookup, "horizon_accel_decoration_free", FunctionDescriptor.ofVoid(PTR), false);
        chunkKeep = link(lookup, "horizon_accel_chunk_keep", FunctionDescriptor.of(I32, PTR, PTR, I64, I32, PTR, SIZE), true);
        forget = link(lookup, "horizon_accel_forget", FunctionDescriptor.ofVoid(PTR, PTR, SIZE), true);
        biomes = link(lookup, "horizon_accel_biomes", FunctionDescriptor.of(I32, PTR, I32, I32, PTR, SIZE), false);
        deflate = link(lookup, "horizon_accel_deflate", FunctionDescriptor.of(I64, PTR, SIZE, PTR, SIZE), false);
        deflateBound = link(lookup, "horizon_accel_deflate_bound", FunctionDescriptor.of(SIZE, SIZE), true);
        inflate = link(lookup, "horizon_accel_inflate", FunctionDescriptor.of(I64, PTR, SIZE, PTR, SIZE), false);
    }

    private static MethodHandle link(SymbolLookup lookup, String name, FunctionDescriptor descriptor, boolean critical) {
        MemorySegment symbol = lookup.find(name).orElseThrow(() -> new UnsatisfiedLinkError(name));
        return critical
                ? Linker.nativeLinker().downcallHandle(symbol, descriptor, Linker.Option.critical(true))
                : Linker.nativeLinker().downcallHandle(symbol, descriptor);
    }

    /** Loads the library at {@code path} for the life of the JVM, and checks it speaks this ABI. */
    public static HorizonAccel load(Path path) {
        HorizonAccel accel = new HorizonAccel(SymbolLookup.libraryLookup(path, Arena.global()));
        int abi = (int) call(() -> (int) accel.abi.invokeExact());
        if (abi != ABI) {
            throw new IllegalStateException("horizon-accel speaks ABI " + abi + ", this mod " + ABI);
        }
        return accel;
    }

    /**
     * Compresses {@code src} into {@code dst} as one zlib stream at {@code Deflater}'s default level, through
     * zlib-rs; the bytes written, or -1 when {@code dst} is smaller than {@link #deflateBound}. Both must be native
     * memory: compressing a chunk takes a fraction of a millisecond, too long to hold the garbage collector off.
     */
    public long deflate(MemorySegment src, MemorySegment dst) {
        return (long) call(() -> (long) deflate.invokeExact(src, src.byteSize(), dst, dst.byteSize()));
    }

    /** The output size {@link #deflate} always fits {@code length} bytes in (zlib's {@code compressBound}). */
    public long deflateBound(long length) {
        return (long) call(() -> (long) deflateBound.invokeExact(length));
    }

    /**
     * Decompresses the zlib stream at the start of {@code src} into {@code dst} (bytes after the stream's end are
     * ignored): the bytes written when the stream ended, -1 when {@code dst} filled first, -2 when {@code src} is not
     * a whole zlib stream. Native memory only, as for {@link #deflate}.
     */
    public long inflate(MemorySegment src, MemorySegment dst) {
        return (long) call(() -> (long) inflate.invokeExact(src, src.byteSize(), dst, dst.byteSize()));
    }

    /** One dimension's generator. Its methods are safe to call from many worldgen threads at once. */
    public final class Generator implements AutoCloseable {
        private final MemorySegment handle;

        private Generator(MemorySegment handle) {
            this.handle = handle;
        }

        /** Every Horizon block state, as vanilla's {@code BlockStateParser} reads it. Index is Horizon's id. */
        public String[] stateNames() {
            int count = (int) call(() -> (int) stateCount.invokeExact(handle));
            String[] names = new String[count];
            for (int i = 0; i < count; i++) {
                int id = i;
                names[i] = text((buffer, cap) -> (long) stateName.invokeExact(handle, id, buffer, cap));
            }
            return names;
        }

        public String[] biomeNames() {
            int count = (int) call(() -> (int) biomeCount.invokeExact(handle));
            String[] names = new String[count];
            for (int i = 0; i < count; i++) {
                int id = i;
                names[i] = text((buffer, cap) -> (long) biomeName.invokeExact(handle, id, buffer, cap));
            }
            return names;
        }

        /** The caller's registry id for each of {@link #stateNames()}. Once, before {@link #generate}. */
        public void mapStates(int[] ids) {
            check((int) call(() -> (int) mapStates.invokeExact(handle, MemorySegment.ofArray(ids), (long) ids.length)));
        }

        public void mapBiomes(int[] ids) {
            check((int) call(() -> (int) mapBiomes.invokeExact(handle, MemorySegment.ofArray(ids), (long) ids.length)));
        }

        /** Chunk ({@code x}, {@code z}) up to vanilla's carvers status, in the caller's ids once mapped. */
        public Chunk generate(int x, int z) {
            Generated generated = generateKeepable(x, z);
            generated.free();
            return generated.chunk();
        }

        /**
         * As {@link #generate}, with Horizon's copy still open: {@link Generated#keep} hands it back for decorating
         * the chunk later without copying it in, {@link Generated#free} drops it. One of them must be called.
         */
        public Generated generateKeepable(int x, int z) {
            MemorySegment chunk = (MemorySegment) call(() -> (MemorySegment) generate.invokeExact(handle, x, z));
            if (chunk.equals(MemorySegment.NULL)) {
                throw new IllegalStateException(lastError());
            }
            boolean done = false;
            try {
                int[] bounds = new int[2];
                MemorySegment minY = MemorySegment.ofArray(bounds).asSlice(0, 4);
                MemorySegment height = MemorySegment.ofArray(bounds).asSlice(4, 4);
                call(() -> {
                    chunkBounds.invokeExact(chunk, minY, height);
                    return null;
                });
                int sections = bounds[1] / 16;
                Section[] packed = new Section[sections];
                int[] palette = new int[4096];
                long[] words = new long[4096];
                long[] sizes = new long[3];
                for (int s = 0; s < sections; s++) {
                    int section = s;
                    int status = (int) call(() -> (int) chunkSection.invokeExact(handle, chunk, section,
                            MemorySegment.ofArray(palette), (long) palette.length, MemorySegment.ofArray(words),
                            (long) words.length, MemorySegment.ofArray(sizes)));
                    if (status != 0) {
                        throw new IllegalStateException(status == 1 ? "section larger than 4096 entries" : lastError());
                    }
                    packed[s] = new Section(java.util.Arrays.copyOf(palette, (int) sizes[0]),
                            java.util.Arrays.copyOf(words, (int) sizes[1]), (int) sizes[2]);
                }
                int[] biomes = new int[sections * 64];
                check((int) call(() -> (int) chunkBiomes.invokeExact(handle, chunk, MemorySegment.ofArray(biomes), (long) biomes.length)));
                short[][] post = new short[sections][];
                for (int s = 0; s < sections; s++) {
                    int section = s;
                    long n = (long) call(() -> (long) chunkPostprocessing.invokeExact(chunk, section, MemorySegment.NULL, 0L));
                    post[s] = new short[(int) n];
                    if (n > 0) {
                        MemorySegment out = MemorySegment.ofArray(post[s]);
                        call(() -> (long) chunkPostprocessing.invokeExact(chunk, section, out, n));
                    }
                }
                done = true;
                return new Generated(new Chunk(x, z, bounds[0], bounds[1], packed, biomes, post), chunk);
            } finally {
                if (!done) {
                    call(() -> {
                        chunkFree.invokeExact(chunk);
                        return null;
                    });
                }
            }
        }

        /** A generated chunk with Horizon's own copy still open. */
        public final class Generated {
            private final Chunk chunk;
            private MemorySegment native_;

            private Generated(Chunk chunk, MemorySegment native_) {
                this.chunk = chunk;
                this.native_ = native_;
            }

            public Chunk chunk() {
                return chunk;
            }

            /**
             * Keeps Horizon's copy under {@code token}, with the heightmaps the caller's copy ended up with: bit
             * {@code i} of {@code mask} for vanilla's {@code Heightmap.Types} ordinal {@code i}, their raw words in
             * order in {@code words}.
             */
            public void keep(long token, int mask, long[] words) {
                MemorySegment kept = take();
                check((int) call(() -> (int) chunkKeep.invokeExact(handle, kept, token, mask,
                        MemorySegment.ofArray(words), (long) words.length)));
            }

            public void free() {
                MemorySegment freed = take();
                call(() -> {
                    chunkFree.invokeExact(freed);
                    return null;
                });
            }

            private MemorySegment take() {
                if (native_ == null) {
                    throw new IllegalStateException("already kept or freed");
                }
                MemorySegment taken = native_;
                native_ = null;
                return taken;
            }
        }

        /**
         * Chunk ({@code x}, {@code z})'s biomes alone, 64 per section from the bottom in vanilla's order
         * ({@code y << 4 | z << 2 | x}), as the game's ids once mapped, into {@code out}. Horizon keeps them for
         * generating the chunk.
         */
        public void biomes(int x, int z, int[] out) {
            // Not a critical call (it can take a millisecond), so the output goes through native memory.
            try (Arena arena = Arena.ofConfined()) {
                MemorySegment ids = arena.allocate(ValueLayout.JAVA_INT, out.length);
                check((int) call(() -> (int) biomes.invokeExact(handle, x, z, ids, (long) out.length)));
                MemorySegment.copy(ids, ValueLayout.JAVA_INT, 0, out, 0, out.length);
            }
        }

        /** Drops the chunks kept under {@code tokens}. */
        public void forget(long[] tokens) {
            call(() -> {
                forget.invokeExact(handle, MemorySegment.ofArray(tokens), (long) tokens.length);
                return null;
            });
        }

        /**
         * {@code [chunks, biomes, noise, surface, structures, carvers]} so far: Horizon's own time in each stage, in
         * nanoseconds summed over threads.
         */
        public long[] stageNanos() {
            long[] out = new long[6];
            check((int) call(() -> (int) stageTimings.invokeExact(handle, MemorySegment.ofArray(out), (long) out.length)));
            return out;
        }

        /**
         * Decorates chunk ({@code cx}, {@code cz}) (vanilla's FEATURES status) on a copy of its 3×3 region, given as
         * the first {@code intCount} of {@code ints} and {@code longCount} of {@code longs} (layout in
         * {@code crates/horizon-accel/src/decorate.rs}), and returns the writes for the caller to repeat. Null when a
         * chunk named as kept is no longer kept: send them all in full.
         */
        public Decoration decorate(int cx, int cz, int[] ints, int intCount, long[] longs, int longCount) {
            MemorySegment result;
            try (Arena arena = Arena.ofConfined()) {
                MemorySegment in = arena.allocate(I32, Math.max(1, intCount));
                MemorySegment.copy(ints, 0, in, I32, 0, intCount);
                MemorySegment words = arena.allocate(I64, Math.max(1, longCount));
                MemorySegment.copy(longs, 0, words, I64, 0, longCount);
                result = (MemorySegment) call(() -> (MemorySegment) decorate.invokeExact(handle, cx, cz, in,
                        (long) intCount, words, (long) longCount));
            }
            if (result.equals(MemorySegment.NULL)) {
                throw new IllegalStateException(lastError());
            }
            try {
                long[] sizes = new long[6];
                call(() -> {
                    decorationSizes.invokeExact(result, MemorySegment.ofArray(sizes));
                    return null;
                });
                if (sizes[5] != 0) {
                    return null;
                }
                int[] ops = new int[(int) sizes[0]];
                byte[] strings = new byte[(int) sizes[1]];
                check((int) call(() -> (int) decorationCopy.invokeExact(result, MemorySegment.ofArray(ops),
                        (long) ops.length, MemorySegment.ofArray(strings), (long) strings.length)));
                return new Decoration(ops, strings, (int) sizes[2], sizes[3], sizes[4]);
            } finally {
                call(() -> {
                    decorationFree.invokeExact(result);
                    return null;
                });
            }
        }

        @Override
        public void close() {
            call(() -> {
                HorizonAccel.this.close.invokeExact(handle);
                return null;
            });
        }
    }

    /**
     * A generated chunk: its sections from the bottom; {@code biomes} 64 per section
     * ({@code y << 4 | z << 2 | x}); {@code postprocessing} each section's marked positions, packed as vanilla
     * packs them.
     */
    public record Chunk(int x, int z, int minY, int height, Section[] sections, int[] biomes, short[][] postprocessing) {
        /** Every block as the game's ids, 4096 per section in vanilla's order ({@code y << 8 | z << 4 | x}). */
        public int[] blocks() {
            int[] blocks = new int[sections.length * 4096];
            for (int s = 0; s < sections.length; s++) {
                sections[s].unpack(blocks, s * 4096);
            }
            return blocks;
        }
    }

    /**
     * One section as vanilla's {@code PalettedContainer} stores it: a palette of the game's ids and indices
     * packed into longs, {@code bits} each ({@code SimpleBitStorage}: no index spans two longs). No words
     * (and 0 bits): every block is {@code palette[0]}.
     */
    public record Section(int[] palette, long[] words, int bits) {
        public void unpack(int[] out, int offset) {
            if (bits == 0) {
                java.util.Arrays.fill(out, offset, offset + 4096, palette[0]);
                return;
            }
            int perWord = 64 / bits;
            long mask = (1L << bits) - 1;
            for (int i = 0; i < 4096; i++) {
                out[offset + i] = palette[(int) ((words[i / perWord] >>> (i % perWord * bits)) & mask)];
            }
        }
    }

    /**
     * A decorated region's writes, {@link #RECORD} ints each ({@code [kind, x, y, z, a, b, c]}, kinds in
     * {@code decorate.rs}), and the strings they name (NUL-terminated, at byte offsets). {@code unsupported}: writes
     * the caller can't repeat or features Horizon couldn't run; nonzero means decorate with vanilla instead.
     */
    public record Decoration(int[] ops, byte[] strings, int unsupported, long copyNanos, long decorateNanos) {
        public static final int RECORD = 7;

        public String string(int offset) {
            int end = offset;
            while (strings[end] != 0) {
                end++;
            }
            return new String(strings, offset, end - offset, StandardCharsets.UTF_8);
        }

        /** Binary NBT ({@code length} bytes at {@code offset}), as vanilla's {@code NbtIo.readAnyTag} reads it. */
        public java.io.DataInputStream bytes(int offset, int length) {
            return new java.io.DataInputStream(new java.io.ByteArrayInputStream(strings, offset, length));
        }
    }

    /** Opens a generator over a prepared resources folder (its {@code facts.json} and {@code worldgen/}). */
    public Generator open(Path resources, long seed, String dimension, boolean structures) {
        try (Arena arena = Arena.ofConfined()) {
            byte[] path = resources.toString().getBytes(StandardCharsets.UTF_8);
            byte[] dim = dimension.getBytes(StandardCharsets.UTF_8);
            MemorySegment pathSegment = arena.allocate(Math.max(1, path.length));
            MemorySegment dimSegment = arena.allocate(Math.max(1, dim.length));
            pathSegment.copyFrom(MemorySegment.ofArray(path));
            dimSegment.copyFrom(MemorySegment.ofArray(dim));
            MemorySegment handle = (MemorySegment) call(() -> (MemorySegment) open.invokeExact(
                    pathSegment, (long) path.length, seed, dimSegment, (long) dim.length, structures ? 1 : 0));
            if (handle.equals(MemorySegment.NULL)) {
                throw new IllegalStateException(lastError());
            }
            return new Generator(handle);
        }
    }

    /** This thread's last error from the library. */
    public String lastError() {
        return text((buffer, cap) -> (long) error.invokeExact(buffer, cap));
    }

    @FunctionalInterface
    interface Copy {
        long into(MemorySegment buffer, long cap) throws Throwable;
    }

    /** Reads a string the library copies out, retrying once with the length it reports. */
    private static String text(Copy copy) {
        byte[] buffer = new byte[256];
        long n = (long) call(() -> copy.into(MemorySegment.ofArray(buffer), buffer.length));
        if (n <= buffer.length) {
            return new String(buffer, 0, (int) n, StandardCharsets.UTF_8);
        }
        byte[] bigger = new byte[(int) n];
        call(() -> copy.into(MemorySegment.ofArray(bigger), bigger.length));
        return new String(bigger, StandardCharsets.UTF_8);
    }

    private void check(int status) {
        if (status != 0) {
            throw new IllegalStateException(lastError());
        }
    }

    @FunctionalInterface
    interface Call {
        Object run() throws Throwable;
    }

    static Object call(Call call) {
        try {
            return call.run();
        } catch (RuntimeException | Error e) {
            throw e;
        } catch (Throwable t) {
            throw new IllegalStateException(t);
        }
    }
}
