package dev.horizonlab.accel.mixin;

import dev.horizonlab.accel.mc.AccelMod;
import dev.horizonlab.accel.mc.TerrainSwap;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import net.minecraft.core.Holder;
import net.minecraft.server.level.WorldGenRegion;
import net.minecraft.util.Util;
import net.minecraft.world.level.StructureManager;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.biome.BiomeManager;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.levelgen.NoiseBasedChunkGenerator;
import net.minecraft.world.level.levelgen.NoiseGeneratorSettings;
import net.minecraft.world.level.levelgen.RandomState;
import net.minecraft.world.level.levelgen.blending.Blender;
import org.spongepowered.asm.mixin.Final;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfoReturnable;

/** The TERRAIN status (noise, surface, carvers) from Horizon's Rust, when {@link AccelMod} allows it. */
@Mixin(NoiseBasedChunkGenerator.class)
abstract class NoiseBasedChunkGeneratorMixin {
    @Shadow
    @Final
    private Holder<NoiseGeneratorSettings> settings;

    @Inject(method = "buildTerrain", at = @At("HEAD"), cancellable = true)
    private void horizonAccel$buildTerrain(ChunkAccess chunk, Blender blender, RandomState random,
            StructureManager structures, BiomeManager biomes, WorldGenRegion region, Set<Holder<Biome>> possible,
            CallbackInfoReturnable<CompletableFuture<ChunkAccess>> result) {
        TerrainSwap swap = AccelMod.terrainFor((NoiseBasedChunkGenerator) (Object) this, settings, blender, region, structures);
        if (swap != null) {
            result.setReturnValue(CompletableFuture.supplyAsync(() -> swap.fill(chunk),
                    Util.backgroundExecutor().forName("horizon_accel_terrain")));
        }
    }
}
