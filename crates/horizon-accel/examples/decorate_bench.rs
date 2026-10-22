//! Horizon's FEATURES status timed on its own chunks: how fast the Rust
//! features are, before any of them is swapped into Java.
//!
//! Generates a (SIZE+2) x (SIZE+2) square of proto chunks around a center,
//! then decorates the inner SIZE x SIZE chunks in row order, each with its
//! 3x3 neighborhood, as one thread would. With BIOME set, the square is
//! centered on the closest column of that biome (found at Y=-40) instead of
//! the origin.
//!
//! With DECORATE_COPY=1 it decorates a second copy of the square the way the
//! features swap does: each region is first rebuilt from its blocks alone
//! (`proto_from_sections`), decorated with its write log, and copied back.
//! The two copies must end up with the same blocks.
//!
//! decorate_bench <resources> SEED SIZE [BIOME | X,Z]
use horizon_core::position::ChunkPos;
use horizon_world::{
    registry::BlockRegistry,
    worldgen::generator::{BiomeSearch, GeneratorData, OverworldGenerator, ProtoChunk},
};
use std::{collections::HashMap, fs, path::Path, time::Instant};

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (resources, seed, size) = match &args[..] {
        [r, s, n, ..] => (
            r,
            s.parse::<i64>().map_err(|e| e.to_string())?,
            n.parse::<i32>().map_err(|e| e.to_string())?,
        ),
        _ => return Err("usage: decorate_bench <resources> SEED SIZE [BIOME]".into()),
    };
    let resources = Path::new(resources);
    let registry = BlockRegistry::from_facts_json(
        fs::File::open(resources.join("facts.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let registry: &'static BlockRegistry = Box::leak(Box::new(registry));
    let data =
        GeneratorData::load(&resources.join("worldgen"), registry).map_err(|e| e.to_string())?;
    let generator =
        OverworldGenerator::for_world(data, seed, "minecraft:normal", "minecraft:overworld", true)
            .map_err(|e| e.to_string())?;
    let mut worker = generator.worker();
    let center = match args.get(3) {
        // X,Z: the square's minimum chunk, as VanillaTerrainBench's rounds place them.
        Some(corner) if corner.contains(',') => {
            let (x, z) = corner.split_once(',').unwrap();
            let number = |s: &str| s.parse::<i32>().map_err(|e| e.to_string());
            ChunkPos::new(number(x)? + size / 2, number(z)? + size / 2)
        }
        Some(biome) => {
            let wanted = biome.clone();
            let allowed = move |name: &str| name == wanted;
            let found = worker
                .find_closest_biome(&BiomeSearch {
                    allowed: &allowed,
                    origin: [0, -40, 0],
                    radius: 6400,
                    horizontal: 32,
                    vertical: 64,
                    min_y: -64,
                    max_y: 320,
                })
                .ok_or("biome not found")?;
            println!("{} at {:?}", found.0, found.1);
            ChunkPos::new(found.1[0] >> 4, found.1[2] >> 4)
        }
        None => ChunkPos::new(0, 0),
    };
    let half = size / 2;
    let start = Instant::now();
    let mut chunks: HashMap<(i32, i32), ProtoChunk> = HashMap::new();
    for dz in -half - 1..=size - half {
        for dx in -half - 1..=size - half {
            let pos = ChunkPos::new(center.x + dx, center.z + dz);
            chunks.insert(
                (pos.x, pos.z),
                worker.generate_proto(pos).map_err(|e| e.to_string())?,
            );
        }
    }
    let generated = start.elapsed();
    let start = Instant::now();
    let (mut placed, mut unsupported, mut failed) = (0, 0, 0);
    let mut skipped = std::collections::BTreeSet::new();
    for dz in -half..size - half {
        for dx in -half..size - half {
            let (cx, cz) = (center.x + dx, center.z + dz);
            let mut take = |x, z| chunks.remove(&(x, z)).expect("generated neighbor");
            let mut region: [ProtoChunk; 9] =
                std::array::from_fn(|i| take(cx + (i % 3) as i32 - 1, cz + (i / 3) as i32 - 1));
            let stats = worker.decorate(&mut region).map_err(|e| e.to_string())?;
            placed += stats.placed;
            unsupported += stats.unsupported;
            failed += stats.failed;
            skipped.extend(stats.skipped);
            for (i, chunk) in region.into_iter().enumerate() {
                chunks.insert((cx + (i % 3) as i32 - 1, cz + (i / 3) as i32 - 1), chunk);
            }
        }
    }
    let decorated = start.elapsed();
    if std::env::var_os("DECORATE_COPY").is_some() {
        let mut copies: HashMap<(i32, i32), ProtoChunk> = HashMap::new();
        for &(x, z) in chunks.keys() {
            copies.insert(
                (x, z),
                worker
                    .generate_proto(ChunkPos::new(x, z))
                    .map_err(|e| e.to_string())?,
            );
        }
        let (mut copying, mut deco, mut ops) =
            (std::time::Duration::ZERO, std::time::Duration::ZERO, 0);
        for dz in -half..size - half {
            for dx in -half..size - half {
                let (cx, cz) = (center.x + dx, center.z + dz);
                let clock = Instant::now();
                let mut region = Vec::with_capacity(9);
                for i in 0..9 {
                    let pos = ChunkPos::new(cx + (i % 3) as i32 - 1, cz + (i / 3) as i32 - 1);
                    let source = &copies[&(pos.x, pos.z)];
                    let sections = (0..)
                        .map_while(|s| source.terrain.section(s))
                        .map(|section| {
                            let (palette, words) = section.packed();
                            horizon_world::chunk::BlockSection::from_packed(
                                palette,
                                section.bits(),
                                words,
                                registry,
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|e| e.to_string())?;
                    let heightmaps: Vec<_> = horizon_world::heightmap::HeightmapType::ALL
                        .into_iter()
                        .filter_map(|kind| {
                            source
                                .terrain
                                .heightmap(kind)
                                .map(|map| (kind, map.raw_data()))
                        })
                        .collect();
                    let mut copy = worker
                        .proto_from_sections(pos, sections, &heightmaps, i == 4)
                        .map_err(|e| e.to_string())?;
                    if source.is_decorated() {
                        copy.set_decorated();
                    }
                    if std::env::var_os("DECORATE_DEBUG").is_some() {
                        use horizon_world::heightmap::HeightmapType as H;
                        for kind in [
                            H::OceanFloorWg,
                            H::WorldSurfaceWg,
                            H::MotionBlocking,
                            H::MotionBlockingNoLeaves,
                            H::OceanFloor,
                            H::WorldSurface,
                        ] {
                            let (a, b) =
                                (source.terrain.heightmap(kind), copy.terrain.heightmap(kind));
                            let differ = match (a, b) {
                                (Some(a), Some(b)) => (0..256)
                                    .filter(|i| {
                                        a.first_available(i % 16, i / 16).ok()
                                            != b.first_available(i % 16, i / 16).ok()
                                    })
                                    .count(),
                                (a, b) => {
                                    if a.is_some() != b.is_some() {
                                        999
                                    } else {
                                        0
                                    }
                                }
                            };
                            if differ > 0 {
                                println!(
                                    "chunk {:?} {kind:?}: {differ} columns differ (decorated {})",
                                    (pos.x, pos.z),
                                    source.is_decorated()
                                );
                            }
                        }
                        if i == 4 {
                            println!(
                                "chunk {:?}: {} vs {} structure starts",
                                (pos.x, pos.z),
                                source.structure_start_count(),
                                copy.structure_start_count()
                            );
                        }
                    }
                    region.push(copy);
                }
                let mut region: [ProtoChunk; 9] = region.try_into().map_err(|_| "region")?;
                copying += clock.elapsed();
                let clock = Instant::now();
                let (_, log) = worker
                    .decorate_logged(&mut region)
                    .map_err(|e| e.to_string())?;
                deco += clock.elapsed();
                ops += log.len();
                if std::env::var_os("DECORATE_DEBUG").is_some() {
                    let mut kinds = std::collections::BTreeMap::new();
                    for op in &log {
                        let name = format!("{op:?}");
                        *kinds
                            .entry(name.split('(').next().unwrap().to_owned())
                            .or_insert(0) += 1;
                    }
                    println!("ops {kinds:?}");
                }
                for (i, chunk) in region.into_iter().enumerate() {
                    copies.insert((cx + (i % 3) as i32 - 1, cz + (i / 3) as i32 - 1), chunk);
                }
            }
        }
        let n = (size * size) as f64;
        println!(
            "copied: copy {:.2} ms/chunk, decorate {:.2} ms/chunk, {:.0} writes/chunk",
            copying.as_secs_f64() * 1000.0 / n,
            deco.as_secs_f64() * 1000.0 / n,
            ops as f64 / n
        );
        let mut differ = 0;
        for (key, a) in &chunks {
            let b = &copies[key];
            let mut same = true;
            for s in 0.. {
                let (Some(x), Some(y)) = (a.terrain.section(s), b.terrain.section(s)) else {
                    break;
                };
                if (0..4096).any(|i| x.get(i).unwrap() != y.get(i).unwrap()) {
                    same = false;
                    break;
                }
            }
            if !same {
                differ += 1;
            }
        }
        println!(
            "{} of {} chunks identical",
            chunks.len() - differ,
            chunks.len()
        );
    }
    let n = (size * size) as f64;
    println!(
        "{n} chunks: generate {:.2} ms/chunk, decorate {:.2} ms/chunk ({placed} roots, {unsupported} unsupported, {failed} failed)",
        generated.as_secs_f64() * 1000.0 / ((size + 2) * (size + 2)) as f64,
        decorated.as_secs_f64() * 1000.0 / n
    );
    if !skipped.is_empty() {
        println!("skipped: {skipped:?}");
    }
    Ok(())
}
