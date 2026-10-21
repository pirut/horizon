#!/usr/bin/env python3
"""Terrain generation, vanilla Java against Horizon's Rust: the go/no-go benchmark of
docs/research/java-bridge.md.

Java side: mods/horizon-accel/src/bench (VanillaTerrainBench) runs vanilla 26.3's own
ChunkGenerator in-process on the pinned server jar. Rust side: horizon-accel's
terrain_bench example runs the same seed and chunks through `Accel`, the code the
swap calls. Both bring fresh 16x16 squares of chunks to the carvers status after
warmup rounds, and the runs alternate Java and Rust so neither gets a quieter machine.

Needs `horizon-prepare --prepare` to have run (the pinned jars under .local/minecraft
and the resources under .local/horizon-client), and a Java 25 JDK.

usage: accel_bench.py [--threads 1,4] [--pairs 2] [--seed 12345]
       [--dimension overworld] [--size 16] [--warmup 3] [--rounds 5] [--json OUT]
"""
import argparse
import json
import os
import re
import statistics
import subprocess
import sys
from pathlib import Path

try:
    from . import lab, test_rust_parity as base
except ImportError:
    import lab
    import test_rust_parity as base

BENCH = lab.ROOT / "mods/horizon-accel/src/bench/java/dev/horizonlab/accel/bench/VanillaTerrainBench.java"
MEDIAN = re.compile(r"median (.*) ms/chunk")


def java_side():
    lock, jars, libraries = base.verify_artifacts()
    classpath = os.pathsep.join(map(str, [jars["server"], *libraries]))
    classes = lab.LOCAL / "accel-bench/classes"
    classes.mkdir(parents=True, exist_ok=True)
    subprocess.run(["javac", "--release", str(lock["minecraft"]["java_major"]), "-proc:none", "-nowarn",
                    "-cp", classpath, "-d", str(classes), str(BENCH)], check=True)
    return ["java", "-Xmx8G", "-cp", str(classes) + os.pathsep + classpath,
            "dev.horizonlab.accel.bench.VanillaTerrainBench"]


def rust_side():
    subprocess.run(["cargo", "build", "--release", "-q", "-p", "horizon-accel", "--example", "terrain_bench"],
                   check=True, cwd=lab.ROOT)
    metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--no-deps", "--format-version", "1"],
                                                  cwd=lab.ROOT, text=True))
    return [str(Path(metadata["target_directory"]) / "release/examples/terrain_bench"), str(lab.RESOURCES)]


def run(command, args):
    result = subprocess.run([*command, *map(str, args)], capture_output=True, text=True)
    out = result.stdout + result.stderr
    match = MEDIAN.search(out)
    if result.returncode or not match:
        raise SystemExit(f"{command[-1]} failed:\n{out[-4000:]}")
    words = match.group(1).split()
    return {words[i]: float(words[i + 1]) for i in range(0, len(words), 2)}


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--threads", default="1,4")
    parser.add_argument("--pairs", type=int, default=2)
    parser.add_argument("--seed", type=int, default=12345)
    parser.add_argument("--dimension", default="overworld")
    parser.add_argument("--size", type=int, default=16)
    parser.add_argument("--warmup", type=int, default=3)
    parser.add_argument("--rounds", type=int, default=5)
    parser.add_argument("--json", type=Path)
    options = parser.parse_args()
    java, rust = java_side(), rust_side()
    results = []
    for threads in map(int, options.threads.split(",")):
        args = [options.seed, options.dimension, options.size, threads, options.warmup, options.rounds]
        pairs = []
        for _ in range(options.pairs):
            pairs.append((run(java, args), run(rust, args)))
        java_terrain = statistics.median(j["terrain"] for j, _ in pairs)
        rust_terrain = statistics.median(r["terrain"] for _, r in pairs)
        java_total = statistics.median(j["total"] for j, _ in pairs)
        rust_total = statistics.median(r["total"] for _, r in pairs)
        results.append({"threads": threads, "pairs": pairs, "java_terrain": java_terrain,
                        "rust_terrain": rust_terrain, "java_total": java_total, "rust_total": rust_total})
        print(f"{threads} threads: terrain {java_terrain:.2f} -> {rust_terrain:.2f} ms/chunk "
              f"({java_terrain / rust_terrain:.2f}x), to carvers {java_total:.2f} -> {rust_total:.2f} "
              f"({java_total / rust_total:.2f}x)", flush=True)
    if options.json:
        options.json.write_text(json.dumps({"seed": options.seed, "dimension": options.dimension,
                                            "size": options.size, "results": results}, indent=2) + "\n")


if __name__ == "__main__":
    sys.exit(main())
