package dev.horizonlab.accel;

import java.util.ArrayList;
import java.util.List;
import java.util.function.ToIntFunction;

/**
 * Horizon's block states and biomes mapped to the running game's registry ids, by name. Mods add blocks and
 * biomes, so the game's ids differ from vanilla's, and the native side has to write the game's ids.
 *
 * <p>A Horizon name the game doesn't know means the game's registries aren't the ones Horizon generates for
 * (a mod replaced a vanilla block, say), so the swap stays off: {@link #missing} lists them.
 */
public record IdMap(int[] ids, List<String> missing) {
    /** {@code resolve} returns the game's id for a name, or a negative number when it has none. */
    public static IdMap of(String[] names, ToIntFunction<String> resolve) {
        int[] ids = new int[names.length];
        List<String> missing = new ArrayList<>();
        for (int i = 0; i < names.length; i++) {
            ids[i] = resolve.applyAsInt(names[i]);
            if (ids[i] < 0) {
                missing.add(names[i]);
            }
        }
        return new IdMap(ids, List.copyOf(missing));
    }

    public boolean complete() {
        return missing.isEmpty();
    }
}
