package dev.horizonlab.accel.bench;

import java.nio.file.*;
import java.util.*;
import java.util.concurrent.*;
import java.util.concurrent.atomic.AtomicInteger;
import dev.horizonlab.accel.HorizonAccel;
import dev.horizonlab.accel.mc.TerrainSwap;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.RegistryDataLoader;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.RegistryLayer;
import net.minecraft.server.level.GenerationChunkHolder;
import net.minecraft.server.level.ServerChunkCache;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.level.WorldGenRegion;
import net.minecraft.server.packs.PackType;
import net.minecraft.server.packs.repository.ServerPacksSource;
import net.minecraft.server.packs.resources.MultiPackResourceManager;
import net.minecraft.tags.TagLoader;
import net.minecraft.util.StaticCache2D;
import net.minecraft.util.datafix.DataFixers;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.*;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.chunk.status.ChunkPyramid;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.blending.Blender;
import net.minecraft.world.level.levelgen.structure.*;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplateManager;
import net.minecraft.world.level.storage.LevelStorageSource;

/**
 * Vanilla terrain generation timed in-process, the Java half of the go/no-go
 * benchmark in docs/research/java-bridge.md. Its Rust half is
 * crates/horizon-world/examples/chunkgen_bench.rs.
 *
 * The level scaffolding is VanillaChunkGenerationOracle's (mods/horizon): real
 * WorldGenRegions over an allocation-only ServerLevel, so each status runs
 * vanilla's own code. A round brings a fresh SIZE x SIZE square of chunks
 * through STRUCTURE_STARTS, STRUCTURE_REFERENCES, BIOMES and TERRAIN (noise,
 * surface, carvers), with THREADS workers per status, as the server's worldgen
 * pool would. Each status is timed on its own and divided by the chunks it ran
 * on (starts and biomes run on a margin around the square), so "total" is a
 * chunk's steady-state cost up to carvers and "terrain" is exactly the work a
 * terrain swap replaces.
 *
 * usage: VanillaTerrainBench SEED DIMENSION SIZE THREADS WARMUP ROUNDS
 * prints one "round" line per measured round and a "median" line.
 */
public final class VanillaTerrainBench {
    private static final class Cache extends ServerChunkCache {
        private Cache() {
            super(null, null, null, null, null, null, 0, 0, false, null);
        }
        static ChunkGeneratorStructureState state;
        static RandomState random;
        static ChunkGenerator generator;
        @Override
        public ChunkGeneratorStructureState getGeneratorState() {
            return state;
        }
        @Override
        public RandomState randomState() {
            return random;
        }
        @Override
        public ChunkGenerator getGenerator() {
            return generator;
        }
    }

    private static final class BenchLevel extends ServerLevel {
        private BenchLevel() {
            super(null, null, null, null, null, null, false, 0L, null, false);
        }
        static long seed;
        static RegistryAccess access;
        static net.minecraft.world.level.dimension.DimensionType type;
        static StructureManager structures;
        static BiomeResolver resolver;
        static ServerChunkCache cache;
        static net.minecraft.server.MinecraftServer server;
        static ResourceKey<Level> key;
        static int minY, height;
        static Map<Long, ProtoChunk> chunks;
        @Override
        public net.minecraft.server.MinecraftServer getServer() {
            return server;
        }
        // Scheduled ticks read the game time; generation starts at 0.
        static final net.minecraft.world.level.storage.LevelData DATA = new net.minecraft.world.level.storage.LevelData() {
            public RespawnData getRespawnData() { return null; }
            public long getGameTime() { return 0; }
            public boolean isHardcore() { return false; }
            public net.minecraft.world.Difficulty getDifficulty() { return net.minecraft.world.Difficulty.NORMAL; }
            public boolean isDifficultyLocked() { return false; }
        };
        @Override
        public net.minecraft.world.level.storage.LevelData getLevelData() {
            return DATA;
        }
        static final java.util.concurrent.atomic.AtomicInteger IDS = new java.util.concurrent.atomic.AtomicInteger();
        @Override
        public long getOverworldClockTime() {
            return 0;
        }
        @Override
        public long getDefaultClockTime() {
            return 0;
        }
        // Lighting runs after FEATURES, so every light read during it is 0.
        static final net.minecraft.world.level.lighting.LevelLightEngine LIGHT = new net.minecraft.world.level.lighting.LevelLightEngine(
                new net.minecraft.world.level.chunk.LightChunkGetter() {
                    public net.minecraft.world.level.chunk.LightChunk getChunkForLighting(int x, int z) { return null; }
                    public net.minecraft.world.level.BlockGetter getLevel() { return null; }
                }, false, false);
        @Override
        public net.minecraft.world.level.lighting.LevelLightEngine getLightEngine() {
            return LIGHT;
        }
        // The server queues POI updates for its main thread; nothing reads them here.
        @Override
        public void updatePOIOnBlockStateChange(net.minecraft.core.BlockPos pos, net.minecraft.world.level.block.state.BlockState oldState, net.minecraft.world.level.block.state.BlockState newState) {
        }
        @Override
        public float getMoonBrightness(net.minecraft.core.BlockPos pos) {
            return 1.0f;
        }
        @Override
        public int getNextEntityId() {
            return IDS.incrementAndGet();
        }
        @Override
        public long getSeed() {
            return seed;
        }
        @Override
        public net.minecraft.world.flag.FeatureFlagSet enabledFeatures() {
            return net.minecraft.world.flag.FeatureFlags.DEFAULT_FLAGS;
        }
        @Override
        public RegistryAccess registryAccess() {
            return access;
        }
        @Override
        public net.minecraft.world.level.dimension.DimensionType dimensionType() {
            return type;
        }
        @Override
        public StructureManager structureManager() {
            return structures;
        }
        @Override
        public BiomeResolver uncachedBiomeResolver() {
            return resolver;
        }
        @Override
        public ServerChunkCache getChunkSource() {
            return cache;
        }
        @Override
        public ResourceKey<Level> dimension() {
            return key;
        }
        @Override
        public int getMinY() {
            return minY;
        }
        @Override
        public int getHeight() {
            return height;
        }
        @Override
        public int getSeaLevel() {
            return Cache.generator.getSeaLevel();
        }
        @Override
        public ChunkAccess getChunk(int x, int z, ChunkStatus status, boolean load) {
            return chunks.get(ChunkPos.pack(x, z));
        }
    }

    private static final class Holder2 extends GenerationChunkHolder {
        final ProtoChunk chunk;
        Holder2(ProtoChunk chunk) {
            super(chunk.getPos());
            this.chunk = chunk;
        }
        @Override
        public ChunkAccess getChunkIfPresentUnchecked(ChunkStatus status) {
            return status.isOrBefore(chunk.getPersistedStatus()) ? chunk : null;
        }
        @Override
        protected void addSaveDependency(CompletableFuture<?> sync) {}
        @Override
        public int getTicketLevel() {
            return 0;
        }
        @Override
        public int getQueueLevel() {
            return 0;
        }
    }

    interface ChunkTask {
        void run(int x, int z) throws Exception;
    }

    /** Runs {@code task} over every chunk of the rectangle on {@code pool}, and returns the elapsed ns. */
    static long timed(ExecutorService pool, int threads, int x0, int z0, int x1, int z1, ChunkTask task) throws Exception {
        int width = x1 - x0 + 1, count = width * (z1 - z0 + 1);
        AtomicInteger next = new AtomicInteger();
        List<Future<?>> futures = new ArrayList<>();
        long start = System.nanoTime();
        for (int t = 0; t < threads; t++) {
            futures.add(pool.submit(() -> {
                for (int i; (i = next.getAndIncrement()) < count; ) task.run(x0 + i % width, z0 + i / width);
                return null;
            }));
        }
        for (var f : futures) f.get();
        return System.nanoTime() - start;
    }

    /** Each target chunk filled again by the swap, on a fresh chunk, compared with vanilla's block by block. */
    static void parity(TerrainSwap swap, Map<Long, ProtoChunk> chunks, int x0, int z0, int x1, int z1,
            LevelHeightAccessor height, PalettedContainerFactory factory) {
        int checked = 0, differ = 0;
        for (int z = z0; z <= z1; z++) for (int x = x0; x <= x1; x++) {
            ProtoChunk vanilla = chunks.get(ChunkPos.pack(x, z));
            ProtoChunk ours = new ProtoChunk(new ChunkPos(x, z), UpgradeData.EMPTY, height, factory, null);
            swap.fill(ours);
            checked++;
            String first = null;
            int blocks = 0;
            var pos = new BlockPos.MutableBlockPos();
            for (int y = height.getMinY(); y < height.getMaxY() + 1; y++) for (int dz = 0; dz < 16; dz++) for (int dx = 0; dx < 16; dx++) {
                pos.set((x << 4) + dx, y, (z << 4) + dz);
                var a = vanilla.getBlockState(pos);
                var b = ours.getBlockState(pos);
                if (a != b) {
                    blocks++;
                    if (first == null) first = pos.toShortString() + " vanilla " + a + " horizon " + b;
                }
            }
            for (int s = 0; s < vanilla.getPostProcessing().length; s++) {
                var a = vanilla.getPostProcessing()[s];
                var b = ours.getPostProcessing()[s];
                if (!java.util.Objects.equals(a == null || a.isEmpty() ? null : a, b == null || b.isEmpty() ? null : b)) {
                    if (first == null) first = "post-processing of section " + s + ": vanilla " + a + " horizon " + b;
                    blocks++;
                }
            }
            for (var type : new Heightmap.Types[] {Heightmap.Types.OCEAN_FLOOR_WG, Heightmap.Types.WORLD_SURFACE_WG}) {
                if (!Arrays.equals(vanilla.getOrCreateHeightmapUnprimed(type).getRawData(), ours.getOrCreateHeightmapUnprimed(type).getRawData())) {
                    if (first == null) first = "heightmap " + type;
                    blocks++;
                }
            }
            if (blocks > 0) {
                differ++;
                System.out.println("parity chunk " + x + " " + z + ": " + blocks + " differences, first " + first);
            }
        }
        System.out.println("parity " + (checked - differ) + "/" + checked + " chunks identical");
    }

    /**
     * Every chunk FEATURES wrote to (the square and its ring), decorated by vanilla in {@code a} and by the swap in
     * {@code b}: blocks, block entities, scheduled ticks, post-processing marks and heightmaps.
     */
    /** Where horizon-accel's palette packing differs from vanilla's on this chunk, or null. */
    static String packParity(ProtoChunk chunk, PalettedContainerFactory factory) {
        for (int s = 0; s < chunk.getSections().length; s++) {
            var section = chunk.getSections()[s];
            String a = samePack(section.getStates(), factory.blockStatesStrategy());
            if (a != null) return "block packing of section " + s + ": " + a;
            String b = samePack((net.minecraft.world.level.chunk.PalettedContainer<net.minecraft.core.Holder<Biome>>) section.getBiomes(), factory.biomeStrategy());
            if (b != null) return "biome packing of section " + s + ": " + b;
        }
        return null;
    }

    static <T> String samePack(net.minecraft.world.level.chunk.PalettedContainer<T> container, net.minecraft.world.level.chunk.Strategy<T> strategy) {
        var fast = dev.horizonlab.accel.mc.FastPack.packAlways(container, strategy);
        if (fast == null) return null;
        var vanilla = container.pack(strategy);
        long[] fa = fast.storage().map(java.util.stream.LongStream::toArray).orElse(null);
        long[] va = vanilla.storage().map(java.util.stream.LongStream::toArray).orElse(null);
        if (!fast.paletteEntries().equals(vanilla.paletteEntries()) || fast.bitsPerEntry() != vanilla.bitsPerEntry()
                || !java.util.Arrays.equals(fa, va)) {
            return "vanilla " + vanilla.paletteEntries() + " " + vanilla.bitsPerEntry() + " horizon " + fast.paletteEntries() + " " + fast.bitsPerEntry();
        }
        return null;
    }

    static void featureParity(Map<Long, ProtoChunk> a, Map<Long, ProtoChunk> b, int x0, int z0, int x1, int z1,
            LevelHeightAccessor height, RegistryAccess access) {
        int checked = 0, differ = 0;
        for (int z = z0 - 1; z <= z1 + 1; z++) for (int x = x0 - 1; x <= x1 + 1; x++) {
            ProtoChunk vanilla = a.get(ChunkPos.pack(x, z)), ours = b.get(ChunkPos.pack(x, z));
            checked++;
            List<String> problems = new ArrayList<>();
            var pos = new BlockPos.MutableBlockPos();
            int blocks = 0;
            for (int y = height.getMinY(); y < height.getMaxY() + 1; y++) for (int dz = 0; dz < 16; dz++) for (int dx = 0; dx < 16; dx++) {
                pos.set((x << 4) + dx, y, (z << 4) + dz);
                var va = vanilla.getBlockState(pos);
                var vb = ours.getBlockState(pos);
                if (va != vb && blocks++ == 0) problems.add(pos.toShortString() + " vanilla " + va + " horizon " + vb);
            }
            if (blocks > 1) problems.add(blocks + " blocks");
            String pack = packParity(vanilla, PalettedContainerFactory.create(access));
            if (pack == null) pack = packParity(ours, PalettedContainerFactory.create(access));
            if (pack != null) problems.add(pack);
            // Biomes, value by value and palette by palette (the palette's order is what a save writes).
            for (int s = 0; s < vanilla.getSections().length; s++) {
                var ba = vanilla.getSections()[s].getBiomes();
                var bb = ours.getSections()[s].getBiomes();
                String biomeProblem = null;
                for (int i = 0; i < 64 && biomeProblem == null; i++) {
                    if (!ba.get(i & 3, i >> 4, (i >> 2) & 3).equals(bb.get(i & 3, i >> 4, (i >> 2) & 3))) {
                        biomeProblem = "biome " + i + " of section " + s;
                    }
                }
                if (biomeProblem == null) {
                    var pa = new ArrayList<Object>();
                    var pb = new ArrayList<Object>();
                    ba.forEachInPalette(pa::add);
                    bb.forEachInPalette(pb::add);
                    if (!pa.equals(pb) || ba.bitsPerEntry() != bb.bitsPerEntry()) biomeProblem = "biome order of section " + s;
                }
                if (biomeProblem != null) {
                    problems.add(biomeProblem);
                    break;
                }
            }
            if (!vanilla.getBlockEntityNbts().equals(ours.getBlockEntityNbts())) {
                problems.add("block entity nbt: vanilla " + vanilla.getBlockEntityNbts() + " horizon " + ours.getBlockEntityNbts());
            }
            var entitiesA = new java.util.TreeMap<String, String>();
            var entitiesB = new java.util.TreeMap<String, String>();
            vanilla.getBlockEntities().forEach((p, e) -> entitiesA.put(p.toShortString(), e.saveWithFullMetadata(access).toString()));
            ours.getBlockEntities().forEach((p, e) -> entitiesB.put(p.toShortString(), e.saveWithFullMetadata(access).toString()));
            if (!entitiesA.equals(entitiesB)) problems.add("block entities: vanilla " + entitiesA + " horizon " + entitiesB);
            // Entities draw UUIDs and attribute bonuses from an unseeded random, so only kinds and places can match.
            java.util.function.Function<ProtoChunk, String> entities = c -> c.getEntities().stream()
                    .map(t -> t.getStringOr("id", "?") + t.get("Pos")).toList().toString();
            if (!entities.apply(vanilla).equals(entities.apply(ours))) problems.add("entities: vanilla " + entities.apply(vanilla) + " horizon " + entities.apply(ours));
            var ticksA = ((net.minecraft.world.ticks.ProtoChunkTicks<?>) vanilla.getBlockTicks()).scheduledTicks();
            var ticksB = ((net.minecraft.world.ticks.ProtoChunkTicks<?>) ours.getBlockTicks()).scheduledTicks();
            if (!ticksA.equals(ticksB)) problems.add("block ticks: vanilla " + ticksA.size() + " horizon " + ticksB.size());
            var fluidsA = ((net.minecraft.world.ticks.ProtoChunkTicks<?>) vanilla.getFluidTicks()).scheduledTicks();
            var fluidsB = ((net.minecraft.world.ticks.ProtoChunkTicks<?>) ours.getFluidTicks()).scheduledTicks();
            if (!fluidsA.equals(fluidsB)) problems.add("fluid ticks: vanilla " + fluidsA.size() + " horizon " + fluidsB.size());
            for (int s = 0; s < vanilla.getPostProcessing().length; s++) {
                var pa = vanilla.getPostProcessing()[s];
                var pb = ours.getPostProcessing()[s];
                if (!java.util.Objects.equals(pa == null || pa.isEmpty() ? null : pa, pb == null || pb.isEmpty() ? null : pb)) {
                    problems.add("post-processing of section " + s + ": vanilla " + pa + " horizon " + pb);
                }
            }
            for (var type : Heightmap.Types.values()) {
                boolean ha = vanilla.hasPrimedHeightmap(type), hb = ours.hasPrimedHeightmap(type);
                if (ha != hb || ha && !Arrays.equals(vanilla.getOrCreateHeightmapUnprimed(type).getRawData(), ours.getOrCreateHeightmapUnprimed(type).getRawData())) {
                    problems.add("heightmap " + type);
                }
            }
            if (!problems.isEmpty()) {
                differ++;
                String text = String.join("; ", problems);
                System.out.println("fparity chunk " + x + " " + z + ": " + (text.length() > 600 ? text.substring(0, 600) + "..." : text));
            }
        }
        System.out.println("fparity " + (checked - differ) + "/" + checked + " chunks identical");
    }

    public static void main(String[] args) throws Exception {
        long seed = Long.parseLong(args[0]);
        String dimension = args[1];
        int size = Integer.parseInt(args[2]);
        int threads = Integer.parseInt(args[3]);
        int warmup = Integer.parseInt(args[4]);
        int rounds = Integer.parseInt(args[5]);
        long setup = System.nanoTime();
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var repo = ServerPacksSource.createVanillaTrustedRepository();
        repo.reload();
        repo.setSelected(List.of("vanilla"));
        var resources = new MultiPackResourceManager(PackType.SERVER_DATA, repo.openAllSelected());
        var layers = RegistryLayer.createRegistryAccess();
        var staticTags = TagLoader.loadTagsForExistingRegistries(resources, layers.getLayer(RegistryLayer.STATIC));
        var context = TagLoader.buildUpdatedLookups(layers.getAccessForLoading(RegistryLayer.WORLD), staticTags);
        RegistryAccess.Frozen world = RegistryDataLoader.load(resources, context, RegistryDataLoader.WORLD_REGISTRIES, Runnable::run).join();
        var loaded = net.minecraft.server.ReloadableServerRegistries.reload(layers.replaceFrom(RegistryLayer.WORLD, world), staticTags, resources, Runnable::run).join();
        var components = BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(loaded.lookupWithUpdatedTags());
        staticTags.forEach(Registry.PendingTags::apply);
        components.forEach(net.minecraft.core.component.DataComponentInitializers.PendingComponents::apply);
        RegistryAccess access = loaded.layers().compositeAccess();
        var settings = world.lookupOrThrow(Registries.NOISE_SETTINGS).getOrThrow(switch (dimension) {
            case "the_nether" -> NoiseGeneratorSettings.NETHER;
            case "the_end" -> NoiseGeneratorSettings.END;
            default -> NoiseGeneratorSettings.OVERWORLD;
        });
        var presets = world.lookupOrThrow(Registries.MULTI_NOISE_BIOME_SOURCE_PARAMETER_LIST);
        BiomeSource biomeSource = switch (dimension) {
            case "the_nether" -> MultiNoiseBiomeSource.createFromPreset(presets.getOrThrow(MultiNoiseBiomeSourceParameterLists.NETHER));
            case "the_end" -> TheEndBiomeSource.create(world.lookupOrThrow(Registries.BIOME));
            default -> MultiNoiseBiomeSource.createFromPreset(presets.getOrThrow(MultiNoiseBiomeSourceParameterLists.OVERWORLD));
        };
        ResourceKey<Level> levelKey = switch (dimension) {
            case "the_nether" -> Level.NETHER;
            case "the_end" -> Level.END;
            default -> Level.OVERWORLD;
        };
        var randomState = RandomState.create(world.lookupOrThrow(Registries.NOISE), seed, settings.value());
        var generator = new NoiseBasedChunkGenerator(biomeSource, settings);
        var state = ChunkGeneratorStructureState.createForNormal(randomState, seed, new ChunkPos(0, 0), biomeSource, world.lookupOrThrow(Registries.STRUCTURE_SET));
        // The server places concentric-ring structures (strongholds) once, at startup, before any worker asks.
        state.ensureStructuresGenerated();
        Path tmp = Files.createTempDirectory("horizon-bench");
        var storage = LevelStorageSource.createDefault(tmp).createAccess("bench");
        var templates = new StructureTemplateManager(resources, storage, DataFixers.getDataFixer(), BuiltInRegistries.BLOCK);
        var dimensionType = world.lookupOrThrow(Registries.DIMENSION_TYPE).getOrThrow(switch (dimension) {
            case "the_nether" -> net.minecraft.world.level.dimension.BuiltinDimensionTypes.NETHER;
            case "the_end" -> net.minecraft.world.level.dimension.BuiltinDimensionTypes.END;
            default -> net.minecraft.world.level.dimension.BuiltinDimensionTypes.OVERWORLD;
        }).value();
        LevelHeightAccessor height = dimension.equals("overworld") ? LevelHeightAccessor.create(-64, 384) : LevelHeightAccessor.create(0, 256);
        var factory = PalettedContainerFactory.create(access);
        var unsafeField = Class.forName("sun.misc.Unsafe").getDeclaredField("theUnsafe");
        unsafeField.setAccessible(true);
        Object unsafe = unsafeField.get(null);
        var allocate = unsafe.getClass().getMethod("allocateInstance", Class.class);
        Map<Long, ProtoChunk> chunks = new ConcurrentHashMap<>();
        Cache.state = state;
        Cache.random = randomState;
        Cache.generator = generator;
        BenchLevel.seed = seed;
        BenchLevel.access = access;
        BenchLevel.type = dimensionType;
        BenchLevel.resolver = biomeSource.createUncachedResolver(randomState);
        BenchLevel.cache = (ServerChunkCache) allocate.invoke(unsafe, Cache.class);
        BenchLevel.server = (net.minecraft.server.MinecraftServer) allocate.invoke(unsafe, net.minecraft.server.dedicated.DedicatedServer.class);
        var templateField = net.minecraft.server.MinecraftServer.class.getDeclaredField("structureTemplateManager");
        templateField.setAccessible(true);
        templateField.set(BenchLevel.server, templates);
        BenchLevel.key = levelKey;
        BenchLevel.minY = height.getMinY();
        BenchLevel.height = height.getHeight();
        BenchLevel.chunks = chunks;
        var check = new StructureCheck((pos, visitor) -> CompletableFuture.completedFuture(null), access, templates, levelKey, generator, randomState, height, biomeSource, seed, DataFixers.getDataFixer());
        var level = (ServerLevel) allocate.invoke(unsafe, BenchLevel.class);
        BenchLevel.structures = new StructureManager(level, new WorldOptions(seed, true, false), check);
        var pyramid = ChunkPyramid.GENERATION_PYRAMID;
        java.util.function.BiFunction<Integer, Integer, ProtoChunk> chunkAt = (x, z) -> chunks.computeIfAbsent(ChunkPos.pack(x, z), k -> new ProtoChunk(new ChunkPos(x, z), UpgradeData.EMPTY, height, factory, null));
        java.util.function.BiFunction<ChunkStatus, ProtoChunk, WorldGenRegion> regionFor = (status, chunk) -> {
            var step = pyramid.getStepTo(status);
            int radius = step.directDependencies().size() - 1;
            var cache = StaticCache2D.<GenerationChunkHolder>create(chunk.getPos().x(), chunk.getPos().z(), radius, (x, z) -> new Holder2(chunkAt.apply(x, z)));
            return new WorldGenRegion(level, cache, step, chunk);
        };
        System.out.printf("setup %.1f ms, %d threads, %s, seed %d, %dx%d chunks per round%n",
                (System.nanoTime() - setup) / 1e6, threads, dimension, seed, size, size);

        System.out.println("worldgen fingerprint " + dev.horizonlab.accel.mc.WorldgenFingerprint.of(access));
        // HORIZON_ACCEL_LIB and HORIZON_ACCEL_RESOURCES: also open horizon-accel's terrain swap. HORIZON_ACCEL_MODE
        // "swap" times it in place of vanilla's TERRAIN status; "parity" compares it with vanilla's chunks.
        String mode = System.getenv().getOrDefault("HORIZON_ACCEL_MODE", "swap");
        boolean features = System.getenv("HORIZON_BENCH_FEATURES") != null;
        int[] origin = System.getenv("HORIZON_BENCH_ORIGIN") == null ? null
                : Arrays.stream(System.getenv("HORIZON_BENCH_ORIGIN").split(",")).mapToInt(Integer::parseInt).toArray();
        TerrainSwap opened = null;
        if (System.getenv("HORIZON_ACCEL_LIB") != null) {
            long start = System.nanoTime();
            opened = TerrainSwap.open(HorizonAccel.load(Path.of(System.getenv("HORIZON_ACCEL_LIB"))),
                    Path.of(System.getenv("HORIZON_ACCEL_RESOURCES")), seed, levelKey.identifier().toString(), true,
                    access.lookupOrThrow(Registries.BIOME));
            if (!opened.problems().isEmpty()) throw new IllegalStateException(String.join("; ", opened.problems()));
            System.out.printf("horizon-accel %s, opened in %.1f ms%n", mode, (System.nanoTime() - start) / 1e6);
        }
        TerrainSwap swap = opened;
        // Decoration starts with the first round, so terrain keeps Horizon's copies from the start.
        if (swap != null) swap.features();
        ExecutorService pool = Executors.newFixedThreadPool(threads, r -> {
            Thread thread = new Thread(r, "worldgen");
            thread.setDaemon(true);
            return thread;
        });
        double[][] results = new double[rounds][];
        // HORIZON_ACCEL_MODE=fparity: rounds in pairs over the same square, vanilla's terrain and decoration, then
        // the swaps' (terrain swap parity is checked separately), compared.
        boolean fparity = mode.equals("fparity");
        Map<Long, ProtoChunk> vanillaRun = new HashMap<>();
        long fallbacks = 0;
        for (int round = 0; round < warmup + rounds; round++) {
            chunks.clear();
            // A fresh square each round, far from the last, so nothing is reused.
            // HORIZON_BENCH_ORIGIN=X,Z: the measured rounds start at that chunk; warmup stays far away.
            int place = fparity ? round / 2 : round;
            int x0 = place * (size + 32), z0 = 0;
            if (origin != null) {
                x0 = round < warmup ? -10000 - x0 : origin[0] + ((fparity ? place - warmup / 2 : round - warmup)) * (size + 32);
                z0 = round < warmup ? 0 : origin[1];
            }
            boolean swapFeatures = swap != null && (fparity ? round % 2 == 1 : mode.equals("swap"));
            int x1 = x0 + size - 1, z1 = z0 + size - 1;
            int m = features ? 1 : 0;
            long starts = timed(pool, threads, x0 - 8 - m, z0 - 8 - m, x1 + 8 + m, z1 + 8 + m, (x, z) -> {
                var chunk = chunkAt.apply(x, z);
                generator.createStructures(access, state, BenchLevel.structures, chunk, templates, levelKey);
                chunk.setPersistedStatus(ChunkStatus.STRUCTURE_STARTS);
            });
            // HORIZON_BENCH_FEATURES: also decorate the square, so terrain and everything before it run one chunk further out.
            long references = timed(pool, threads, x0 - m, z0 - m, x1 + m, z1 + m, (x, z) -> {
                var chunk = chunkAt.apply(x, z);
                var region = regionFor.apply(ChunkStatus.STRUCTURE_REFERENCES, chunk);
                generator.createReferences(region, BenchLevel.structures.forWorldGenRegion(region), chunk);
                chunk.setPersistedStatus(ChunkStatus.STRUCTURE_REFERENCES);
            });
            // TERRAIN reads its neighbours' biomes, so biomes run one chunk further out.
            long biomes = timed(pool, threads, x0 - 1 - m, z0 - 1 - m, x1 + 1 + m, z1 + 1 + m, (x, z) -> {
                var chunk = chunkAt.apply(x, z);
                var region = regionFor.apply(ChunkStatus.BIOMES, chunk);
                generator.createBiomes(randomState, Blender.empty(), BenchLevel.structures.forWorldGenRegion(region), chunk).join();
                chunk.setPersistedStatus(ChunkStatus.BIOMES);
            });
            long terrain = timed(pool, threads, x0 - m, z0 - m, x1 + m, z1 + m, (x, z) -> {
                var chunk = chunkAt.apply(x, z);
                if (swap != null && (mode.equals("swap") || swapFeatures)) {
                    swap.fill(chunk);
                    chunk.setPersistedStatus(ChunkStatus.TERRAIN);
                    return;
                }
                var region = regionFor.apply(ChunkStatus.TERRAIN, chunk);
                Set<net.minecraft.core.Holder<Biome>> possible = new HashSet<>();
                for (int dz = -1; dz <= 1; dz++) for (int dx = -1; dx <= 1; dx++) region.getChunk(x + dx, z + dz).collectBiomesInPalette(possible);
                generator.buildTerrain(chunk, Blender.empty(), randomState, BenchLevel.structures.forWorldGenRegion(region), region.getBiomeManager(), region, possible).join();
                chunk.setPersistedStatus(ChunkStatus.TERRAIN);
            });
            if (swap != null && mode.equals("parity")) {
                parity(swap, chunks, x0, z0, x1, z1, height, factory);
            }
            // One thread in row order, as decoration order changes what neighbours see.
            long decorate = 0;
            if (features) {
                long start = System.nanoTime();
                for (int z = z0; z <= z1; z++) {
                    for (int x = x0; x <= x1; x++) {
                        var chunk = chunkAt.apply(x, z);
                        var region = regionFor.apply(ChunkStatus.FEATURES, chunk);
                        if (swapFeatures && swap.features().decorate(region, chunk)) {
                            chunk.setPersistedStatus(ChunkStatus.FEATURES);
                            continue;
                        }
                        if (swapFeatures) fallbacks++;
                        generator.applyBiomeDecoration(region, chunk, BenchLevel.structures.forWorldGenRegion(region));
                        chunk.setPersistedStatus(ChunkStatus.FEATURES);
                    }
                }
                decorate = System.nanoTime() - start;
                if (fparity && round % 2 == 0) {
                    vanillaRun = new HashMap<>(chunks);
                } else if (fparity) {
                    featureParity(vanillaRun, chunks, x0, z0, x1, z1, height, access);
                }
                if (swapFeatures) System.out.println("features: " + fallbacks + " chunks fell back to vanilla so far; " + swap.features().summary());
            }
            // Each status over the chunks it ran on: in a running server every chunk passes each status once.
            double n = size * size, ns = (size + 16 + 2 * m) * (size + 16 + 2 * m), nb = (size + 2 + 2 * m) * (size + 2 + 2 * m);
            double nt = (size + 2 * m) * (size + 2 * m);
            double[] r = {starts / 1e6 / ns, references / 1e6 / nt, biomes / 1e6 / nb, terrain / 1e6 / nt, decorate / 1e6 / n, 0};
            r[5] = r[0] + r[1] + r[2] + r[3] + r[4];
            System.out.printf("%s %d: starts %.3f refs %.3f biomes %.3f terrain %.3f features %.3f total %.3f ms/chunk%n",
                    round < warmup ? "warmup" : "round", round, r[0], r[1], r[2], r[3], r[4], r[5]);
            if (round >= warmup) results[round - warmup] = r;
        }
        String[] names = {"starts", "refs", "biomes", "terrain", "features", "total"};
        StringBuilder line = new StringBuilder("median");
        for (int i = 0; i < names.length; i++) {
            final int k = i;
            double[] column = Arrays.stream(results).mapToDouble(r -> r[k]).sorted().toArray();
            line.append(String.format(" %s %.3f", names[i], column[column.length / 2]));
        }
        System.out.println(line + " ms/chunk");
        pool.shutdown();
        storage.close();
        System.exit(0);
    }
}
