package dev.horizonlab.accel.mc;

import com.mojang.serialization.Codec;
import dev.horizonlab.accel.Swaps;
import java.io.DataOutput;
import java.io.DataOutputStream;
import java.io.IOException;
import java.io.UTFDataFormatException;
import java.lang.invoke.MethodHandles;
import java.lang.invoke.VarHandle;
import java.nio.ByteOrder;
import java.util.Arrays;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.ListTag;
import net.minecraft.nbt.LongArrayTag;
import net.minecraft.nbt.NbtIo;
import net.minecraft.nbt.NbtOps;
import net.minecraft.nbt.Tag;
import net.minecraft.world.level.chunk.PalettedContainerFactory;
import net.minecraft.world.level.chunk.PalettedContainerRO;
import net.minecraft.world.level.chunk.Strategy;

/**
 * The two slow steps of a chunk save, with vanilla's output byte for byte.
 *
 * <p>Palettes: {@code SerializableChunkData.write} encodes every section's block and biome container through
 * DataFixerUpper codecs, which rebuild each palette entry's tag (a block state's name and properties) on every save.
 * Here a container becomes {@code {palette, data}} directly, with each entry's tag encoded by vanilla's codec once
 * and copied after that.
 *
 * <p>NBT writes: {@code NbtIo.write} into the region file's {@code DataOutputStream} pays a locked buffered-stream
 * call for every number (a section's block data is hundreds of {@code writeLong}s). Here the tag is written into a
 * plain byte array first, by vanilla's own tag code, and handed to the stream in one call.
 *
 * <p>Off when another mod patches {@code SerializableChunkData.write} or {@code NbtIo.write}, or with
 * {@code -Dhorizon.accel.save=false}.
 */
public final class FastSave {
    static final Swaps.Swap PALETTES = new Swaps.Swap("save",
            "net/minecraft/world/level/chunk/storage/SerializableChunkData", Set.of("write"));
    static final Swaps.Swap NBT = new Swaps.Swap("nbt-write", "net/minecraft/nbt/NbtIo",
            Set.of("write", "writeUnnamedTagWithFallback", "writeUnnamedTag"));
    private static final Map<Codec<?>, Map<Object, Tag>> ENTRY_TAGS = new ConcurrentHashMap<>();
    private static final ThreadLocal<Sink> SINKS = ThreadLocal.withInitial(Sink::new);
    private static volatile Boolean palettesOn, nbtOn;

    private FastSave() {}

    private static boolean decide(Swaps.Swap swap, String what) {
        var decision = Swaps.decide(swap, MixinTargets.of(swap.targetClass()), "dev.horizonlab.accel.", Set.of(), Map.of());
        if (!Boolean.parseBoolean(System.getProperty("horizon.accel.save", "true"))) {
            decision = new Swaps.Decision(swap, List.of("horizon.accel.save=false"));
        }
        AccelMod.LOG.info("horizon-accel: {}", decision.on() ? "fast " + what : "vanilla " + what + ": "
                + String.join("; ", decision.reasons()));
        return decision.on();
    }

    private static boolean palettesOn() {
        Boolean on = palettesOn;
        if (on == null) {
            synchronized (FastSave.class) {
                if (palettesOn == null) palettesOn = decide(PALETTES, "palette encoding for chunk saves");
                on = palettesOn;
            }
        }
        return on;
    }

    private static boolean nbtOn() {
        Boolean on = nbtOn;
        if (on == null) {
            synchronized (FastSave.class) {
                if (nbtOn == null) nbtOn = decide(NBT, "NBT writes for chunk saves");
                on = nbtOn;
            }
        }
        return on;
    }

    /**
     * {@code tag.store(key, codec, value)} for a section's block or biome container, done here; false for anything
     * else, or when this is off, so the caller runs vanilla's.
     */
    public static boolean store(CompoundTag tag, String key, Codec<?> codec, Object value, PalettedContainerFactory factory) {
        return value instanceof PalettedContainerRO && palettesOn() && storeAlways(tag, key, codec, value, factory);
    }

    /** {@link #store} whether or not it is on. */
    @SuppressWarnings({"unchecked", "rawtypes"})
    public static boolean storeAlways(CompoundTag tag, String key, Codec<?> codec, Object value, PalettedContainerFactory factory) {
        if (!(value instanceof PalettedContainerRO container)) return false;
        Strategy strategy;
        if (codec == factory.blockStatesContainerCodec()) {
            strategy = factory.blockStatesStrategy();
        } else if (codec == factory.biomeContainerCodec()) {
            strategy = factory.biomeStrategy();
        } else {
            return false;
        }
        tag.put(key, encodeAlways((Codec) codec, strategy, container));
        return true;
    }

    /** What {@code codec.encodeStart(NbtOps.INSTANCE, container)} returns, whether or not this is on. */
    public static <T, C extends PalettedContainerRO<T>> Tag encodeAlways(Codec<C> codec, Strategy<T> strategy, C container) {
        Map<Object, Tag> cache = ENTRY_TAGS.computeIfAbsent(codec, c -> new ConcurrentHashMap<>());
        var packed = container.pack(strategy);
        var entries = packed.paletteEntries();
        var palette = new ListTag();
        for (int i = 0; i < entries.size(); i++) {
            Tag entry = cache.get(entries.get(i));
            if (entry == null) {
                return learn(codec, strategy, container, cache);
            }
            palette.add(entry.copy());
        }
        var out = new CompoundTag();
        out.put("palette", palette);
        packed.storage().ifPresent(data -> out.put("data", new LongArrayTag(data.toArray())));
        return out;
    }

    /** Vanilla's encoding, remembering the tag of each palette entry in it. */
    private static <T, C extends PalettedContainerRO<T>> Tag learn(Codec<C> codec, Strategy<T> strategy, C container,
            Map<Object, Tag> cache) {
        Tag tag = codec.encodeStart(NbtOps.INSTANCE, container).getOrThrow();
        var entries = container.pack(strategy).paletteEntries();
        if (tag instanceof CompoundTag compound && compound.get("palette") instanceof ListTag palette
                && palette.size() == entries.size()) {
            for (int i = 0; i < entries.size(); i++) {
                cache.putIfAbsent(entries.get(i), palette.get(i).copy());
            }
        }
        return tag;
    }

    /** {@code NbtIo.write(tag, output)} done here; false when this is off or {@code output} isn't a plain stream. */
    public static boolean write(CompoundTag tag, DataOutput output) throws IOException {
        if (!(output instanceof DataOutputStream stream) || !nbtOn()) return false;
        writeAlways(tag, stream);
        return true;
    }

    /** {@link #write} whether or not it is on. */
    public static void writeAlways(CompoundTag tag, DataOutputStream output) throws IOException {
        Sink sink = SINKS.get();
        sink.length = 0;
        try {
            NbtIo.write(tag, sink);
            output.write(sink.bytes, 0, sink.length);
        } finally {
            if (sink.bytes.length > Sink.KEEP) sink.bytes = new byte[Sink.INITIAL];
        }
    }

    /** A {@link DataOutput} into a growing byte array, with {@link DataOutputStream}'s encoding of every call. */
    static final class Sink implements DataOutput {
        static final int INITIAL = 64 << 10, KEEP = 4 << 20;
        private static final VarHandle SHORT = MethodHandles.byteArrayViewVarHandle(short[].class, ByteOrder.BIG_ENDIAN);
        private static final VarHandle INT = MethodHandles.byteArrayViewVarHandle(int[].class, ByteOrder.BIG_ENDIAN);
        private static final VarHandle LONG = MethodHandles.byteArrayViewVarHandle(long[].class, ByteOrder.BIG_ENDIAN);
        byte[] bytes = new byte[INITIAL];
        int length;

        /** Room for {@code count} more bytes, at the returned index. Callers read {@code bytes} after this. */
        private int reserve(int count) {
            int at = length;
            if (at + count > bytes.length) {
                bytes = Arrays.copyOf(bytes, Math.max(bytes.length * 2, at + count));
            }
            length = at + count;
            return at;
        }

        @Override public void write(int b) {
            int at = reserve(1);
            bytes[at] = (byte) b;
        }
        @Override public void write(byte[] b) { write(b, 0, b.length); }
        @Override public void write(byte[] b, int off, int len) {
            java.util.Objects.checkFromIndexSize(off, len, b.length);
            int at = reserve(len);
            System.arraycopy(b, off, bytes, at, len);
        }
        @Override public void writeBoolean(boolean v) { write(v ? 1 : 0); }
        @Override public void writeByte(int v) { write(v); }
        @Override public void writeShort(int v) {
            int at = reserve(2);
            SHORT.set(bytes, at, (short) v);
        }
        @Override public void writeChar(int v) { writeShort(v); }
        @Override public void writeInt(int v) {
            int at = reserve(4);
            INT.set(bytes, at, v);
        }
        @Override public void writeLong(long v) {
            int at = reserve(8);
            LONG.set(bytes, at, v);
        }
        @Override public void writeFloat(float v) { writeInt(Float.floatToIntBits(v)); }
        @Override public void writeDouble(double v) { writeLong(Double.doubleToLongBits(v)); }
        @Override public void writeBytes(String s) {
            int at = reserve(s.length());
            for (int i = 0; i < s.length(); i++) bytes[at + i] = (byte) s.charAt(i);
        }
        @Override public void writeChars(String s) {
            for (int i = 0; i < s.length(); i++) writeChar(s.charAt(i));
        }
        @Override public void writeUTF(String s) throws IOException {
            int strlen = s.length(), utflen = strlen;
            for (int i = 0; i < strlen; i++) {
                int c = s.charAt(i);
                if (c >= 0x80 || c == 0) utflen += c >= 0x800 ? 2 : 1;
            }
            if (utflen > 65535) {
                throw new UTFDataFormatException("encoded string (" + s.substring(0, 8) + "..." + s.substring(strlen - 8)
                        + ") too long: " + utflen + " bytes");
            }
            int at = reserve(2 + utflen);
            SHORT.set(bytes, at, (short) utflen);
            at += 2;
            for (int i = 0; i < strlen; i++) {
                int c = s.charAt(i);
                if (c < 0x80 && c != 0) {
                    bytes[at++] = (byte) c;
                } else if (c >= 0x800) {
                    bytes[at++] = (byte) (0xE0 | ((c >> 12) & 0x0F));
                    bytes[at++] = (byte) (0x80 | ((c >> 6) & 0x3F));
                    bytes[at++] = (byte) (0x80 | (c & 0x3F));
                } else {
                    bytes[at++] = (byte) (0xC0 | ((c >> 6) & 0x1F));
                    bytes[at++] = (byte) (0x80 | (c & 0x3F));
                }
            }
        }
    }
}
