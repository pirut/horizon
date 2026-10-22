# The real Earth in Horizon

Earth Platform ([pirut/earth-platform](https://github.com/pirut/earth-platform)) puts the real planet in
Minecraft 1.21.1 at true scale: one block is one metre, worlds run from Y -16384 to 16383 in 16-block cubes, and
elevation, land cover and climate stream from open data on first visit. This is its rebuild on Horizon, with one
goal: **the Earth loads as fast as the data can arrive.**

`crates/horizon-earth` holds the Earth itself, with no game code: the data sources, the terrain model and cube
generation. It is a port of Earth Platform's `geography` package and `EarthGenerator`, so it is licensed like Earth
Platform, LGPL-3.0-only (the rest of Horizon stays MIT). The cubic engine that will play it is the next stage
(below).

## Why it is faster

Earth Platform's time on a first visit goes to waiting on downloads, then to the JVM working out cubes one block
lookup at a time. horizon-earth changes the shape of both:

- **Everything a place needs is fetched at once.** `Earth::prepare` lists every elevation tile, land cover block
  and climate row a place reads and fetches them on 48 connections together, finest data first. The four climate
  headers load in parallel, and climate rows that sit near each other in their file come in one range request.
- **The player waits only for what is near.** `Earth::prepare_near` fetches what the cubes around the player read
  (2.3 MB for Yosemite against Earth Platform's 43 MB), and the far terrain's data follows while they play.
- **One pass per region, not one lookup per block.** `Earth::region` gathers the elevation pixels a chunk or far
  voxel grid covers into one flat window and evaluates every column from it; land cover is read as a grid
  (latitude depends only on Z and longitude only on X in Web Mercator, so each is worked out once per row or
  column); climate, a 1 km dataset, is read on a 32-block lattice and interpolated. The shared caches are
  touched a handful of times per chunk.
- **Cubes share their column.** The 16 x 16 columns' ground, water and surface are worked out once and every cube
  above and below reads them; buried cubes are filled whole, empty ones are skipped.
- **The disk cache is Earth Platform's.** Same folders and file names (`terrarium-v1/z/x/y.png`,
  `esa-worldcover-v200/...`, `chelsa-v2.1-1981-2010/...`), so a game folder's `earthplatform/cache` works offline
  in Horizon as it is.

## Measured

`cargo run --release -p horizon-earth --example earth_bench -- <cache dir> 4 [place]` measures what Earth
Platform's `CubicBench` measures (`EARTH_BENCH=<cache> EARTH_BENCH_NEAR=1 ./gradlew test --tests '*CubicBench'`),
on the same places and threads. Cloud container, 4 vCPU, every request through an HTTPS proxy (about 0.75 s a
request), so first visits are network-bound; on a home connection the fetch times shrink, the CPU times don't.

Time until a player can play, in seconds: the data the cubes around them read, the landing spot search, and the
6,357 cubes Earth Platform keeps around a player (12 across, 6 up and down), 4 threads. First visit is an empty
cache; return visit is the same place again from disk.

| Place | Earth Platform, first visit | horizon-earth, first visit | Earth Platform, return | horizon-earth, return |
| --- | --- | --- | --- | --- |
| Yosemite Valley | 15.3 (6.8 + 1.5 + 7.0) | 2.5 (2.52 + 0.00 + 0.02) | 8.0 (2.3 + 1.7 + 4.0) | 0.04 |
| Lake Tahoe | 15.4 (7.1 + 3.5 + 4.8) | 1.6 (1.55 + 0.00 + 0.02) | 8.8 (2.2 + 1.7 + 4.8) | 0.05 |
| Amazon tributaries | 14.8 (5.6 + 3.2 + 6.0) | 1.5 (1.41 + 0.00 + 0.08) | 9.3 (2.1 + 1.8 + 5.5) | 0.06 |
| Tokyo | 16.1 (6.4 + 5.3 + 4.4) | 1.4 (1.41 + 0.00 + 0.02) | 8.7 (2.1 + 2.1 + 4.5) | 0.05 |

Far terrain, all eight levels (about 17,000 columns of 16 x 16 voxels, out to about 94 km), CPU time of one
thread on a return visit: Earth Platform 277 to 598 s (estimated from 150 columns a level), horizon-earth 45 to
49 s (every column).

**What the Rust cubes don't have yet**, so the cube numbers are not like for like: caves, ores, trees and plants,
vanilla structures, rock strata, rivers and streams traced from JRC water and the valleys, the national elevation
surveys (3DEP, HRDEM, RGE ALTI, GSI, REMA, Copernicus fill), tile seam mending, and Earth Platform's many
coast, lake and ground special cases. Each comes over as its own change, measured against Earth Platform's
`CubicBench` fingerprints and timings.

## Playing it today

Create New World, World tab, World Type: **Earth** makes an Earth world at Earth Platform's home in Yosemite
Valley; `horizon-client --world NAME --earth LAT,LON` makes one anywhere (a new world only). The world's
generator is `horizon:earth` in `world_gen_settings`, with its latitude, longitude and `shift`. Earth Platform's
`earthplatform/cache` folder in the game folder is read and filled, so the two share downloads.

Until the cubic engine below lands, chunks are still vanilla's 384 blocks tall. An Earth world keeps the true
heights and shows a 384-block window of them: when the world is made, `shift` (the Earth Y of world Y 0, a whole
number of cubes) is set so the spawn's ground stands near Y 16, leaving about 300 blocks of climb before the
window's top. Ground above the window is cut flat at Y 319. Far terrain (`crate::lod`) has no such limit and
draws every mountain at its real height, read a grid at a time from `Earth::region`.

`crates/horizon-client/src/earthgen.rs` is the generator: each section is a cube from `horizon_earth::cube`, with
a bedrock floor at Y -64. Caves, trees and the rest of the decoration come with stage 8.

## Next: the cubic engine

Horizon's world today is vanilla's: chunk columns of 24 sections from Y -64 to 320 (`crate::chunk::HEIGHT`),
saved in Anvil region files and sent in vanilla's packets. An Earth world needs cubes instead, as Earth Platform
built them. The stages, each its own pull request:

1. **Cube storage and generation**: a cube store keyed by cube position next to the chunk map, a
   `CubeGenerator` that streams cubes nearest first in a horizontal circle and a vertical band (Earth Platform's
   12 and 6), filled by `horizon_earth::cube::Cubes`, and an Earth world type that uses it.
2. **Meshing and drawing**: cubes meshed off the main thread, with the face culling across cube borders.
3. **Lighting**: sky light from the top of the loaded cubes down, block light across cube borders.
4. **Saving**: cubes in their own region files, loaded before generating.
5. **Gameplay on cubes**: block, collision, entity and tick lookups by cube, so vanilla rules work at any height.
6. **Networking**: cube packets for Horizon servers and friends.
7. **Far terrain**: Horizon's LOD terrain fed straight from `Earth::region` at each level's voxel size.
8. **Decoration**: trees, plants, ores and caves at any height, from Horizon's vanilla features.
