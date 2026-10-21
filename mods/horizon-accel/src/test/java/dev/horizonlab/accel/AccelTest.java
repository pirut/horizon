package dev.horizonlab.accel;

import java.util.List;
import java.util.Map;
import java.util.Set;

/** {@code java -ea -cp <classes> dev.horizonlab.accel.AccelTest} */
public final class AccelTest {
    public static void main(String[] args) {
        IdMap map = IdMap.of(new String[] {"minecraft:stone", "minecraft:gone", "minecraft:air"},
                name -> switch (name) {
                    case "minecraft:air" -> 0;
                    case "minecraft:stone" -> 1;
                    default -> -1;
                });
        check(map.ids()[0] == 1 && map.ids()[2] == 0, "ids by name");
        check(!map.complete() && map.missing().equals(List.of("minecraft:gone")), "missing names");

        Swaps.Swap terrain = new Swaps.Swap("terrain", "net/minecraft/world/level/levelgen/NoiseBasedChunkGenerator");
        Map<String, Set<String>> applied = Map.of(terrain.targetClass(),
                Set.of("dev.horizonlab.accel.mixin.TerrainMixin"));
        check(Swaps.decide(terrain, applied, "dev.horizonlab.accel.", Map.of()).on(), "own mixins don't count");
        applied = Map.of(terrain.targetClass(),
                Set.of("dev.horizonlab.accel.mixin.TerrainMixin", "com.example.FasterNoiseMixin"));
        Swaps.Decision off = Swaps.decide(terrain, applied, "dev.horizonlab.accel.",
                Map.of("terrain", List.of("1 block state unmapped")));
        check(!off.on() && off.reasons().equals(List.of("patched by com.example.FasterNoiseMixin",
                "1 block state unmapped")), "another mod's mixin turns the swap off");
        System.out.println("AccelTest: ok");
    }

    static void check(boolean ok, String what) {
        if (!ok) {
            throw new AssertionError(what);
        }
    }
}
