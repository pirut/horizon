package dev.horizonlab.accel;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.TreeSet;

/**
 * Which swaps run: a swap runs only when no other mod patches a method it replaces and its inputs are ones Horizon
 * handles. Otherwise vanilla's code (with every other mod's patches) runs, and the reason is logged.
 *
 * <p>A mixin on the same class that leaves the replaced methods alone doesn't matter, and neither does one known to
 * change only how the work is done, never its result (a performance mod's pooling, or which executor runs it):
 * {@code sameResults} names those, so horizon-accel runs alongside C2ME.
 */
public final class Swaps {
    /** A swappable subsystem: the class whose methods horizon-accel replaces, and those methods. */
    public record Swap(String name, String targetClass, Set<String> methods) {}

    /** The decision for one swap, with the reasons it is off. */
    public record Decision(Swap swap, List<String> reasons) {
        public boolean on() {
            return reasons.isEmpty();
        }
    }

    private Swaps() {}

    /**
     * {@code mixins} maps each mixin applied to the swap's class (from Mixin's {@code ClassInfo}) to the methods
     * of that class it injects into, redirects in or overwrites ({@code "*"}: all of them); horizon-accel's own mixins live under
     * {@code ownPackage} and don't count. {@code unsupported} maps a swap's name to input problems found at startup
     * (an unmapped id, a modded chunk generator).
     */
    public static Decision decide(Swap swap, Map<String, Set<String>> mixins, String ownPackage,
            Set<String> sameResults, Map<String, List<String>> unsupported) {
        List<String> reasons = new ArrayList<>();
        new java.util.TreeMap<>(mixins).forEach((mixin, methods) -> {
            if (mixin.startsWith(ownPackage) || sameResults.contains(mixin)) {
                return;
            }
            Set<String> touched = new TreeSet<>(methods.contains("*") ? swap.methods() : methods);
            touched.retainAll(swap.methods());
            if (!touched.isEmpty()) {
                reasons.add("patched by " + mixin + " (" + String.join(", ", touched) + ")");
            }
        });
        reasons.addAll(unsupported.getOrDefault(swap.name(), List.of()));
        return new Decision(swap, List.copyOf(reasons));
    }
}
