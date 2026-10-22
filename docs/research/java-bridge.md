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

1. **Subsystem, Rust against Java**: `tools/accel_bench.py`. `VanillaTerrainBench`
   (`mods/horizon-accel/src/bench`) runs vanilla 26.3's own statuses in-process on the pinned server jar, with
   real `WorldGenRegion`s (the scaffolding of `VanillaChunkGenerationOracle`); `terrain_bench`
   (`crates/horizon-accel/examples`) runs Horizon's through `Accel`, the code the swap calls. Both bring fresh
   16×16 squares of chunks to the carvers status after warmup rounds, and the script alternates them. With
   `HORIZON_ACCEL_MODE=parity` the Java bench also fills every chunk again through the swap and compares it
   with vanilla's, block for block, with post-processing marks and world-generation heightmaps.
2. **Whole server**: `tools/accel_pregen.py`. A fresh Fabric 26.3 server per mod set (Fabric API and Chunky
   always; Lithium, C2ME, horizon-accel as asked), the same seed, and Chunky generating the same square of
   chunks through every status. `--jfr` records each run. `tools/accel_world_diff.py` compares two saved
   worlds block for block.

Both need the player's own copy of Minecraft 26.3, which Horizon never downloads (`horizon-prepare --prepare`).

## Results

Measured on 2026-10-22 on a 4-vCPU cloud VM (2.8 GHz Xeon, Temurin 25.0.4, seed 12345). Run-to-run noise is
about ±8 %, so every comparison alternates its sides and repeats.

**Terrain alone** (noise, surface, carvers; `accel_bench.py`, two pairs of 5 rounds × 256 chunks):

| | vanilla Java | Horizon Rust | |
|---|---:|---:|---:|
| 1 thread | 20.1 ms/chunk | 13.8 ms/chunk | 1.45× |
| 4 threads | 7.2 ms/chunk | 3.6 ms/chunk | 2.0× |
| up to carvers, incl. structure starts and biomes, 4 threads | 7.7 | 4.4 | 1.76× |

Through the swap, inside vanilla's pipeline (Rust, the copy out, and writing the chunk), terrain takes 16.0
ms/chunk on 1 thread and 4.9 on 4: writing a chunk costs about 0.45 ms. The parity mode found 768 chunks on
each of three seeds (12345, -1, 987654321987) identical to vanilla's.

**Whole server** (`accel_pregen.py`, 4,225 chunks, Chunky, chunks per second, mean of the repeats):

| mods | chunks/s | vs vanilla |
|---|---:|---:|
| vanilla (Fabric API, Chunky) | 52.3 | |
| Lithium | 56.3 | +8 % |
| horizon-accel | 55.9 | +7 % |
| C2ME | 70.3 (3 later runs: 68.8) | +34 % |
| C2ME + horizon-accel | 72.5 | +39 % |

C2ME + horizon-accel beat C2ME alone in each of three alternating pairs (74.3/70.8/72.3 against
70.3/68.8/67.4). JFR on vanilla shows where a chunk's time goes: terrain about half (noise 29 %, surface
21 %, carvers 3 %), features 22 %, light 5 %, saving 5 %, biomes 2 %. Inside a server Horizon's terrain takes
18 to 20 ms per chunk per thread (noise 9.3, surface 5.5, structures 1.8, biomes 1.1, carvers 1.0; the mod
logs this every 4,096 chunks), against 14 ms in isolation, because it shares the cores with the server's
other work; with C2ME's larger worker pool, 31 ms.

Saved worlds can't show parity directly: vanilla's own features (ore blobs, seagrass) land differently from
run to run, so two vanilla worlds share only 454 of 1,089 chunks exactly, and vanilla against horizon-accel
424. The block-for-block check is the in-process parity mode above.

**After the Perlin and climate-search speedups** (pirut/horizon#4, same results): Horizon's terrain is 12.4
ms/chunk on 1 thread (was 14.5) and its biomes 1.06 (was 1.20). In a later pregen: vanilla 48.1 chunks/s,
horizon-accel 54.7 (+14 %), C2ME + horizon-accel 77.0. Without C2ME the CPU is only 45 to 90 % busy during
pregen, so vanilla's chunk scheduling, not terrain, caps the server; faster Rust terrain mostly shows under
C2ME.

**Where C2ME + horizon-accel spends its worker CPU** (JFR, 9,100 samples): features 32 % (sculk patches 12 %,
ores 8 %, trees 2 %), Horizon's terrain over FFM 15 %, C2ME's scheduler 13 %, saving 15 %, light 6 %, Java
biomes 3 %. Features are now the largest part.

**Features, Java against Rust** (one thread, the same 8×8 squares; `VanillaTerrainBench` with
`HORIZON_BENCH_FEATURES=1`, `decorate_bench`): vanilla 4.1 and 5.8 ms/chunk, Horizon 2.3 and 2.2 (2× to
2.6×). Horizon decorates its own chunks with every feature supported. A features swap would have to copy the
3×3 region into Rust (about 1 ms) and replay Rust's writes through Java's `setBlock` so block entities, ticks
and post-processing stay exact.

**The features swap** (`FeatureSwap`, a mixin on `ChunkGenerator.applyBiomeDecoration`, on by default; turn
it off with `-Dhorizon.accel.features=false`). Java sends the 3×3 region to Rust, Rust decorates it and
returns a log of operations as 7-int records (set a block in the region or straight into a section, schedule
a block or fluid tick, mark post-processing, set a spawner's entity or a container's loot table, load a block
entity or add an entity from NBT, read a heightmap so vanilla's lazy priming happens at the same point), and
Java replays them through vanilla's own calls. Anything Rust can't express falls back to vanilla's
decoration for that chunk. To skip most of the copy, Rust keeps the chunks it generated in terrain (keyed by a
token Java holds in a weak map, dropped after the 9 decorations that read a chunk or by LRU at 2,048), so a
region usually costs only 9 tokens.

Parity: `HORIZON_ACCEL_MODE=fparity` compares blocks, block-entity NBT, entities (type and position; UUIDs
are random), block and fluid ticks, post-processing marks and every heightmap against vanilla. Seven seeds
(12345, 1, 42, 999, 7, 2024, -4172144997902289642), about 9,400 chunks: all identical, none fell back.

Speed: in one thread the swap about ties vanilla (4.6 against 4.5 ms/chunk: Rust 3.2, the replay 0.95,
the rest 0.25), because replaying through `WorldGenRegion.setBlock` costs nearly a third. In a server with
C2ME it wins: pregen of 4,225 chunks at 89.2 and 89.9 chunks/s with features on against 86.1 and 84.9 off
(+4 to 5 %), alternating. Engine speedups that came with it: a grid index for `TerrainRegion`, a heightmap
fast path and a bitset in ore placement, and a `BiomeTemperature` cache that no longer pre-zeroes 38M
instructions' worth of memory.

## What exists, and what it costs

`crates/horizon-accel` puts Horizon's terrain generation behind a C ABI (a `cdylib`): open a dimension's
generator over prepared resources, list its block states and biomes by name, take the game's ids for them,
then generate chunks from any number of threads and copy out their sections, quart biomes and post-processing
marks. Sections come out packed as vanilla's `PalettedContainer` stores them (a palette of the game's ids and
`SimpleBitStorage` words), so the Java side can hand them to the chunk as they are instead of setting 98,304
blocks one at a time. No panic crosses the boundary. `mods/horizon-accel` holds the Java side, which so far needs nothing
from Minecraft: `HorizonAccel` (the FFM binding), `IdMap` (Horizon names to the game's ids) and `Swaps` (a
swap is off when another mod's mixin changes a method it replaces, unless that mixin is known to change only
how the work runs, or when its inputs aren't supported). Its tests run with
`python3 -m unittest tests.test_accel_java` when a Java 25 JDK is installed.

`mods/horizon-accel/src/mc` is the Minecraft-facing part, built with `tools/build_accel_mod.py` into
`dist/horizon-accel-<version>.jar`:

- `NoiseBasedChunkGeneratorMixin` replaces `buildTerrain` at its head when `AccelMod` allows it, and runs the
  swap on the same background executor vanilla uses.
- `AccelMod` loads the library (`-Dhorizon.accel.library`, `-Dhorizon.accel.resources`) and decides once per
  dimension: vanilla's generator settings and biome source for that dimension, no blending with old chunks,
  no other mod's mixin on the replaced methods (`MixinTargets` reads each applied mixin's injector targets
  from its class file; C2ME's buffer pooling and executor choice are listed as same-results), and vanilla's
  world-generation data (`WorldgenFingerprint`: a hash of every registry entry the terrain reads, as the game's
  codecs encode it, keys sorted). A data pack such as Terralith changes the hash and keeps vanilla's code.
- `TerrainSwap` writes each section with `PalettedContainer.unpack`, the way chunk loading builds one, adds
  the post-processing marks, and primes the two world-generation heightmaps from the final blocks.

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
3. **Done:** terrain in Rust against Java: 1.45× to 2× faster (Results).
4. **Done:** the Minecraft-facing mod: mixin, per-dimension decision, data fingerprint, per-method conflicts.
5. **Done:** the parity harness (in-process, block for block) and the whole-server benchmark.
6. Make Horizon's noise and surface faster, which helps both Horizon and this mod; then swap biomes too (the
   Rust call already computes them, so Java's BIOMES status is duplicated work), then features.
