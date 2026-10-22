package dev.horizonlab.accel.mixin;

import dev.horizonlab.accel.mc.AccelMod;
import dev.horizonlab.accel.mc.FeatureSwap;
import net.minecraft.server.level.WorldGenRegion;
import net.minecraft.world.level.StructureManager;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.ChunkGenerator;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/** The FEATURES status decided by Horizon's Rust and written by vanilla's code, when {@link AccelMod} allows it. */
@Mixin(ChunkGenerator.class)
abstract class ChunkGeneratorMixin {
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
