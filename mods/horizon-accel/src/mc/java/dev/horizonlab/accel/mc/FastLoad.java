package dev.horizonlab.accel.mc;

import com.mojang.datafixers.util.Pair;
import com.mojang.serialization.Codec;
import com.mojang.serialization.DataResult;
import com.mojang.serialization.DynamicOps;
import dev.horizonlab.accel.Swaps;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.stream.LongStream;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.ListTag;
import net.minecraft.nbt.LongArrayTag;
import net.minecraft.nbt.NbtOps;
import net.minecraft.nbt.Tag;
import net.minecraft.world.level.chunk.PalettedContainer;
import net.minecraft.world.level.chunk.PalettedContainerFactory;
import net.minecraft.world.level.chunk.PalettedContainerRO;
import net.minecraft.world.level.chunk.Strategy;

/**
 * Chunk loads without DataFixerUpper decoding of section palettes. {@code SerializableChunkData.parse} decodes every
 * section's block and biome containers through codecs, which parse each palette entry's tag (a block state's name
 * and properties) on every load. Here the codecs it gets are wrapped: a container tag's palette entries come from a
 * cache of what vanilla's codec decoded each entry tag to, and the container is built by vanilla's own
 * {@code PalettedContainer.unpack}. Anything unusual (an entry vanilla's codec reports an error for, data that isn't
 * a long array, an unpack error) goes to vanilla's codec whole, so errors and their repairs stay vanilla's.
 *
 * <p>Off when another mod patches {@code SerializableChunkData.parse}, or with {@code -Dhorizon.accel.load=false}.
 */
public final class FastLoad {
    static final Swaps.Swap LOAD = new Swaps.Swap("load",
            "net/minecraft/world/level/chunk/storage/SerializableChunkData",
            Set.of("parse", "lambda$parse$3", "lambda$parse$5"));
    private static final Map<Codec<?>, Codec<?>> WRAPPED = new ConcurrentHashMap<>();
    private static volatile Boolean on;

    private FastLoad() {}

    private static boolean on() {
        Boolean decided = on;
        if (decided == null) {
            synchronized (FastLoad.class) {
                if (on == null) {
                    var decision = Swaps.decide(LOAD, MixinTargets.of(LOAD.targetClass()), "dev.horizonlab.accel.",
                            Set.of(), Map.of());
                    if (!Boolean.parseBoolean(System.getProperty("horizon.accel.load", "true"))) {
                        decision = new Swaps.Decision(LOAD, List.of("horizon.accel.load=false"));
                    }
                    AccelMod.LOG.info("horizon-accel: {}", decision.on() ? "fast palette decoding for chunk loads"
                            : "vanilla palette decoding for chunk loads: " + String.join("; ", decision.reasons()));
                    on = decision.on();
                }
                decided = on;
            }
        }
        return decided;
    }

    /** The block-state container codec {@code parse} decodes with: the factory's, wrapped when this is on. */
    public static Codec<PalettedContainer<net.minecraft.world.level.block.state.BlockState>> blockStates(PalettedContainerFactory factory) {
        return on() ? blockStatesAlways(factory) : factory.blockStatesContainerCodec();
    }

    /** The biome container codec {@code parse} decodes with: the factory's, wrapped when this is on. */
    public static Codec<PalettedContainerRO<net.minecraft.core.Holder<net.minecraft.world.level.biome.Biome>>> biomes(PalettedContainerFactory factory) {
        return on() ? biomesAlways(factory) : factory.biomeContainerCodec();
    }

    /** {@link #blockStates}, wrapped whether or not this is on. */
    @SuppressWarnings("unchecked")
    public static Codec<PalettedContainer<net.minecraft.world.level.block.state.BlockState>> blockStatesAlways(PalettedContainerFactory factory) {
        var codec = factory.blockStatesContainerCodec();
        return (Codec<PalettedContainer<net.minecraft.world.level.block.state.BlockState>>) WRAPPED.computeIfAbsent(codec,
                c -> new Decoding<>(codec, factory.blockStatesStrategy()));
    }

    /** {@link #biomes}, wrapped whether or not this is on. */
    @SuppressWarnings("unchecked")
    public static Codec<PalettedContainerRO<net.minecraft.core.Holder<net.minecraft.world.level.biome.Biome>>> biomesAlways(PalettedContainerFactory factory) {
        var codec = factory.biomeContainerCodec();
        return (Codec<PalettedContainerRO<net.minecraft.core.Holder<net.minecraft.world.level.biome.Biome>>>) WRAPPED.computeIfAbsent(codec,
                c -> new Decoding<>(codec, factory.biomeStrategy()));
    }

    /** A container codec that decodes NBT container tags itself and leaves everything else to {@code vanilla}. */
    private record Decoding<T, C extends PalettedContainerRO<T>>(Codec<C> vanilla, Strategy<T> strategy,
            Map<Tag, T> entries) implements Codec<C> {
        Decoding(Codec<C> vanilla, Strategy<T> strategy) {
            this(vanilla, strategy, new ConcurrentHashMap<>());
        }

        @Override
        public <O> DataResult<O> encode(C input, DynamicOps<O> ops, O prefix) {
            return vanilla.encode(input, ops, prefix);
        }

        @Override
        @SuppressWarnings("unchecked")
        public <O> DataResult<Pair<C, O>> decode(DynamicOps<O> ops, O input) {
            if (ops == NbtOps.INSTANCE && input instanceof CompoundTag tag) {
                C container = fast(tag);
                if (container != null) {
                    return DataResult.success(Pair.of(container, input));
                }
            }
            return vanilla.decode(ops, input);
        }

        /** The container, or null for vanilla's codec to decode. */
        @SuppressWarnings("unchecked")
        private C fast(CompoundTag tag) {
            if (!(tag.get("palette") instanceof ListTag palette) || palette.isEmpty()) return null;
            Optional<LongStream> storage;
            Tag data = tag.get("data");
            if (data == null) {
                storage = Optional.empty();
            } else if (data instanceof LongArrayTag longs) {
                storage = Optional.of(LongStream.of(longs.getAsLongArray()));
            } else {
                return null;
            }
            List<T> values = new ArrayList<>(palette.size());
            for (int i = 0; i < palette.size(); i++) {
                Tag entry = palette.get(i);
                T value = entries.get(entry);
                if (value == null && (value = learn(entry)) == null) return null;
                values.add(value);
            }
            DataResult<PalettedContainer<T>> unpacked = PalettedContainer.unpack(strategy, new PalettedContainerRO.PackedData<>(values, storage));
            return unpacked.isSuccess() ? (C) unpacked.getOrThrow() : null;
        }

        /** What vanilla's codec decodes this palette entry to, or null when it reports an error for it. */
        private T learn(Tag entry) {
            var one = new ListTag();
            one.add(entry.copy());
            var tag = new CompoundTag();
            tag.put("palette", one);
            var result = vanilla.parse(NbtOps.INSTANCE, tag);
            if (!result.isSuccess()) return null;
            T value = result.getOrThrow().get(0, 0, 0);
            entries.putIfAbsent(entry.copy(), value);
            return value;
        }
    }
}
