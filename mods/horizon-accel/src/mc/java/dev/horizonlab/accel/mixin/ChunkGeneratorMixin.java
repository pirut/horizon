package dev.horizonlab.accel.mixin;

import dev.horizonlab.accel.mc.AccelMod;
import dev.horizonlab.accel.mc.FeatureSwap;
import dev.horizonlab.accel.mc.TerrainSwap;
import java.util.concurrent.CompletableFuture;
import net.minecraft.util.Util;
import net.minecraft.server.level.WorldGenRegion;
import net.minecraft.world.level.StructureManager;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.ChunkGenerator;
import net.minecraft.world.level.levelgen.RandomState;
import net.minecraft.world.level.levelgen.blending.Blender;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfoReturnable;

/**
 * The BIOMES status from Horizon's terrain run, and the FEATURES status decided by Horizon's Rust and written by
 * vanilla's code, when {@link AccelMod} allows them.
 */
@Mixin(ChunkGenerator.class)
abstract class ChunkGeneratorMixin {
    @Inject(method = "createBiomes", at = @At("HEAD"), cancellable = true)
    private void horizonAccel$createBiomes(RandomState random, Blender blender, StructureManager structures,
            ChunkAccess chunk, CallbackInfoReturnable<CompletableFuture<ChunkAccess>> result) {
        TerrainSwap swap = AccelMod.biomesFor((ChunkGenerator) (Object) this, blender, chunk);
        if (swap != null) {
            result.setReturnValue(CompletableFuture.supplyAsync(() -> swap.biomes(chunk),
                    Util.backgroundExecutor().forName("createBiomes")));
        }
    }

    @Inject(method = "applyBiomeDecoration", at = @At("HEAD"), cancellable = true)
    private void horizonAccel$applyBiomeDecoration(WorldGenLevel level, ChunkAccess chunk, StructureManager structures,
            CallbackInfo info) {
        if (level instanceof WorldGenRegion region) {
            FeatureSwap swap = AccelMod.featuresFor((ChunkGenerator) (Object) this, region, structures);
            if (swap != null && swap.decorate(region, chunk)) {
                info.cancel();
            }
        }
    }
}
