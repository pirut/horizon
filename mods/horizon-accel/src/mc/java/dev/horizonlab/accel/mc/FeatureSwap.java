package dev.horizonlab.accel.mc;

import dev.horizonlab.accel.HorizonAccel;
import java.lang.invoke.MethodHandle;
import java.lang.invoke.MethodHandles;
import java.lang.invoke.MethodType;
import java.lang.invoke.VarHandle;
import java.util.Arrays;
import java.util.List;
import java.util.Map;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.NbtAccounter;
import net.minecraft.nbt.NbtIo;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.level.WorldGenRegion;
import net.minecraft.util.BitStorage;
import net.minecraft.util.ProblemReporter;
import net.minecraft.util.RandomSource;
import net.minecraft.world.RandomizableContainer;
import net.minecraft.world.entity.EntitySpawnReason;
import net.minecraft.world.entity.EntitySpawnRequest;
import net.minecraft.world.entity.EntityType;
import net.minecraft.world.entity.Mob;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.entity.SpawnerBlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.GlobalPalette;
import net.minecraft.world.level.chunk.LevelChunkSection;
import net.minecraft.world.level.chunk.Palette;
import net.minecraft.world.level.chunk.PalettedContainer;
import net.minecraft.world.level.chunk.Strategy;
import net.minecraft.world.level.levelgen.Heightmap;
import net.minecraft.world.level.material.Fluid;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.level.storage.TagValueInput;

/**
 * Vanilla's FEATURES status ({@code ChunkGenerator.applyBiomeDecoration}) decided by Horizon's Rust and written by
 * vanilla's own code. The 3×3 region goes to Rust as vanilla stores it (each section's palette and packed indices,
 * each heightmap's raw words); Rust decorates its copy and returns every write it made, in order, as the calls that
 * make them in Java: {@code WorldGenRegion.setBlock}, section writes, scheduled ticks, post-processing marks,
 * spawner and loot-table setup. Replaying those makes the block entities, ticks and heightmaps vanilla makes.
 *
 * <p>A region whose decoration needs a write Java can't repeat yet or a feature Horizon can't run comes back unsupported before anything is written, and vanilla
 * decorates it instead.
 */
public final class FeatureSwap {
    private static final Strategy<BlockState> STATES = Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY);
    private static final Heightmap.Types[] HEIGHTMAPS = Heightmap.Types.values();
    /** Horizon's {@code FluidKind::ALL}, in order. */
    private static final Fluid[] FLUIDS = {Fluids.EMPTY, Fluids.FLOWING_WATER, Fluids.WATER, Fluids.FLOWING_LAVA, Fluids.LAVA};
    private static final VarHandle DATA;
    private static final MethodHandle STORAGE, PALETTE;

    static {
        try {
            Class<?> data = Class.forName("net.minecraft.world.level.chunk.PalettedContainer$Data");
            DATA = MethodHandles.privateLookupIn(PalettedContainer.class, MethodHandles.lookup())
                    .findVarHandle(PalettedContainer.class, "data", data);
            var lookup = MethodHandles.privateLookupIn(data, MethodHandles.lookup());
            STORAGE = lookup.findVirtual(data, "storage", MethodType.methodType(BitStorage.class))
                    .asType(MethodType.methodType(BitStorage.class, Object.class));
            PALETTE = lookup.findVirtual(data, "palette", MethodType.methodType(Palette.class))
                    .asType(MethodType.methodType(Palette.class, Object.class));
        } catch (ReflectiveOperationException e) {
            throw new ExceptionInInitializerError(e);
        }
    }

    private final HorizonAccel.Generator generator;
    private final Stats stats = new Stats();

    FeatureSwap(HorizonAccel.Generator generator) {
        this.generator = generator;
    }

    /** Chunks decorated so far and where their time went. */
    public String summary() {
        return stats.summary();
    }

    /** One thread's copy buffers, grown as needed. */
    private static final class Buffers {
        int[] ints = new int[1 << 16];
        long[] longs = new long[1 << 16];
        int intCount, longCount;

        void put(int value) {
            if (intCount == ints.length) {
                ints = Arrays.copyOf(ints, ints.length * 2);
            }
            ints[intCount++] = value;
        }

        void put(long[] values) {
            if (longCount + values.length > longs.length) {
                longs = Arrays.copyOf(longs, Math.max(longs.length * 2, longCount + values.length));
            }
            System.arraycopy(values, 0, longs, longCount, values.length);
            longCount += values.length;
        }
    }

    private static final ThreadLocal<Buffers> BUFFERS = ThreadLocal.withInitial(Buffers::new);

    /**
     * Decorates {@code chunk}, the center of {@code region}, as {@code applyBiomeDecoration} would. False when
     * nothing was written and vanilla must decorate it.
     */
    public boolean decorate(WorldGenRegion region, ChunkAccess chunk) {
        long start = System.nanoTime();
        int cx = chunk.getPos().x(), cz = chunk.getPos().z();
        ChunkAccess[] chunks = new ChunkAccess[9];
        long[] tokens = new long[9];
        HorizonAccel.Decoration decoration = null;
        try {
            for (int i = 0; i < 9; i++) {
                chunks[i] = region.getChunk(cx + i % 3 - 1, cz + i / 3 - 1);
            }
            // Once more in full if Horizon no longer has a chunk it was said to have.
            for (int attempt = 0; decoration == null && attempt < 2; attempt++) {
                Buffers buffers = BUFFERS.get();
                buffers.intCount = buffers.longCount = 0;
                for (int i = 0; i < 9; i++) {
                    Long kept = attempt == 0 ? keptTokens.get(chunks[i]) : null;
                    tokens[i] = kept != null ? kept : track(chunks[i]);
                    buffers.put(kept != null ? 1 : 0);
                    buffers.put((int) (tokens[i] >>> 32));
                    buffers.put((int) tokens[i]);
                    if (kept == null) {
                        copy(chunks[i], buffers);
                    }
                }
                decoration = generator.decorate(cx, cz, buffers.ints, buffers.intCount, buffers.longs, buffers.longCount);
            }
            if (decoration == null) {
                throw new IllegalStateException("kept chunks went missing twice");
            }
        } catch (RuntimeException e) {
            untrack(chunks, tokens);
            stats.add(false, System.nanoTime() - start, 0, 0, 0);
            AccelMod.LOG.warn("horizon-accel: vanilla decorates chunk {} {}: {}", cx, cz, e.getMessage());
            return false;
        }
        long rustAt = System.nanoTime();
        if (decoration.unsupported() > 0) {
            untrack(chunks, tokens);
            stats.add(false, rustAt - start, 0, decoration.copyNanos(), decoration.decorateNanos());
            return false;
        }
        replay(region, chunks, cx, cz, decoration);
        stats.add(true, rustAt - start, System.nanoTime() - rustAt, decoration.copyNanos(), decoration.decorateNanos());
        return true;
    }

    /**
     * Chunk objects whose copy Horizon keeps, by the token it keeps it under. A chunk is in here while Horizon's copy
     * matches it: every write to it since came from a replay of Horizon's own decoration.
     */
    private final Map<ChunkAccess, Long> keptTokens = java.util.Collections.synchronizedMap(new java.util.WeakHashMap<>());
    private final java.util.concurrent.atomic.AtomicLong nextToken = new java.util.concurrent.atomic.AtomicLong();

    /** A new token for {@code chunk}, whose copy is about to be handed to Horizon. */
    long track(ChunkAccess chunk) {
        long token = nextToken.incrementAndGet();
        keptTokens.put(chunk, token);
        return token;
    }

    /** Vanilla decorates these chunks' region instead, so Horizon's copies stop matching them. */
    private void untrack(ChunkAccess[] chunks, long[] tokens) {
        for (ChunkAccess chunk : chunks) {
            if (chunk != null) {
                keptTokens.remove(chunk);
            }
        }
        generator.forget(tokens);
    }

    /** {@code chunk}'s heightmaps as region input describes them: the mask, then their words in order. */
    static long[] heightmaps(ChunkAccess chunk, int[] mask) {
        long[][] maps = new long[HEIGHTMAPS.length][];
        for (Map.Entry<Heightmap.Types, Heightmap> entry : chunk.getHeightmaps()) {
            maps[entry.getKey().ordinal()] = entry.getValue().getRawData();
        }
        mask[0] = 0;
        int words = 0;
        for (int i = 0; i < maps.length; i++) {
            if (maps[i] != null) {
                mask[0] |= 1 << i;
                words += maps[i].length;
            }
        }
        long[] out = new long[words];
        int at = 0;
        for (long[] map : maps) {
            if (map != null) {
                System.arraycopy(map, 0, out, at, map.length);
                at += map.length;
            }
        }
        return out;
    }

    /** One chunk of the region input, after its token: undecorated, its heightmaps, then its sections. */
    private static void copy(ChunkAccess chunk, Buffers out) {
        out.put(0);
        int[] mask = new int[1];
        out.put(heightmaps(chunk, mask));
        out.put(mask[0]);
        for (LevelChunkSection section : chunk.getSections()) {
            PalettedContainer<BlockState> states = section.getStates();
            Object data = DATA.getVolatile(states);
            BitStorage storage;
            Palette<BlockState> palette;
            try {
                storage = (BitStorage) STORAGE.invokeExact(data);
                @SuppressWarnings("unchecked")
                Palette<BlockState> p = (Palette<BlockState>) PALETTE.invokeExact(data);
                palette = p;
            } catch (Throwable t) {
                throw new IllegalStateException(t);
            }
            if (palette instanceof GlobalPalette) {
                // Over 256 states in one section: send the compact form vanilla saves.
                var packed = states.pack(STATES);
                List<BlockState> entries = packed.paletteEntries();
                long[] words = packed.storage().map(s -> s.toArray()).orElse(new long[0]);
                out.put(entries.size());
                out.put(packed.bitsPerEntry());
                out.put(words.length);
                for (BlockState state : entries) {
                    out.put(Block.getId(state));
                }
                out.put(words);
                continue;
            }
            long[] words = storage.getRaw();
            int size = palette.getSize();
            out.put(size);
            out.put(storage.getBits());
            out.put(words.length);
            for (int i = 0; i < size; i++) {
                out.put(Block.getId(palette.valueFor(i)));
            }
            out.put(words);
        }
    }

    /** {@code chunks}: the region's, row by row around ({@code cx}, {@code cz}), for the section writes. */
    private static void replay(WorldGenRegion region, ChunkAccess[] chunks, int cx, int cz,
            HorizonAccel.Decoration decoration) {
        int[] ops = decoration.ops();
        var pos = new BlockPos.MutableBlockPos();
        // Most writes are ore veins, one section after another: keep the last section at hand.
        LevelChunkSection section = null;
        long sectionKey = Long.MIN_VALUE;
        for (int i = 0; i < ops.length; i += HorizonAccel.Decoration.RECORD) {
            int x = ops[i + 1], y = ops[i + 2], z = ops[i + 3], a = ops[i + 4], b = ops[i + 5];
            if (ops[i] == 3) {
                long key = (long) (x >> 4) << 42 | (long) (y >> 4 & 0x1fffff) << 21 | (z >> 4 & 0x1fffff);
                if (key != sectionKey) {
                    ChunkAccess chunk = chunks[(x >> 4) - cx + 1 + 3 * ((z >> 4) - cz + 1)];
                    section = chunk.getSection(chunk.getSectionIndex(y));
                    sectionKey = key;
                }
                section.setBlockState(x & 15, y & 15, z & 15, Block.stateById(a), false);
                continue;
            }
            pos.set(x, y, z);
            switch (ops[i]) {
                case 1 -> region.setBlock(pos, Block.stateById(a), b, 512);
                case 4 -> region.scheduleTick(pos.immutable(), Block.stateById(a).getBlock(), b);
                case 5 -> region.scheduleTick(pos.immutable(), FLUIDS[a], b);
                case 6 -> region.getChunk(x >> 4, z >> 4).markPosForPostProcessing(pos.immutable());
                case 7 -> {
                    if (region.getBlockEntity(pos) instanceof SpawnerBlockEntity spawner) {
                        var type = BuiltInRegistries.ENTITY_TYPE.getValue(Identifier.parse(decoration.string(a)));
                        // A new spawner has no spawn potentials to draw from, so the random is never read.
                        spawner.setEntityId(type, RandomSource.create(0));
                    }
                }
                case 8 -> {
                    if (region.getBlockEntity(pos) instanceof RandomizableContainer container) {
                        long seed = (long) b << 32 | (ops[i + 6] & 0xffffffffL);
                        container.setLootTable(ResourceKey.create(Registries.LOOT_TABLE,
                                Identifier.parse(decoration.string(a))), seed);
                    }
                }
                case 9 -> region.getChunk(x >> 4, z >> 4).getHeight(HEIGHTMAPS[a], x & 15, z & 15);
                case 10 -> {
                    var blockEntity = region.getBlockEntity(pos);
                    if (blockEntity != null) {
                        blockEntity.loadWithComponents(TagValueInput.create(ProblemReporter.DISCARDING,
                                region.registryAccess(), nbt(decoration, a, b)));
                    }
                }
                case 11 -> addEntity(region, nbt(decoration, a, b));
                default -> throw new IllegalStateException("unknown decoration record " + ops[i]);
            }
        }
    }

    private static CompoundTag nbt(HorizonAccel.Decoration decoration, int offset, int length) {
        try {
            return (CompoundTag) NbtIo.readAnyTag(decoration.bytes(offset, length), NbtAccounter.unlimitedHeap());
        } catch (java.io.IOException e) {
            throw new java.io.UncheckedIOException(e);
        }
    }

    /**
     * StructureTemplate.placeEntities and the structure pieces that spawn mobs: the entity loads from its NBT (place
     * and turn already applied), mobs that vanilla finalizes ({@code horizon:finalize}) are, and it joins the region.
     */
    @SuppressWarnings("deprecation") // the region's level, as StructureTemplate creates entities in it
    private static void addEntity(WorldGenRegion region, CompoundTag tag) {
        boolean finalize = tag.contains("horizon:finalize");
        tag.remove("horizon:finalize");
        java.util.Optional<net.minecraft.world.entity.Entity> created;
        try {
            created = EntityType.create(TagValueInput.create(ProblemReporter.DISCARDING, region.registryAccess(), tag),
                    region.getLevel(), new EntitySpawnRequest(EntitySpawnReason.STRUCTURE, false));
        } catch (Exception e) {
            // StructureTemplate.createEntityIgnoreException: an entity that fails to load is left out.
            created = java.util.Optional.empty();
        }
        created.ifPresent(entity -> {
            float yaw = entity.getYRot();
            entity.snapTo(entity.getX(), entity.getY(), entity.getZ(), yaw, entity.getXRot());
            entity.setYBodyRot(yaw);
            entity.setYHeadRot(yaw);
            if (finalize && entity instanceof Mob mob) {
                mob.finalizeSpawn(region, region.getCurrentDifficultyAt(BlockPos.containing(mob.position())),
                        EntitySpawnReason.STRUCTURE, null);
            }
            region.addFreshEntityWithPassengers(entity);
        });
    }

    /** Regions Horizon decorated and the time they took, logged every 4096 chunks. */
    private static final class Stats {
        private final java.util.concurrent.atomic.AtomicLong chunks = new java.util.concurrent.atomic.AtomicLong();
        private final java.util.concurrent.atomic.LongAdder swapped = new java.util.concurrent.atomic.LongAdder();
        private final java.util.concurrent.atomic.LongAdder rust = new java.util.concurrent.atomic.LongAdder();
        private final java.util.concurrent.atomic.LongAdder replay = new java.util.concurrent.atomic.LongAdder();
        private final java.util.concurrent.atomic.LongAdder copy = new java.util.concurrent.atomic.LongAdder();
        private final java.util.concurrent.atomic.LongAdder decorate = new java.util.concurrent.atomic.LongAdder();

        void add(boolean done, long rustNanos, long replayNanos, long copyNanos, long decorateNanos) {
            if (done) {
                swapped.increment();
            }
            rust.add(rustNanos);
            replay.add(replayNanos);
            copy.add(copyNanos);
            decorate.add(decorateNanos);
            long n = chunks.incrementAndGet();
            if (n % 4096 == 0) {
                AccelMod.LOG.info("horizon-accel: {}", summary());
            }
        }

        String summary() {
            long n = Math.max(1, chunks.get());
            return String.format("%d chunks decorated, %d by Horizon; per chunk %s ms copying and in Rust (Rust copy %s, "
                    + "decorate %s), %s ms replaying", chunks.get(), swapped.sum(), ms(rust.sum(), n), ms(copy.sum(), n),
                    ms(decorate.sum(), n), ms(replay.sum(), Math.max(1, swapped.sum())));
        }

        private static String ms(double nanos, double n) {
            return String.format("%.2f", nanos / 1e6 / n);
        }
    }
}
