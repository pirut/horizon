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
    public static final int ABI = 2;

    private static final ValueLayout.OfInt I32 = ValueLayout.JAVA_INT;
    private static final ValueLayout.OfLong I64 = ValueLayout.JAVA_LONG;
    private static final ValueLayout SIZE = ValueLayout.JAVA_LONG;
    private static final ValueLayout PTR = ValueLayout.ADDRESS;

    final MethodHandle abi, noop, noopCritical, sum, error, open, close, stateCount, stateName,
            biomeCount, biomeName, mapStates, mapBiomes, generate, chunkBounds, chunkBlocks, chunkSection, chunkBiomes,
            chunkPostprocessing, chunkFree, stageTimings;

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
            MemorySegment chunk = (MemorySegment) call(() -> (MemorySegment) generate.invokeExact(handle, x, z));
            if (chunk.equals(MemorySegment.NULL)) {
                throw new IllegalStateException(lastError());
            }
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
                return new Chunk(x, z, bounds[0], bounds[1], packed, biomes, post);
            } finally {
                call(() -> {
                    chunkFree.invokeExact(chunk);
                    return null;
                });
            }
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
