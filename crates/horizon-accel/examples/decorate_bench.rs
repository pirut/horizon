//! Horizon's FEATURES status timed on its own chunks: how fast the Rust
//! features are, before any of them is swapped into Java.
//!
//! Generates a (SIZE+2) x (SIZE+2) square of proto chunks around a center,
//! then decorates the inner SIZE x SIZE chunks in row order, each with its
//! 3x3 neighborhood, as one thread would. With BIOME set, the square is
//! centered on the closest column of that biome (found at Y=-40) instead of
//! the origin.
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
