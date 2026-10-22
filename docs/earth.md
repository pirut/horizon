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

Yosemite Valley, first visit (empty cache), 4 threads:

| | Earth Platform | horizon-earth |
| --- | --- | --- |
| Data the cubes need | 6.8 s, 263 files, 43 MB | 2.9 s, 22 files, 2.3 MB |
| Landing spot search | 1.5 s | 2 ms |
| The 6,357 cubes around the player | 7.0 s | 21 ms |
| **Until the player can play** | **15.3 s** | **2.9 s** |

Return visits, the other places and far terrain: see the tables in the pull request that adds each stage.

**What the Rust cubes don't have yet**, so the cube numbers are not like for like: caves, ores, trees and plants,
vanilla structures, rock strata, rivers and streams traced from JRC water and the valleys, the national elevation
surveys (3DEP, HRDEM, RGE ALTI, GSI, REMA, Copernicus fill), tile seam mending, and Earth Platform's many
coast, lake and ground special cases. Each comes over as its own change, measured against Earth Platform's
`CubicBench` fingerprints and timings.

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
