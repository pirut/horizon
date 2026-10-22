#!/usr/bin/env python3
"""World pregeneration on a real Fabric 26.3 server, with and without horizon-accel.

Each configuration gets a fresh world with the same seed, then Chunky generates the same square of
chunks (every status through features and light, as players exploring would cause) while the server
runs idle. The time Chunky takes, and chunks per second, is the number: it includes everything a
server does per chunk, so horizon-accel's share is measured where it lands, not in isolation.

Configurations are '+'-joined mod names from the folder of jars (--mods): `vanilla` is Fabric with
only Fabric API and Chunky; `accel` adds horizon-accel (dist/, from tools/build_accel_mod.py).

usage: accel_pregen.py --mods DIR [--configs vanilla,lithium,c2me,accel,lithium+accel]
       [--radius 512] [--seed 12345] [--repeat 1] [--heap 6G] [--json OUT]
DIR holds fabric-server.jar (Fabric's server launcher), fabric-api*.jar, Chunky*.jar and
the optional mods (lithium*.jar, c2me*.jar, ...). The game jar is your pinned server jar.
"""
import argparse
import json
import os
from pathlib import Path
import queue
import re
import shutil
import subprocess
import sys
import threading
import time

try:
    from . import lab
except ImportError:
    import lab

ROOT = lab.ROOT
RUNS = lab.LOCAL / "accel-pregen"
FINISHED = re.compile(r"Task finished for minecraft:overworld|Task (?:stopped|cancelled)")
PROGRESS = re.compile(r"\[Chunky\] Task running for minecraft:overworld\. Processed: (\d+) chunks \(([\d.]+)%\), ETA: ([\d:]+), Rate: ([\d.]+) cps")


def jar(mods, prefix):
    found = sorted(p for p in mods.glob("*.jar") if p.name.lower().startswith(prefix))
    if not found:
        raise SystemExit(f"no {prefix}*.jar in {mods}")
    return found[-1]


def run(config, options):
    world = RUNS / config
    if world.exists():
        shutil.rmtree(world)
    (world / "mods").mkdir(parents=True)
    shutil.copy(jar(options.mods, "fabric-server"), world / "fabric-server.jar")
    shutil.copy(lab.LOCAL / f"minecraft/{lab.VERSION}/jars/server.jar", world / "server.jar")
    for name in ["fabric-api", "chunky"]:
        shutil.copy(jar(options.mods, name), world / "mods")
    flags = []
    for mod in config.split("+"):
        if mod == "vanilla":
            continue
        if mod == "accel":
            shutil.copy(sorted((ROOT / "dist").glob("horizon-accel-*.jar"))[-1], world / "mods")
            library = next(p for p in (ROOT / "target/release").glob("*horizon_accel.*")
                           if p.suffix in (".so", ".dylib", ".dll"))
            flags += [f"-Dhorizon.accel.library={library}", f"-Dhorizon.accel.resources={lab.RESOURCES}"]
        else:
            shutil.copy(jar(options.mods, mod), world / "mods")
    (world / "eula.txt").write_text("eula=true\n")
    (world / "server.properties").write_text(
        f"online-mode=false\nlevel-seed={options.seed}\nview-distance=2\nsimulation-distance=2\n"
        "spawn-protection=0\nsync-chunk-writes=false\n")
    flags += options.java_arg
    if options.jfr:
        flags.append("-XX:StartFlightRecording=filename=profile.jfr,settings=profile")
    command = ["java", f"-Xms{options.heap}", f"-Xmx{options.heap}", "--enable-native-access=ALL-UNNAMED", *flags,
               "-jar", "fabric-server.jar", "nogui"]
    env = {k: v for k, v in os.environ.items() if k != "JAVA_TOOL_OPTIONS"}
    server = subprocess.Popen(command, cwd=world, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                              stderr=subprocess.STDOUT, text=True, env=env)
    lines = queue.Queue()
    log = open(world / "console.log", "w")

    def pump():
        for line in server.stdout:
            log.write(line)
            lines.put(line)
        lines.put(None)

    threading.Thread(target=pump, daemon=True).start()

    def send(text):
        server.stdin.write(text + "\n")
        server.stdin.flush()

    def wait(pattern, timeout):
        deadline = time.time() + timeout
        while time.time() < deadline:
            try:
                line = lines.get(timeout=1)
            except queue.Empty:
                continue
            if line is None:
                raise SystemExit(f"{config}: server exited, see {world / 'console.log'}")
            if "horizon-accel:" in line:
                print("   ", line.strip().split("]: ", 1)[-1], flush=True)
            if re.search(pattern, line):
                return line
        raise SystemExit(f"{config}: timed out waiting for {pattern}")

    started = time.time()
    wait(r"Done \(", 600)
    boot = time.time() - started
    for command in ["chunky world minecraft:overworld", "chunky shape square", "chunky center 0 0",
                    f"chunky radius {options.radius}", "chunky silent", "chunky start"]:
        send(command)
    began = time.time()
    wait(FINISHED.pattern, 7200)
    elapsed = time.time() - began
    send("stop")
    server.wait(timeout=300)
    log.close()
    side = (2 * options.radius // 16) + 1
    chunks = side * side
    result = {"config": config, "boot_s": boot, "pregen_s": elapsed, "chunks": chunks, "cps": chunks / elapsed}
    print(f"{config}: {chunks} chunks in {elapsed:.1f} s, {chunks / elapsed:.1f} chunks/s (boot {boot:.1f} s)",
          flush=True)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--mods", type=Path, required=True)
    parser.add_argument("--configs", default="vanilla,lithium,c2me,accel,lithium+accel")
    parser.add_argument("--radius", type=int, default=512)
    parser.add_argument("--seed", type=int, default=12345)
    parser.add_argument("--repeat", type=int, default=1)
    parser.add_argument("--heap", default="6G")
    parser.add_argument("--json", type=Path)
    parser.add_argument("--java-arg", action="append", default=[], help="an extra JVM argument for every server")
    parser.add_argument("--jfr", action="store_true", help="record each run with Java Flight Recorder (profile.jfr)")
    options = parser.parse_args()
    results = []
    for _ in range(options.repeat):
        for config in options.configs.split(","):
            results.append(run(config, options))
    if options.json:
        options.json.write_text(json.dumps(results, indent=2) + "\n")


if __name__ == "__main__":
    sys.exit(main())
