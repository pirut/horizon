package dev.horizonlab.accel;

import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.nio.file.Path;

/**
 * What one crossing from Java into horizon-accel costs, and reading memory across it.
 *
 * <pre>java --enable-native-access=ALL-UNNAMED -cp &lt;classes&gt; dev.horizonlab.accel.CrossingBench &lt;libhorizon_accel.so&gt; [resources]</pre>
 *
 * With a prepared resources folder it also times whole chunks through the library.
 */
public final class CrossingBench {
    static long sink;

    static long javaNoop(long x) {
        return x;
    }

    interface Body {
        void run(int n) throws Throwable;
    }

    /** Median ns per operation over seven rounds of {@code n}, after three warm-up rounds. */
    static double time(String name, int n, Body body) throws Throwable {
        double[] rounds = new double[7];
        for (int r = -3; r < rounds.length; r++) {
            long start = System.nanoTime();
            body.run(n);
            long elapsed = System.nanoTime() - start;
            if (r >= 0) {
                rounds[r] = (double) elapsed / n;
            }
        }
        java.util.Arrays.sort(rounds);
        System.out.printf("%-44s %10.2f ns%n", name, rounds[3]);
        return rounds[3];
    }

    public static void main(String[] args) throws Throwable {
        HorizonAccel accel = HorizonAccel.load(Path.of(args[0]));
        int calls = 20_000_000;
        time("Java static call (baseline)", calls, n -> {
            long s = 0;
            for (int i = 0; i < n; i++) s += javaNoop(i);
            sink += s;
        });
        time("FFM downcall, noop", calls, n -> {
            long s = 0;
            for (int i = 0; i < n; i++) s += (long) accel.noop.invokeExact((long) i);
            sink += s;
        });
        time("FFM downcall, noop, critical", calls, n -> {
            long s = 0;
            for (int i = 0; i < n; i++) s += (long) accel.noopCritical.invokeExact((long) i);
            sink += s;
        });
        int[] section = new int[4096];
        for (int i = 0; i < section.length; i++) section[i] = i;
        MemorySegment heap = MemorySegment.ofArray(section);
        time("Java sum of a 4096-int section", 200_000, n -> {
            long s = 0;
            for (int i = 0; i < n; i++) {
                for (int v : section) s += v;
            }
            sink += s;
        });
        time("Rust sum of a 4096-int heap array, critical", 200_000, n -> {
            long s = 0;
            for (int i = 0; i < n; i++) s += (long) accel.sum.invokeExact(heap, (long) section.length);
            sink += s;
        });
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment native_ = arena.allocate(ValueLayout.JAVA_INT, section.length);
            native_.copyFrom(heap);
            time("Rust sum of a 4096-int native segment", 200_000, n -> {
                long s = 0;
                for (int i = 0; i < n; i++) s += (long) accel.sum.invokeExact(native_, (long) section.length);
                sink += s;
            });
        }
        if (args.length > 1) {
            chunks(accel, Path.of(args[1]));
        }
        System.out.println("(sink " + (sink & 1) + ")");
    }

    /** Terrain chunks through the library on one thread: generation plus the copy into Java arrays. */
    static void chunks(HorizonAccel accel, Path resources) {
        long start = System.nanoTime();
        try (HorizonAccel.Generator generator = accel.open(resources, 12345, "minecraft:overworld", true)) {
            System.out.printf("open: %.1f ms, %d states, %d biomes%n", (System.nanoTime() - start) / 1e6,
                    generator.stateNames().length, generator.biomeNames().length);
            int size = 16;
            for (int round = 0; round < 2; round++) {
                long t = System.nanoTime();
                long blocks = 0;
                for (int x = 0; x < size; x++) {
                    for (int z = 0; z < size; z++) {
                        blocks += generator.generate(x + round * 64, z).blocks().length;
                    }
                }
                System.out.printf("%s: %.3f ms per chunk (%d blocks)%n", round == 0 ? "warm-up" : "measured",
                        (System.nanoTime() - t) / 1e6 / (size * size), blocks);
            }
        }
    }
}
