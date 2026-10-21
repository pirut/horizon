# Java mods at Horizon's speed: Horizon's engine inside Java Minecraft

The goal: **every Java mod works, and a modded game runs faster than the same mods on standard Fabric.**

[java-mod-compat.md](java-mod-compat.md) explains why Fabric and NeoForge jars can't run on Horizon as they
are. Most mods work by patching vanilla's own Java code with mixins, and Horizon has none of that code.
Rebuilding vanilla's API in Java on top of Horizon gets one mod working at a time, and never all of them.

So this plan turns the problem around. It **runs the player's own Minecraft with Fabric, exactly as today**,
so every mod loads and patches the classes it expects. One more mod, `horizon-accel`, swaps vanilla's slowest
self-contained parts for Horizon's Rust code, which Horizon's parity harnesses already check against vanilla.
This is the same idea as Sodium, Lithium, C2ME and ScalableLux, but with a whole engine behind it instead of
one subsystem's rewrite.

## What decides the speed

A modded game's time goes to three places:

1. **Vanilla's own work**: world generation, lighting, chunk meshing, saving, ticking. This is where
   `horizon-accel` can win, one subsystem at a time.
2. **The mods' own code.** It runs on the same JVM as before, at the same speed. Nothing changes here.
3. **The crossing** between Java and Rust. Each swapped subsystem crosses once per unit of work (a chunk, a
   section, a batch of light updates), never once per block. With Java's FFM API (final since Java 22), a
   downcall costs a few nanoseconds, and data moves through shared memory segments without copies.

The result can only beat modded Fabric where Rust does a subsystem's work faster than HotSpot does, and Horizon
mirrors vanilla's algorithms closely, so that gap has to be **measured per subsystem before anything is
built**. Horizon's numbers so far compare Horizon with older Horizon, not with Java.

## Rules for each swapped subsystem

- **Same results as vanilla.** A swap is only allowed for code Horizon's parity harnesses already compare
  with vanilla 26.3. A world generated or lit with the mod is the same, block for block, as one without it.
- **Mods win.** At startup the mod reads which mixins target each swapped method (Mixin's own class info). If
  any other mod patches it, or if the inputs aren't ones Horizon handles (a modded chunk generator, a density
  function type Horizon doesn't have), that subsystem stays on vanilla's Java code and the log says why.
  Compatibility never gets traded for speed.
- **Registries by name.** Mods add blocks and biomes, so the numeric ids differ from vanilla's. The mod builds a
  name-keyed table from the live registries at startup, and Rust works in those ids.
- **Live data, not vanilla's files.** World generation reads the server's loaded worldgen registries, data
  packs included, and passes their JSON to Horizon's density-function compiler. A worldgen data pack such as
  Terralith gets the speedup too, wherever Horizon supports its parts.
- **Measured against modded Fabric with the usual performance mods** (Lithium, C2ME, ScalableLux, Sodium). A
  swap that is not faster than the best Java mod for the same subsystem doesn't ship.

## Candidates, in order

| Subsystem | Vanilla entry point (26.3 names) | Horizon code | Crossing | Notes |
|---|---|---|---|---|
| Terrain noise and surface | `NoiseBasedChunkGenerator.fillFromNoise`, `buildSurface` | `horizon-world` noise, aquifer and surface (parity-checked) | one call per chunk: in, the seed and chunk position; out, section palettes written into the chunk | Pure function of the seed, position and settings. Fewest moving parts. C2ME is the Java baseline |
| Carvers, features | `applyCarvers`, `applyBiomeDecoration` | `horizon-world` carving and feature placement | per chunk, needs the neighbouring chunks | Features touch block entities and mod features. Vanilla features only |
| Lighting | `LevelLightEngine` | `light.rs` (vanilla's rules) | per section, light arrays in shared memory | ScalableLux is the Java baseline. Many mods read light, few patch it |
| Chunk saving | `RegionFileStorage`, NBT writing | `horizon-world` saves and compression | per chunk | Simple, and helps servers |
| Section meshing | Sodium's mesher, or vanilla's `SectionCompiler` | `horizon-client` meshing | per section | Collides with Sodium and Iris. Last, if ever |

Ticking (entities, redstone, block entities) stays in Java. Mods hook it everywhere, and its state is the
game's state.

## How it fits together

```
 Minecraft 26.3 (the player's own) + Fabric Loader + every mod, unchanged
   └─ horizon-accel (Fabric mod, Java 25)
        ├─ at startup: load libhorizon_accel (Rust cdylib), read the registries by name,
        │              check each swapped method for other mods' mixins
        ├─ mixins: one per swapped entry point; on a supported input they call Rust, otherwise vanilla
        └─ FFM downcalls; chunk data in shared MemorySegments
 libhorizon_accel (Rust, crates/horizon-accel): a C ABI over horizon-world's generators, lighting and saves
```

The Java side is a small Fabric mod in Horizon's own `dev/horizonlab` namespace, built like the dev-only mod in
`mods/horizon` (`tools/build_mod.py`). It holds no game code. The native library builds with the rest of the
workspace for each platform.

## Finding conflicts: `horizon-port-pack reach`

```sh
cargo run -p horizon-port-pack -- reach ~/my-instance/mods ~/.horizon/reach
```

For every jar (and the jars nested in it), this lists the Minecraft classes and members its code names, the
Fabric API modules it uses, and each mixin with its targets, injector kinds, methods and `@At` points. Summed
over a folder, `reach.md` ranks every mixin target and injected method by how many mods patch it. Run over
popular packs, it shows which subsystems mods leave alone (worth swapping) and which they patch (keep in Java).
It reads names only (the constant pool, the class header and annotations), and decodes no method body.

## Benchmarks, before and after every step

1. **Subsystem, Rust against Java.** The same work through both: for terrain, the same seed and 256 chunks
   through vanilla's `fillFromNoise` in a running server (with and without C2ME) and through Horizon's
   `chunkgen_bench`. This is the go or no-go for each candidate.
2. **Whole game, modded.** `tools/server_bench.py` and `tools/perf_bench.py` already run Fabric with mods
   side by side with Horizon. They get a `fabric+accel` server: the same pack with and without `horizon-accel`,
   for MSPT, P99, chunk generation rate (Chunky pregeneration), memory and start time.

Both need the player's own copy of Minecraft 26.3, which Horizon never downloads, so they run on a machine
that has it.

## What exists, and what it costs

`crates/horizon-accel` puts Horizon's terrain generation behind a C ABI (a `cdylib`): open a dimension's
generator over prepared resources, list its block states and biomes by name, take the game's ids for them,
then generate chunks from any number of threads and copy out their sections, quart biomes and post-processing
marks. Sections come out packed as vanilla's `PalettedContainer` stores them (a palette of the game's ids and
`SimpleBitStorage` words), so the Java side can hand them to the chunk as they are instead of setting 98,304
blocks one at a time. No panic crosses the boundary. `mods/horizon-accel` holds the Java side, which so far needs nothing
from Minecraft: `HorizonAccel` (the FFM binding), `IdMap` (Horizon names to the game's ids) and `Swaps` (a
swap is off when another mod's mixin is applied to its class, or when its inputs aren't supported). Its tests
run with `python3 -m unittest tests.test_accel_java` when a Java 25 JDK is installed.

The cost of crossing, from `CrossingBench` (`mods/horizon-accel/src/test`) on a 4-thread 2.8 GHz Xeon cloud
VM, OpenJDK 25.0.2, release build, median of seven rounds:

| | ns per call |
|---|---:|
| Java static call (baseline) | 0.3 |
| FFM downcall, no-op | 11.9 |
| FFM downcall, no-op, critical | 6.3 |
| Rust reading a 4096-int section from a Java `int[]` (critical, no copy) | 818 |
| Rust reading the same from a native segment | 800 |

Copying out one chunk takes about 2 crossings per section (its blocks and its post-processing marks), about 60 for a 384-block-tall chunk: **under
1 µs**, against Horizon's measured 10 ms per chunk of terrain work on one thread (`noise-aquifer-benchmark.md`).
So the crossing is not what decides this. Whether Rust's terrain generation beats HotSpot's is.

## Steps

1. Scanner and this plan.
2. **Done:** the native library, the FFM binding, the id mapping and the conflict rule, with the crossing
   measured.
3. Measure terrain generation in Rust against Java (benchmark 1). This needs Minecraft 26.3: the cloud
   machine this was built on can't reach Mojang's servers. Stop here if Rust isn't clearly faster.
4. The Minecraft-facing part of the mod, which needs the game jar to compile: a mixin on
   `NoiseBasedChunkGenerator` that, for vanilla noise settings, asks the library for the chunk and writes its
   blocks, biomes, heightmaps and post-processing marks into the `ProtoChunk`; registry mapping through
   `BlockStateParser` and the biome registry; and `Swaps` fed from Mixin's `ClassInfo`.
5. A parity harness comparing a world generated with the mod with one generated without it, chunk for chunk,
   plus the whole-game benchmark.
6. The next subsystem from the table, by measured gap.
