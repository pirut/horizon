//! The Earth world's load time, place by place, measured as Earth Platform's CubicBench measures
//! it: fetching a place's data, finding a landing spot, the cubes around a player standing there,
//! and far terrain at every level.
//!
//! `cargo run --release -p horizon-earth --example earth_bench -- <cache dir> [threads] [place]`
//! An empty cache dir measures a first visit; run again for a return visit.
use horizon_earth::{Earth, Settings, cube::Cubes};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Instant,
};

const PLACES: [(&str, f64, f64); 6] = [
    ("Yosemite Valley", 37.7240, -119.6365),
    ("Lake Tahoe", 38.9520, -120.1080),
    ("Amazon", -3.4653, -62.2159),
    ("Amazon tributaries", -3.3, -60.6),
    ("Tokyo", 35.6812, 139.7671),
    ("Mato Grosso", -11.8243, -55.5029),
];
/// Earth Platform's cube radii around a player: 12 across, 6 up and down.
const H: i32 = 12;
const V: i32 = 6;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cache: String = args.get(1).expect("cache dir").clone();
    let threads: usize = args.get(2).map_or(4, |t| t.parse().unwrap());
    let only = args.get(3).map(|s| s.to_lowercase());
    for (name, lat, lon) in PLACES {
        if only
            .as_ref()
            .is_some_and(|o| !name.to_lowercase().starts_with(o.as_str()))
        {
            continue;
        }
        let mut settings = Settings::at(lat, lon);
        settings.cache = Some(std::path::PathBuf::from(&cache));
        let earth = Arc::new(Earth::new(settings));
        let (hx, hz) = earth.home();
        println!("\n== {name} (block {hx}, {hz})");
        let t = Instant::now();
        let near = earth.prepare_near(hx, hz, H * 16 + 64);
        let (files, bytes, _, _) = earth.http().totals.snapshot();
        println!(
            "prepare near (what the cubes need): {:.0} ms, {} pieces, {files} files {:.1} MB",
            t.elapsed().as_secs_f64() * 1e3,
            near.pieces,
            bytes as f64 / 1e6
        );
        // Far terrain reaches 94 km by default in Earth Platform; its prepare fetches to level 6.
        let prepared = earth.prepare(hx, hz, H * 16 + 64, 17_600);
        let (files, bytes, waiting, disk) = earth.http().totals.snapshot();
        println!(
            "prepare (data fetch): {:.0} ms, {} pieces, {files} files {:.1} MB downloaded ({:.1} s waiting), {disk} disk reads, {} failed{}",
            t.elapsed().as_secs_f64() * 1e3,
            prepared.pieces,
            bytes as f64 / 1e6,
            waiting.as_secs_f64(),
            prepared.failed,
            prepared
                .first_failure
                .map(|f| format!(" (first: {f})"))
                .unwrap_or_default()
        );
        let t = Instant::now();
        let (sx, sy, sz) = earth.spawn();
        println!(
            "landing spot ({sx}, {sy}, {sz}): {:.0} ms",
            t.elapsed().as_secs_f64() * 1e3
        );

        let cubes = Cubes::new(earth.clone());
        let (cx, cy, cz) = (sx >> 4, (sy + 2) >> 4, sz >> 4);
        let mut offsets = Vec::new();
        for y in -V..=V {
            for z in -H..=H {
                for x in -H..=H {
                    if (x * x + z * z) as i64 <= (H * H + H) as i64 {
                        offsets.push((x, y, z));
                    }
                }
            }
        }
        offsets.sort_by_key(|&(x, y, z)| (x * x + z * z) * 4 + 9 * y * y);
        let next = AtomicUsize::new(0);
        let solid = AtomicUsize::new(0);
        let t = Instant::now();
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&(dx, dy, dz)) = offsets.get(i) else {
                            break;
                        };
                        if cubes.cube(cx + dx, cy + dy, cz + dz).is_some() {
                            solid.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                });
            }
        });
        let wall = t.elapsed().as_secs_f64() * 1e3;
        println!(
            "cubes: {} in {wall:.0} ms on {threads} threads = {:.0} cubes/s ({:.3} ms per cube per thread), {} not empty",
            offsets.len(),
            offsets.len() as f64 / wall * 1e3,
            wall * threads as f64 / offsets.len() as f64,
            solid.load(Ordering::Relaxed)
        );

        // Far columns: 16 x 16 voxels of 2^level blocks, the 46 x 46 around the player per level.
        let mut total = 0.0;
        for level in 1..=8 {
            let voxel = 1 << level;
            let (px, pz) = (sx.div_euclid(voxel * 16), sz.div_euclid(voxel * 16));
            let mut keys = Vec::new();
            for dz in -23..23 {
                for dx in -23..23 {
                    keys.push((px + dx, pz + dz));
                }
            }
            keys.sort_by_key(|&(x, z)| (x - px).pow(2) + (z - pz).pow(2));
            let next = AtomicUsize::new(0);
            let t = Instant::now();
            std::thread::scope(|s| {
                for _ in 0..threads {
                    s.spawn(|| {
                        loop {
                            let i = next.fetch_add(1, Ordering::Relaxed);
                            let Some(&(x, z)) = keys.get(i) else { break };
                            std::hint::black_box(earth.region(
                                x * voxel * 16,
                                z * voxel * 16,
                                16,
                                voxel,
                            ));
                        }
                    });
                }
            });
            let wall = t.elapsed().as_secs_f64() * 1e3;
            total += wall * threads as f64;
            println!(
                "far level {level}: {} columns in {wall:.0} ms, {:.2} ms per column per thread",
                keys.len(),
                wall * threads as f64 / keys.len() as f64
            );
        }
        let (files, bytes, _, _) = earth.http().totals.snapshot();
        println!(
            "far: about {:.1} s of one thread for all levels; {files} files {:.1} MB downloaded in all",
            total / 1e3,
            bytes as f64 / 1e6
        );
    }
}
