package dev.horizonlab.accel.mixin;

import dev.horizonlab.accel.mc.FastSave;
import java.io.DataOutput;
import java.io.IOException;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.NbtIo;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/** Chunk saves write their tag through {@link FastSave}, when it is on. */
@Mixin(NbtIo.class)
abstract class NbtIoMixin {
    @Inject(method = "write(Lnet/minecraft/nbt/CompoundTag;Ljava/io/DataOutput;)V", at = @At("HEAD"), cancellable = true)
    private static void horizonAccel$write(CompoundTag tag, DataOutput output, CallbackInfo info) throws IOException {
        if (FastSave.write(tag, output)) {
            info.cancel();
        }
    }
}
