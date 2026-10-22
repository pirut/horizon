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

        Swaps.Swap terrain = new Swaps.Swap("terrain", "net/minecraft/world/level/levelgen/NoiseBasedChunkGenerator",
                Set.of("buildTerrain", "doFill", "buildSurface"));
        Map<String, Set<String>> applied = Map.of("dev.horizonlab.accel.mixin.TerrainMixin", Set.of("buildTerrain"),
                "com.example.SpawnMixin", Set.of("spawnOriginalMobs"));
        check(Swaps.decide(terrain, applied, "dev.horizonlab.accel.", Set.of(), Map.of()).on(),
                "own mixins, and mixins on other methods, don't count");
        applied = Map.of("dev.horizonlab.accel.mixin.TerrainMixin", Set.of("buildTerrain"),
                "com.example.FasterNoiseMixin", Set.of("doFill", "getBaseHeight"),
                "com.example.PoolingMixin", Set.of("buildSurface"));
        Swaps.Decision off = Swaps.decide(terrain, applied, "dev.horizonlab.accel.", Set.of("com.example.PoolingMixin"),
                Map.of("terrain", List.of("1 block state unmapped")));
        check(!off.on() && off.reasons().equals(List.of("patched by com.example.FasterNoiseMixin (doFill)",
                "1 block state unmapped")), "another mod's mixin on a replaced method turns the swap off");
        for (int bits : new int[] {4, 5, 11, 12}) {
            int perWord = 64 / bits;
            int[] palette = new int[1 << bits];
            for (int i = 0; i < palette.length; i++) palette[i] = 1000 + i;
            long[] words = new long[(4096 + perWord - 1) / perWord];
            for (int i = 0; i < 4096; i++) {
                words[i / perWord] |= (long) (i % palette.length) << (i % perWord * bits);
            }
            int[] out = new int[4097];
            new HorizonAccel.Section(palette, words, bits).unpack(out, 1);
            for (int i = 0; i < 4096; i++) {
                check(out[i + 1] == 1000 + i % palette.length, "unpacks " + bits + "-bit sections");
            }
        }
        int[] uniform = new int[4096];
        new HorizonAccel.Section(new int[] {42}, new long[0], 0).unpack(uniform, 0);
        check(uniform[0] == 42 && uniform[4095] == 42, "a section of one state");
        System.out.println("AccelTest: ok");
    }

    static void check(boolean ok, String what) {
        if (!ok) {
            throw new AssertionError(what);
        }
    }
}
