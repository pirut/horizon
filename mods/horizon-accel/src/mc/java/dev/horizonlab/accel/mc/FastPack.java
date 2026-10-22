package dev.horizonlab.accel.mc;

import dev.horizonlab.accel.Swaps;
import java.lang.invoke.MethodHandle;
import java.lang.invoke.MethodHandles;
import java.lang.invoke.MethodType;
import java.lang.invoke.VarHandle;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import net.minecraft.util.BitStorage;
import net.minecraft.util.SimpleBitStorage;
import net.minecraft.world.level.chunk.Configuration;
import net.minecraft.world.level.chunk.Palette;
import net.minecraft.world.level.chunk.PalettedContainer;
import net.minecraft.world.level.chunk.PalettedContainerRO;
import net.minecraft.world.level.chunk.Strategy;

/**
 * {@code PalettedContainer.pack}, which every chunk save runs on every block and biome container, without its
 * per-entry palette lookups: the old palette's ids map to the compacted palette's through an array, and a
 * container holding one value packs without touching its entries. The result is vanilla's exactly: the compacted
 * palette lists values in the order they first appear, as vanilla's {@code HashMapPalette} does, and the storage
 * gets the bits vanilla's strategy picks for that palette size.
 *
 * <p>Off when another mod patches {@code pack} (Lithium does the same job) or with
 * {@code -Dhorizon.accel.serialization=false}.
 */
public final class FastPack {
    static final Swaps.Swap SERIALIZATION = new Swaps.Swap("serialization",
            "net/minecraft/world/level/chunk/PalettedContainer", Set.of("pack", "reencodeContents"));
    private static final VarHandle DATA;
    private static final MethodHandle STORAGE, PALETTE, CONFIGURATION;
    private static volatile Boolean on;

    static {
        try {
            Class<?> data = Class.forName("net.minecraft.world.level.chunk.PalettedContainer$Data");
            DATA = MethodHandles.privateLookupIn(PalettedContainer.class, MethodHandles.lookup())
                    .findVarHandle(PalettedContainer.class, "data", data);
            var lookup = MethodHandles.privateLookupIn(data, MethodHandles.lookup());
            STORAGE = lookup.findVirtual(data, "storage", MethodType.methodType(BitStorage.class))
                    .asType(MethodType.methodType(BitStorage.class, Object.class));
            PALETTE = lookup.findVirtual(data, "palette", MethodType.methodType(Palette.class))
                    .asType(MethodType.methodType(Palette.class, Object.class));
            CONFIGURATION = MethodHandles.privateLookupIn(Strategy.class, MethodHandles.lookup())
                    .findVirtual(Strategy.class, "getConfigurationForPaletteSize",
                            MethodType.methodType(Configuration.class, int.class));
        } catch (ReflectiveOperationException e) {
            throw new ExceptionInInitializerError(e);
        }
    }

    private FastPack() {}

    private static boolean on() {
        Boolean decided = on;
        if (decided == null) {
            synchronized (FastPack.class) {
                if (on == null) {
                    var decision = Swaps.decide(SERIALIZATION, MixinTargets.of(SERIALIZATION.targetClass()),
                            "dev.horizonlab.accel.", Set.of(), Map.of());
                    if (!Boolean.parseBoolean(System.getProperty("horizon.accel.serialization", "true"))) {
                        decision = new Swaps.Decision(SERIALIZATION, List.of("horizon.accel.serialization=false"));
                    }
                    AccelMod.LOG.info("horizon-accel: {}", decision.on() ? "fast palette packing for chunk saves"
                            : "vanilla palette packing: " + String.join("; ", decision.reasons()));
                    on = decision.on();
                }
                decided = on;
            }
        }
        return decided;
    }

    /** {@code container.pack(strategy)}, or null to let vanilla's run. */
    public static <T> PalettedContainerRO.PackedData<T> pack(PalettedContainer<T> container, Strategy<T> strategy) {
        return on() ? packAlways(container, strategy) : null;
    }

    /** {@link #pack} whether or not it is on (for parity checks), or null for vanilla's global-palette case. */
    public static <T> PalettedContainerRO.PackedData<T> packAlways(PalettedContainer<T> container, Strategy<T> strategy) {
        container.acquire();
        try {
            Object data = DATA.getVolatile(container);
            BitStorage storage = (BitStorage) STORAGE.invokeExact(data);
            @SuppressWarnings("unchecked")
            Palette<T> palette = (Palette<T>) PALETTE.invokeExact(data);
            int size = strategy.entryCount();
            List<T> entries = new ArrayList<>();
            if (storage.getBits() == 0) {
                entries.add(palette.valueFor(0));
                return packed(strategy, entries, size, null);
            }
            int paletteSize = palette.getSize();
            if (paletteSize > 4096) {
                // The global palette: vanilla's hash map is cheaper than an array over every registry id.
                return null;
            }
            int[] values = new int[storage.getSize()];
            storage.unpack(values);
            int[] ids = new int[paletteSize];
            Arrays.fill(ids, -1);
            for (int i = 0; i < values.length; i++) {
                int old = values[i];
                int id = ids[old];
                if (id < 0) {
                    id = entries.size();
                    ids[old] = id;
                    entries.add(palette.valueFor(old));
                }
                values[i] = id;
            }
            return packed(strategy, entries, size, values);
        } catch (RuntimeException | Error e) {
            throw e;
        } catch (Throwable t) {
            throw new IllegalStateException(t);
        } finally {
            container.release();
        }
    }

    private static <T> PalettedContainerRO.PackedData<T> packed(Strategy<T> strategy, List<T> entries, int size,
            int[] values) throws Throwable {
        int bits = ((Configuration) CONFIGURATION.invokeExact(strategy, entries.size())).bitsInStorage();
        if (bits == 0) {
            return new PalettedContainerRO.PackedData<>(entries, Optional.empty(), 0);
        }
        if (values == null) {
            values = new int[size];
        }
        return new PalettedContainerRO.PackedData<>(entries,
                Optional.of(Arrays.stream(new SimpleBitStorage(bits, size, values).getRaw())), bits);
    }
}
