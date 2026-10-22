package dev.horizonlab.accel.mixin;

import dev.horizonlab.accel.mc.FastPack;
import net.minecraft.world.level.chunk.PalettedContainer;
import net.minecraft.world.level.chunk.PalettedContainerRO;
import net.minecraft.world.level.chunk.Strategy;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfoReturnable;

/** Chunk saves pack palettes through {@link FastPack}, when it is on. */
@Mixin(PalettedContainer.class)
abstract class PalettedContainerMixin<T> {
    @Inject(method = "pack", at = @At("HEAD"), cancellable = true)
    private void horizonAccel$pack(Strategy<T> strategy, CallbackInfoReturnable<PalettedContainerRO.PackedData<T>> result) {
        @SuppressWarnings("unchecked")
        var packed = FastPack.pack((PalettedContainer<T>) (Object) this, strategy);
        if (packed != null) {
            result.setReturnValue(packed);
        }
    }
}
