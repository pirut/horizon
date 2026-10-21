# Running Fabric mods on Horizon: the Java bridge

This is the plan for running Java mod jars in Horizon. It reverses one conclusion of
[java-mod-compat.md](java-mod-compat.md): that page explains why jars cannot run *as they are*, and it is
still right about that. This page is the version that runs them anyway, by giving them a Minecraft to talk
to that is backed by Horizon.

**The requirement.** A pack of Java mods on Horizon must run **faster than the same mods on Fabric**, with
less memory, measured on the same machine, world and load. If a design cannot meet that, it is the wrong
design. Every choice below is judged by it.

## Where speed comes from, and where it cannot

A modded game's time splits into three parts:

1. **The game itself**: ticking, world generation, lighting, meshing, drawing. In a big pack this is most of
   the time. Horizon runs it in Rust, so this is where the gain comes from.
2. **The mods' own code**: their logic, in Java. Horizon runs it on the same HotSpot JIT as Fabric does, so it
   is **as fast, never faster**. A mod that spends 5 ms a tick in its own code spends 5 ms on both.
3. **The bridge**: every call a mod makes into the game, and every event the game sends to a mod, crosses from
   Java to Rust or back. Fabric doesn't pay this. Horizon does, so it has to stay small.

So modded Horizon beats modded Fabric when **(1) saves more than (3) costs**. Two consequences:

- **Performance mods are dropped, not bridged.** Sodium, Lithium, FerriteCore, ModernFix, ImmediatelyFast,
  EntityCulling and C2ME patch costs that Horizon doesn't have ([java-perf-mods.md](../performance/java-perf-mods.md)).
  The loader skips them and says so. They are often a third of a pack's mixins.
- **Part (1) has to be faster first.** Today Horizon's server beats vanilla on average tick time but not Paper,
  and Fabric + Lithium is ahead with 1 and 10 players
  ([four-way benchmark](../../benchmarks/server-four-way-2026-10-19/README.md)). Modded Fabric runs with
  Lithium, so the engine work on average tick time matters for this goal as much as the bridge does.

## Costs, and how each is kept down

| Cost | Size | How the bridge keeps it down |
|---|---|---|
| The JVM is loaded | about 1 s at start, 100 to 300 MB of heap before mods | Loaded **only when a Java mod is on**. Vanilla Horizon stays the same. Fabric pays this too, plus vanilla's own heap (3.4 to 4.5 GB in the server benchmark), so the comparison is already in Horizon's favour |
| Shipping a JVM | 40 to 150 MB | None shipped. Horizon already needs a Java 25 JDK on the player's machine for the one-time preparation, so the bridge uses that JDK |
| A call across the bridge | JNI: tens to hundreds of ns. Java's FFM API (Panama, final in 22): a few ns for a plain downcall | **FFM, not JNI**, for every call from Java to Rust. JNI only to start the JVM |
| Reading game state (the player, block states, entities) | one crossing per getter | **Shared memory**, not calls. Horizon puts read-only views (the player, loaded sections' block ids, entity positions) in memory segments that the facade classes read in place. `level.getBlockState(pos)` in a mod reads Horizon's section array with no crossing |
| Events (tick, HUD, block break, entity tick) | one upcall per listener per event | **One upcall per event type per tick**, with a batch: "these 40 entities ticked", not 40 calls. Events nobody listens to are never sent. Same rule as the WASM mods: a hook nobody asked for costs one branch |
| Mixins into hot loops | an upcall per block, entity or vertex | Only **mapped injection points** run (below), and each is marked hot or cold. A mixin on a per-block or per-vertex path is refused or rewritten into a batched hook. A mixin into code Horizon doesn't run (performance mods, renderer internals) is skipped and reported |
| Garbage collection | pauses in the mod's own code | ZGC (sub-millisecond pauses), and a heap sized for the mods only, since the world lives in Rust |
| Facade objects (`BlockPos`, `ItemStack`, `BlockState`) | allocation per call | Block states are interned ids, positions are passed as packed longs where the API allows, and objects that mods hold are cached per Horizon id |

## How it fits together

```
 Fabric mod jar                Java side (in the JVM)                  Rust side (Horizon)
 ──────────────                ──────────────────────                  ───────────────────
 fabric.mod.json  ─────────▶  bridge loader (our own, not Knot)        horizon-java-bridge
 entrypoints      ─────────▶   ├─ facade: net.minecraft.* classes  ◀── shared views (player, sections, entities)
 mixins.json      ─────────▶   ├─ Fabric API shim (events, registries) ─▶ FFM downcalls: chat, HUD, registries
                               └─ mixin mapper: target+method+@At   ◀── batched upcalls: tick, hud, events
                                  → Horizon hook point, or refused
```

- **Loader.** A small loader of our own, not Fabric Loader's Knot. It reads `fabric.mod.json`, resolves
  dependencies, loads mod classes into one class loader, and calls `ModInitializer` /
  `ClientModInitializer` entrypoints. Mods use the Fabric Loader API (`FabricLoader.getInstance()`, config
  dirs, `isModLoaded`), so the bridge provides those classes.
- **Facade.** Java classes with Minecraft 26.3's names (26.1 and later ship unobfuscated, so mods name
  Mojang's classes directly and no remapping is needed), backed by Horizon. They are written from the
  method signatures mods actually call, as listed by `horizon-port-pack reach`, and never from vanilla's
  code. They are generated where they can be (getters over shared views), and handwritten where they
  can't.
- **Fabric API shim.** Fabric API's event and registry classes, reimplemented to fire from Horizon's
  hooks. Fabric API is its own open-source project (Apache-2.0): the shim keeps its public class and method
  names, which mods compile against, and is written for Horizon.
- **Mixin mapper.** A table from `(target class, method, @At)` to a Horizon hook point, for example
  `Gui.render` `TAIL` maps to the HUD hook. A mod whose mixins all map runs. A mod with an unmapped mixin
  that matters is reported with that mixin's target, which tells us which hook to add next. The table
  grows from the `reach` data, most-needed first.
- **Sandbox.** There is none: Java mods are trusted code with file and network access, as they are on
  Fabric. The launcher says so before a Java mod is turned on. The WASM mods keep their sandbox.

## Sizing it: `horizon-port-pack reach`

```sh
cargo run -p horizon-port-pack -- reach ~/my-instance/mods ~/.horizon/reach
```

For every jar (and the jars nested in it), this lists the Minecraft classes and members its code names, the
Minecraft classes it extends, the Fabric API modules it uses, other libraries, and each mixin with its
targets, injector kinds, methods and `@At` points. Summed over a folder, `reach.md` ranks every Minecraft
member, mixin target and injected method by how many mods need it. That ranking is the build order for the
facade and the mixin table. It reads names only (the constant pool, the class header and annotations)
and decodes no method body. Like the other `horizon-port-pack` steps, it runs on the player's own jars,
and its output stays in their work folder.

## Benchmarks, from the first mod on

Every step is measured against the same mods on Fabric. The harnesses for this already exist:
`tools/server_bench.py` runs Horizon, vanilla, Paper and Fabric + Lithium with the same bots, and
`tools/perf_bench.py` runs Horizon, vanilla and Fabric with Sodium on the same camera path. Both get a
"modded" mode:

- **Fabric side**: Fabric Loader 0.19.5 (`packs/fabric.lock.json`), Fabric API, the mods under test, and
  Lithium (plus Sodium for the client), since that's how people play.
- **Horizon side**: the same mod jars on the bridge, with the performance mods left out.
- **Reported**: MSPT and P99 tick time, FPS and 1 % lows, memory, start time, and the bridge's own share
  (time in upcalls and downcalls, crossings per tick), so a regression shows where it comes from.
- A step that makes modded Horizon slower than modded Fabric doesn't merge until it's fixed.

## Steps

1. **Scanner and this plan** (this change).
2. **JVM host.** `crates/horizon-java-bridge`: find the player's JDK 25, start it on first use through the
   JNI invocation API, load the loader jar, call one entrypoint, and measure: JVM start time, heap, and the
   cost of an FFM downcall and an upcall in a tight loop. Built and tested against our own test mod
   (`dev/horizonlab`, the only Java the public tree allows).
3. **Tick and HUD.** `ClientTickEvents`, `ServerTickEvents`, `HudRenderCallback`, client commands, and the
   facade for the player, the world clock and text drawing. This is enough for the HUD and info mods the
   compat page lists as "port first", running as they are.
4. **First real mod.** Picked from `reach` data over the most-downloaded 26.3 Fabric client mods: the one
   that needs the least new facade and no unmapped hot mixins. Benchmarked against Fabric before and after.
5. **World reads, registries, networking**, in the order the `reach` ranking gives.

## Risks

- **Mods that patch the game's guts.** Mods whose value is a mixin deep in rendering or world generation (Iris,
  Create's contraptions, many tech mods) need a hook for each patch, and some patches won't have a hook that
  keeps Horizon fast. Those are reported and stay native-port candidates.
- **The facade is big.** Popular mods name hundreds of Minecraft members each. `reach` keeps it to what
  mods use, but the long tail is long, and each new Minecraft version moves it.
- **Two copies of state.** Facade objects that mods keep (an `Entity` reference held across ticks) have to
  stay valid while Horizon owns the real entity. They are handles with an id and a generation, checked on
  use.
