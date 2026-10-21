//! Horizon's terrain generation timed through `Accel`, the Rust half of the
//! go/no-go benchmark in docs/research/java-bridge.md. Its Java half is
//! mods/horizon-accel/src/bench (VanillaTerrainBench), and both print the
//! same lines.
//!
//! A round brings a fresh SIZE x SIZE square of chunks to the carvers status
//! (what `Accel::generate` returns, and what the swap hands to Java) with
//! THREADS threads. Rounds move far along x, so no chunk or structure start
//! is reused, and the warmup rounds fill the generator's lazy caches, as a
//! running server's would be.
//!
//! terrain_bench <resources> SEED DIMENSION SIZE THREADS WARMUP ROUNDS
use horizon_accel::Accel;
use horizon_world::worldgen::generator::StageTimings;
use std::{
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

fn ms(d: Duration, n: f64) -> f64 {
    d.as_secs_f64() * 1000.0 / n
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [resources, seed, dimension, size, threads, warmup, rounds] = &args[..] else {
        return Err(
            "usage: terrain_bench <resources> SEED DIMENSION SIZE THREADS WARMUP ROUNDS".into(),
        );
    };
    let number = |s: &str| s.parse::<i64>().map_err(|e| format!("{s}: {e}"));
    let (seed, size, threads) = (
        number(seed)?,
        number(size)? as i32,
        number(threads)? as usize,
    );
    let (warmup, rounds) = (number(warmup)? as usize, number(rounds)? as usize);
    let dimension = if dimension.contains(':') {
        dimension.clone()
    } else {
        format!("minecraft:{dimension}")
    };
    let start = Instant::now();
    let accel = Accel::open(Path::new(resources), seed, &dimension, true)?;
    println!(
        "setup {:.1} ms, {threads} threads, {dimension}, seed {seed}, {size}x{size} chunks per round",
        start.elapsed().as_secs_f64() * 1000.0
    );
    let count = (size * size) as usize;
    let n = count as f64;
    let mut results = Vec::new();
    for round in 0..warmup + rounds {
        let x0 = round as i32 * (size + 32);
        let next = AtomicUsize::new(0);
        let before = accel.stage_timings();
        let start = Instant::now();
        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= count {
                            return;
                        }
                        let (x, z) = (x0 + i as i32 % size, i as i32 / size);
                        accel.generate(x, z).expect("chunk");
                    }
                });
            }
        });
        let wall = start.elapsed();
        let after = accel.stage_timings();
        let stage = |f: fn(&StageTimings) -> Duration| f(&after) - f(&before);
        // Stage times are summed over threads; scale them to wall time so
        // each is its share of the round, as the Java half's are.
        let busy = stage(|t| t.biomes + t.noise + t.surface + t.structures + t.carvers);
        let scale = wall.as_secs_f64() / busy.as_secs_f64().max(1e-9);
        let share = |d: Duration| ms(d, n) * scale;
        let r = [
            share(stage(|t| t.structures)),
            share(stage(|t| t.biomes)),
            share(stage(|t| t.noise + t.surface + t.carvers)),
            ms(wall, n),
        ];
        println!(
            "{} {round}: structures {:.3} biomes {:.3} terrain {:.3} total {:.3} ms/chunk (noise {:.3} surface {:.3} carvers {:.3})",
            if round < warmup { "warmup" } else { "round" },
            r[0],
            r[1],
            r[2],
            r[3],
            share(stage(|t| t.noise)),
            share(stage(|t| t.surface)),
            share(stage(|t| t.carvers)),
        );
        if round >= warmup {
            results.push(r);
        }
    }
    let median = |k: usize| {
        let mut column: Vec<f64> = results.iter().map(|r| r[k]).collect();
        column.sort_by(f64::total_cmp);
        column[column.len() / 2]
    };
    println!(
        "median structures {:.3} biomes {:.3} terrain {:.3} total {:.3} ms/chunk",
        median(0),
        median(1),
        median(2),
        median(3)
    );
    Ok(())
}
