package dev.horizonlab.accel.mixin;

import dev.horizonlab.accel.mc.FastZlib;
import io.netty.buffer.ByteBuf;
import io.netty.channel.ChannelHandlerContext;
import net.minecraft.network.CompressionEncoder;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/** Packets at or over the threshold are compressed by {@link FastZlib}, when it is on. */
@Mixin(CompressionEncoder.class)
abstract class CompressionEncoderMixin {
    @Shadow
    private int threshold;

    @Inject(method = "encode(Lio/netty/channel/ChannelHandlerContext;Lio/netty/buffer/ByteBuf;Lio/netty/buffer/ByteBuf;)V",
            at = @At("HEAD"), cancellable = true)
    private void horizonAccel$encode(ChannelHandlerContext ctx, ByteBuf in, ByteBuf out, CallbackInfo info) {
        int size = in.readableBytes();
        // Vanilla's own code throws for oversized packets and writes small ones uncompressed.
        if (size <= 8388608 && size >= threshold && FastZlib.encode(in, out)) {
            info.cancel();
        }
    }
}
