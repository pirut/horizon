package dev.horizonlab.accel.mixin;

import dev.horizonlab.accel.mc.FastZlib;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import net.minecraft.world.level.chunk.storage.RegionFileVersion;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfoReturnable;

/** Region files' zlib chunks (vanilla's default compression) go through {@link FastZlib}, when it is on. */
@Mixin(RegionFileVersion.class)
abstract class RegionFileVersionMixin {
    @Inject(method = "wrap(Ljava/io/OutputStream;)Ljava/io/OutputStream;", at = @At("HEAD"), cancellable = true)
    private void horizonAccel$deflate(OutputStream out, CallbackInfoReturnable<OutputStream> info) {
        if ((Object) this == RegionFileVersion.VERSION_DEFLATE) {
            OutputStream fast = FastZlib.deflating(out);
            if (fast != null) info.setReturnValue(fast);
        }
    }

    @Inject(method = "wrap(Ljava/io/InputStream;)Ljava/io/InputStream;", at = @At("HEAD"), cancellable = true)
    private void horizonAccel$inflate(InputStream in, CallbackInfoReturnable<InputStream> info) throws IOException {
        if ((Object) this == RegionFileVersion.VERSION_DEFLATE) {
            InputStream fast = FastZlib.inflating(in);
            if (fast != null) info.setReturnValue(fast);
        }
    }
}
