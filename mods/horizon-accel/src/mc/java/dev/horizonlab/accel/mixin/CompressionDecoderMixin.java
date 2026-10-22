package dev.horizonlab.accel.mixin;

import dev.horizonlab.accel.mc.FastZlib;
import io.netty.buffer.ByteBuf;
import io.netty.channel.ChannelHandlerContext;
import java.util.List;
import net.minecraft.network.CompressionDecoder;
import net.minecraft.network.VarInt;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/** Compressed packets are decompressed by {@link FastZlib}, when it is on. */
@Mixin(CompressionDecoder.class)
abstract class CompressionDecoderMixin {
    @Shadow
    private int threshold;
    @Shadow
    private boolean validateDecompressed;

    @Inject(method = "decode", at = @At("HEAD"), cancellable = true)
    private void horizonAccel$decode(ChannelHandlerContext ctx, ByteBuf in, List<Object> out, CallbackInfo info) {
        int start = in.readerIndex();
        int length = VarInt.read(in);
        // Uncompressed packets, and the sizes vanilla rejects, stay with vanilla's code (from the same reader index).
        boolean ours = length > 0 && length <= 8388608 && !(validateDecompressed && length < threshold);
        ByteBuf packet = ours ? FastZlib.decode(ctx, in, length) : null;
        if (packet == null) {
            in.readerIndex(start);
            return;
        }
        out.add(packet);
        info.cancel();
    }
}
