package dev.horizonlab.accel;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * Which swaps run: a swap runs only when no other mod patches the class it replaces and its inputs are ones
 * Horizon handles. Otherwise vanilla's code (with every other mod's patches) runs, and the reason is logged.
 */
public final class Swaps {
    /** A swappable subsystem: the class whose method horizon-accel replaces. */
    public record Swap(String name, String targetClass) {}

    /** The decision for one swap, with the reasons it is off. */
    public record Decision(Swap swap, List<String> reasons) {
        public boolean on() {
            return reasons.isEmpty();
        }
    }

    private Swaps() {}

    /**
     * {@code appliedMixins} maps each target class to the mixin classes applied to it (from Mixin's
     * {@code ClassInfo}); horizon-accel's own mixins live under {@code ownPackage} and don't count.
     * {@code unsupported} maps a swap's name to input problems found at startup (an unmapped id, a modded
     * chunk generator).
     */
    public static Decision decide(Swap swap, Map<String, Set<String>> appliedMixins, String ownPackage,
            Map<String, List<String>> unsupported) {
        List<String> reasons = new ArrayList<>();
        for (String mixin : appliedMixins.getOrDefault(swap.targetClass(), Set.of())) {
            if (!mixin.startsWith(ownPackage)) {
                reasons.add("patched by " + mixin);
            }
        }
        reasons.addAll(unsupported.getOrDefault(swap.name(), List.of()));
        return new Decision(swap, List.copyOf(reasons));
    }
}
