package dev.horizonlab.accel.mc;

import dev.horizonlab.accel.HorizonAccel;
import dev.horizonlab.accel.IdMap;
import it.unimi.dsi.fastutil.shorts.ShortArrayList;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.EnumSet;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.stream.LongStream;
import net.minecraft.commands.arguments.blocks.BlockStateParser;
import net.minecraft.core.Holder;
import net.minecraft.core.QuartPos;
import net.minecraft.core.Registry;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.LevelChunkSection;
import net.minecraft.world.level.chunk.PalettedContainer;
import net.minecraft.world.level.chunk.PalettedContainerRO;
import net.minecraft.world.level.chunk.Strategy;
import net.minecraft.world.level.levelgen.Heightmap;

/**
 * Vanilla's TERRAIN status (noise, surface, carvers) done by Horizon's Rust: the chunk comes back packed as
 * vanilla's {@code PalettedContainer} stores it, so each section is built whole, the way chunk loading builds one,
 * instead of 4096 {@code setBlockState} calls.
 *
 * <p>What the status leaves behind, and how it gets here: the blocks (each section replaced), the
 * post-processing marks (aquifer fluids that must tick, as vanilla's {@code markPosForPostprocessing}), and the
 * two world-generation heightmaps, primed from the final blocks, which is what vanilla's incremental updates
 * arrive at.
 *
 * <p>The BIOMES status comes from Horizon too ({@link #biomes}): its biome stage alone, written through vanilla's own
 * {@code fillBiomesFromNoise} so the containers come out as vanilla builds them. Horizon keeps those biomes, so
 * generating the chunk's terrain (which reads the 3×3 around it) doesn't sample them again.
 */
public final class TerrainSwap {
    private static final Strategy<BlockState> STATES = Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY);
    private static final EnumSet<Heightmap.Types> WORLDGEN_HEIGHTMAPS =
            EnumSet.of(Heightmap.Types.OCEAN_FLOOR_WG, Heightmap.Types.WORLD_SURFACE_WG);

    private final HorizonAccel.Generator generator;
    /** Horizon block state names the game has no state for: each one turns the swap off. */
    private final List<String> unmapped;

    /** The game's biome for each registry id. */
    private final Holder<Biome>[] biomeHolders;

    private TerrainSwap(HorizonAccel.Generator generator, List<String> unmapped, Holder<Biome>[] biomeHolders) {
        this.generator = generator;
        this.unmapped = unmapped;
        this.biomeHolders = biomeHolders;
    }

    /**
     * Opens Horizon's generator for one dimension and maps its block states and biomes to the live registries by
     * name, so mod-added entries and their shifted numeric ids don't matter.
     */
    public static TerrainSwap open(HorizonAccel accel, Path resources, long seed, String dimension, boolean structures,
            Registry<Biome> biomes) {
        HorizonAccel.Generator generator = accel.open(resources, seed, dimension, structures);
        Map<String, Integer> parsed = new HashMap<>();
        IdMap states = IdMap.of(generator.stateNames(), name -> parsed.computeIfAbsent(name, TerrainSwap::stateId));
        generator.mapStates(states.ids());
        IdMap biomeIds = IdMap.of(generator.biomeNames(), name -> {
            Identifier id = Identifier.tryParse(name);
            return id == null ? -1 : biomes.getOptional(id).map(biomes::getId).orElse(-1);
        });
        generator.mapBiomes(biomeIds.ids());
        List<String> unmapped = new ArrayList<>(states.missing());
        @SuppressWarnings({"unchecked", "rawtypes"})
        Holder<Biome>[] holders = new Holder[biomes.size()];
        for (int id = 0; id < holders.length; id++) {
            holders[id] = biomes.get(id).orElse(null);
        }
        return new TerrainSwap(generator, unmapped, holders);
    }

    private static int stateId(String name) {
        try {
            BlockState state = BlockStateParser.parseForBlock(BuiltInRegistries.BLOCK, name, false).blockState();
            return Block.getId(state);
        } catch (Exception e) {
            return -1;
        }
    }

    private volatile FeatureSwap features;

    /** The FEATURES status over the same generator. */
    public FeatureSwap features() {
        if (features == null) {
            synchronized (this) {
                if (features == null) {
                    features = new FeatureSwap(generator);
                }
            }
        }
        return features;
    }

    /** Why the swap can't run, if it can't: the block states Horizon makes that this game doesn't have. */
    public List<String> problems() {
        return unmapped.isEmpty() ? List.of() : List.of(unmapped.size() + " block states unmapped, first "
                + unmapped.getFirst());
    }

    /** Fills {@code chunk}'s biomes as vanilla's {@code createBiomes} would, and returns it. */
    public ChunkAccess biomes(ChunkAccess chunk) {
        int sections = chunk.getSectionsCount();
        int[] ids = new int[sections * 64];
        generator.biomes(chunk.getPos().x(), chunk.getPos().z(), ids);
        int minQuartY = QuartPos.fromBlock(chunk.getMinY());
        int quartX = QuartPos.fromBlock(chunk.getPos().getMinBlockX());
        int quartZ = QuartPos.fromBlock(chunk.getPos().getMinBlockZ());
        chunk.fillBiomesFromNoise((x, y, z) -> {
            int qy = Math.clamp(y - minQuartY, 0, sections * 4 - 1);
            return biomeHolders[ids[qy << 4 | (z - quartZ) << 2 | (x - quartX)]];
        });
        return chunk;
    }

    /** Fills {@code chunk} as vanilla's {@code buildTerrain} would, and returns it. */
    public ChunkAccess fill(ChunkAccess chunk) {
        long start = System.nanoTime();
        HorizonAccel.Generator.Generated kept = generator.generateKeepable(chunk.getPos().x(), chunk.getPos().z());
        long generatedAt = System.nanoTime();
        try {
            write(kept.chunk(), chunk);
        } catch (RuntimeException | Error e) {
            kept.free();
            throw e;
        }
        // Horizon keeps its copy for decorating the chunk, unless nothing will decorate it there.
        FeatureSwap decorating = features;
        if (decorating != null) {
            int[] mask = new int[1];
            long[] words = FeatureSwap.heightmaps(chunk, mask);
            kept.keep(decorating.track(chunk), mask[0], words);
        } else {
            kept.free();
        }
        stats.add(generatedAt - start, System.nanoTime() - generatedAt);
        return chunk;
    }

    private static void write(HorizonAccel.Chunk generated, ChunkAccess chunk) {
        LevelChunkSection[] sections = chunk.getSections();
        int offset = (generated.minY() - chunk.getMinY()) >> 4;
        for (int s = 0; s < generated.sections().length; s++) {
            int index = s + offset;
            if (index < 0 || index >= sections.length) {
                continue;
            }
            HorizonAccel.Section packed = generated.sections()[s];
            List<BlockState> palette = new ArrayList<>(packed.palette().length);
            for (int id : packed.palette()) {
                palette.add(Block.stateById(id));
            }
            var data = new PalettedContainerRO.PackedData<>(palette,
                    packed.bits() == 0 ? Optional.empty() : Optional.of(LongStream.of(packed.words())), packed.bits());
            PalettedContainer<BlockState> states = PalettedContainer.unpack(STATES, data).getOrThrow();
            sections[index] = new LevelChunkSection(states, sections[index].getBiomes());
            short[] marks = generated.postprocessing()[s];
            if (marks.length > 0) {
                chunk.addPackedPostProcess(new ShortArrayList(marks), index);
            }
        }
        Heightmap.primeHeightmaps(chunk, WORLDGEN_HEIGHTMAPS);
    }

    /** Time spent per chunk, in Rust and in writing its result into the chunk, logged every 4096 chunks. */
    private final Stats stats = new Stats();

    private static String ms(double nanos, double n) {
        return String.format("%.2f", nanos / 1e6 / n);
    }

    private final class Stats {
        private final java.util.concurrent.atomic.AtomicLong chunks = new java.util.concurrent.atomic.AtomicLong();
        private final java.util.concurrent.atomic.LongAdder rust = new java.util.concurrent.atomic.LongAdder();
        private final java.util.concurrent.atomic.LongAdder write = new java.util.concurrent.atomic.LongAdder();

        void add(long rustNanos, long writeNanos) {
            rust.add(rustNanos);
            write.add(writeNanos);
            long n = chunks.incrementAndGet();
            if (n % 4096 == 0) {
                long[] stages = generator.stageNanos();
                double c = Math.max(1, stages[0]);
                AccelMod.LOG.info("horizon-accel: {} chunks of terrain, {} ms each in Rust (biomes {}, noise {}, surface {}, "
                        + "structures {}, carvers {}), {} ms writing them", n, ms(rust.sum(), n), ms(stages[1], c),
                        ms(stages[2], c), ms(stages[3], c), ms(stages[4], c), ms(stages[5], c), ms(write.sum(), n));
            }
        }
    }
}
