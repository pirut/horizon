package dev.horizonlab.accel.mixin;

import com.mojang.serialization.Codec;
import dev.horizonlab.accel.mc.FastSave;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.world.level.chunk.PalettedContainerFactory;
import net.minecraft.world.level.chunk.storage.SerializableChunkData;
import org.spongepowered.asm.mixin.Final;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Redirect;

/** Chunk saves encode section palettes through {@link FastSave}, when it is on. */
@Mixin(SerializableChunkData.class)
abstract class SerializableChunkDataMixin {
    @Shadow @Final private PalettedContainerFactory containerFactory;

    @Redirect(method = "write", at = @At(value = "INVOKE",
            target = "Lnet/minecraft/nbt/CompoundTag;store(Ljava/lang/String;Lcom/mojang/serialization/Codec;Ljava/lang/Object;)V"))
    @SuppressWarnings({"unchecked", "rawtypes"})
    private void horizonAccel$store(CompoundTag tag, String key, Codec codec, Object value) {
        if (!FastSave.store(tag, key, codec, value, containerFactory)) {
            tag.store(key, codec, value);
        }
    }
}
