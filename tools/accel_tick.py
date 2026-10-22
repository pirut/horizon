#!/usr/bin/env python3
"""Server ticking on a real Fabric 26.3 server, with and without horizon-accel and other mods.

Each configuration gets a fresh world with the same seed. A square of chunks around the origin is
force-loaded, the same mobs are summoned on its surface (farm animals, villagers, monsters kept
from burning by a fixed night), the server settles, and `tick sprint` runs a fixed number of ticks
as fast as it can. The sprint's own report (ms per tick) is the number: it is the server's whole
tick, mobs' AI, pathfinding, movement and collisions, block and fluid ticks, and chunk work.

Configurations are '+'-joined mod names from the folder of jars (--mods), as in accel_pregen.py.

usage: accel_tick.py --mods DIR [--configs vanilla,lithium,accel] [--chunks 8] [--mobs 1200]
       [--ticks 3000] [--seed 12345] [--repeat 1] [--heap 6G] [--jfr] [--json OUT]
"""
import argparse
import json
import os
from pathlib import Path
import queue
import random
import re
import shutil
import subprocess
import sys
import threading
import time

try:
    from . import lab
    from .accel_pregen import jar, cpu_seconds
except ImportError:
    import lab
    from accel_pregen import jar, cpu_seconds

ROOT = lab.ROOT
RUNS = lab.LOCAL / "accel-tick"
REPORT = re.compile(r"Sprint completed with ([\d.,]+) ticks per second, or ([\d.,]+) ms per tick")
# Summoned in these shares; monsters stay because the night is fixed and the difficulty is normal.
MOBS = [("cow", 3), ("sheep", 3), ("pig", 2), ("chicken", 2), ("villager", 2), ("zombie", 2),
        ("skeleton", 1), ("spider", 1), ("creeper", 1), ("iron_golem", 1)]


def number(text):
    return float(text.replace(",", ""))


def run(config, options):
    world = RUNS / config
    if world.exists():
        shutil.rmtree(world)
    (world / "mods").mkdir(parents=True)
    shutil.copy(jar(options.mods, "fabric-server"), world / "fabric-server.jar")
    shutil.copy(lab.LOCAL / f"minecraft/{lab.VERSION}/jars/server.jar", world / "server.jar")
    shutil.copy(jar(options.mods, "fabric-api"), world / "mods")
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
        f"online-mode=false\nlevel-seed={options.seed}\nview-distance=4\nsimulation-distance=10\n"
        "spawn-protection=0\nsync-chunk-writes=false\ndifficulty=normal\nmax-tick-time=-1\n")
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
            match = re.search(pattern, line)
            if match:
                return match
        raise SystemExit(f"{config}: timed out waiting for {pattern}")

    wait(r"Done \(", 600)
    half = options.chunks * 8
    for line in ["gamerule doDaylightCycle false", "gamerule doWeatherCycle false", "time set midnight",
                 "weather clear", "gamerule doMobSpawning false", "gamerule doPatrolSpawning false",
                 "gamerule doTraderSpawning false", "gamerule randomTickSpeed 3",
                 f"forceload add {-half} {-half} {half - 1} {half - 1}"]:
        send(line)
    send("say forceloaded")
    wait(r"forceloaded", 600)
    # The same mobs at the same places in every run (the terrain is the same for one seed).
    rng = random.Random(options.seed)
    kinds = [kind for kind, share in MOBS for _ in range(share)]
    for i in range(options.mobs):
        x, z = rng.randrange(-half + 2, half - 2), rng.randrange(-half + 2, half - 2)
        send(f"execute positioned {x} 0 {z} positioned over motion_blocking run summon {kinds[i % len(kinds)]} ~ ~ ~")
    send("say summoned")
    wait(r"summoned", 600)
    # Let the mobs land and the chunks' first ticks pass before measuring.
    send(f"tick sprint {options.settle}")
    wait(REPORT.pattern, 3600)
    cpu_began = cpu_seconds(server.pid)
    began = time.time()
    send(f"tick sprint {options.ticks}")
    match = wait(REPORT.pattern, 7200)
    elapsed = time.time() - began
    cpu = cpu_seconds(server.pid) - cpu_began
    send("stop")
    server.wait(timeout=300)
    log.close()
    mspt = number(match.group(2))
    result = {"config": config, "ticks": options.ticks, "mobs": options.mobs, "mspt": mspt,
              "wall_s": elapsed, "cpu_ms_per_tick": cpu * 1000 / options.ticks}
    print(f"{config}: {mspt:.2f} ms/tick, {cpu * 1000 / options.ticks:.1f} CPU ms/tick "
          f"({options.ticks} ticks, {options.mobs} mobs)", flush=True)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--mods", type=Path, required=True)
    parser.add_argument("--configs", default="vanilla,lithium,accel")
    parser.add_argument("--chunks", type=int, default=8, help="side of the force-loaded square, in chunks")
    parser.add_argument("--mobs", type=int, default=1200)
    parser.add_argument("--ticks", type=int, default=3000)
    parser.add_argument("--settle", type=int, default=400)
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
