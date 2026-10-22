package dev.horizonlab.accel.mc;

import com.mojang.logging.LogUtils;
import dev.horizonlab.accel.HorizonAccel;
import dev.horizonlab.accel.Swaps;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import net.fabricmc.api.ModInitializer;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.level.WorldGenRegion;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.StructureManager;
import net.minecraft.world.level.biome.MultiNoiseBiomeSource;
import net.minecraft.world.level.biome.MultiNoiseBiomeSourceParameterLists;
import net.minecraft.world.level.biome.TheEndBiomeSource;
import net.minecraft.world.level.levelgen.NoiseBasedChunkGenerator;
import net.minecraft.world.level.levelgen.NoiseGeneratorSettings;
import net.minecraft.world.level.levelgen.blending.Blender;
import org.slf4j.Logger;

/**
 * horizon-accel's entry point. Loads the Rust library and, per dimension, decides whether Horizon generates its
 * terrain: only for vanilla's own generator settings and biome source, over vanilla's own world-generation data,
 * when no other mod patches {@code NoiseBasedChunkGenerator}. Anything else keeps vanilla's Java code, and the
 * log says why, once.
 *
 * <p>Settings are system properties: {@code horizon.accel.library}, the {@code libhorizon_accel} file;
 * {@code horizon.accel.resources}, the resources {@code horizon-prepare} made from the same game version;
 * {@code horizon.accel.terrain=false} turns the terrain swap off.
 */
public final class AccelMod implements ModInitializer {
    static final Logger LOG = LogUtils.getLogger();
    /** buildTerrain, and every method its work runs through. */
    static final Swaps.Swap TERRAIN = new Swaps.Swap("terrain",
            "net/minecraft/world/level/levelgen/NoiseBasedChunkGenerator", Set.of("buildTerrain",
                    "lambda$buildTerrain$0", "createNoiseChunk", "doFill", "buildSurface", "generateCarvers",
                    "applyCarvingMask", "getBiomeGenerationSettingsForCarver"));
    /**
     * Mixins on those methods that change how the work runs but never its result: C2ME pools the surface rule's
     * buffers and picks the executor terrain runs on.
     */
    static final Set<String> SAME_RESULTS = Set.of(
            "com.ishland.c2me.opts.allocs.mixin.object_pooling_caching.MixinNoiseChunkGenerator",
            "com.ishland.c2me.rewrites.chunksystem.mixin.MixinNoiseChunkGenerator");
    /** vanilla 26.3's world-generation data ({@link WorldgenFingerprint}), with nothing else loaded. */
    static final String VANILLA_WORLDGEN = "990509e188c0984b9eabf7ffe84712796fb8388ee12f242fc371b5f57fc9fc8c";

    private static volatile HorizonAccel library;
    private static Path resources;
    private static final Map<ResourceKey<Level>, TerrainSwap> SWAPS = new HashMap<>();
    private static final Set<ResourceKey<Level>> DECIDED = new HashSet<>();
    private static volatile Swaps.Decision terrain;

    @Override
    public void onInitialize() {
        if (!Boolean.parseBoolean(System.getProperty("horizon.accel.terrain", "true"))) {
            LOG.info("horizon-accel: terrain swap off (horizon.accel.terrain=false)");
            return;
        }
        String lib = System.getProperty("horizon.accel.library");
        String res = System.getProperty("horizon.accel.resources");
        if (lib == null || res == null || !Files.isRegularFile(Path.of(lib)) || !Files.isDirectory(Path.of(res))) {
            LOG.warn("horizon-accel: off. Set -Dhorizon.accel.library=<libhorizon_accel> and "
                    + "-Dhorizon.accel.resources=<horizon-prepare resources>");
            return;
        }
        try {
            library = HorizonAccel.load(Path.of(lib));
            resources = Path.of(res);
            LOG.info("horizon-accel: loaded {}", lib);
        } catch (RuntimeException | UnsatisfiedLinkError e) {
            LOG.error("horizon-accel: off, the library didn't load", e);
        }
    }

    /** Horizon's terrain for this chunk's dimension, or null to run vanilla's. Called on worldgen threads. */
    public static TerrainSwap terrainFor(NoiseBasedChunkGenerator generator, Holder<NoiseGeneratorSettings> settings,
            Blender blender, WorldGenRegion region, StructureManager structures) {
        if (library == null || blender != Blender.empty()) {
            return null;
        }
        @SuppressWarnings("deprecation") // the region's level, which worldgen threads may read but not change
        ResourceKey<Level> dimension = region.getLevel().dimension();
        synchronized (SWAPS) {
            if (DECIDED.add(dimension)) {
                SWAPS.put(dimension, decide(generator, settings, region, dimension, structures.shouldGenerateStructures()));
            }
            return SWAPS.get(dimension);
        }
    }

    private static TerrainSwap decide(NoiseBasedChunkGenerator generator, Holder<NoiseGeneratorSettings> settings,
            WorldGenRegion region, ResourceKey<Level> dimension, boolean structures) {
        List<String> problems = new ArrayList<>();
        if (terrain == null) {
            terrain = Swaps.decide(TERRAIN, MixinTargets.of(TERRAIN.targetClass()), "dev.horizonlab.accel.",
                    SAME_RESULTS, Map.of());
        }
        problems.addAll(terrain.reasons());
        String name = horizonDimension(generator, settings, dimension);
        if (name == null) {
            problems.add("not vanilla's generator settings and biome source for " + dimension.identifier());
        }
        var access = region.registryAccess();
        String fingerprint = WorldgenFingerprint.of(access);
        if (!VANILLA_WORLDGEN.equals(fingerprint) && !Boolean.getBoolean("horizon.accel.anyWorldgen")) {
            problems.add("world-generation data differs from vanilla's (a data pack or mod changes it), " + fingerprint);
        }
        TerrainSwap swap = null;
        if (problems.isEmpty()) {
            try {
                swap = TerrainSwap.open(library, resources, region.getSeed(), name, structures,
                        access.lookupOrThrow(Registries.BIOME));
                problems.addAll(swap.problems());
            } catch (RuntimeException e) {
                problems.add("Horizon's generator didn't open: " + e.getMessage());
            }
        }
        if (!problems.isEmpty()) {
            LOG.info("horizon-accel: vanilla terrain for {}: {}", dimension.identifier(), String.join("; ", problems));
            return null;
        }
        LOG.info("horizon-accel: Horizon generates terrain for {}", dimension.identifier());
        return swap;
    }

    /** Horizon's name for the dimension when the generator is exactly vanilla's for it, else null. */
    static String horizonDimension(NoiseBasedChunkGenerator generator, Holder<NoiseGeneratorSettings> settings,
            ResourceKey<Level> dimension) {
        var source = generator.getBiomeSource();
        if (dimension == Level.OVERWORLD && settings.is(NoiseGeneratorSettings.OVERWORLD)
                && source instanceof MultiNoiseBiomeSource multi && multi.stable(MultiNoiseBiomeSourceParameterLists.OVERWORLD)) {
            return "minecraft:overworld";
        }
        if (dimension == Level.NETHER && settings.is(NoiseGeneratorSettings.NETHER)
                && source instanceof MultiNoiseBiomeSource multi && multi.stable(MultiNoiseBiomeSourceParameterLists.NETHER)) {
            return "minecraft:the_nether";
        }
        if (dimension == Level.END && settings.is(NoiseGeneratorSettings.END) && source instanceof TheEndBiomeSource) {
            return "minecraft:the_end";
        }
        return null;
    }
}
