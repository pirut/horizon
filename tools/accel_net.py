#!/usr/bin/env python3
"""Players on a real Fabric 26.3 server, with and without horizon-accel and other mods: horizon-bots log in,
fly around and receive chunks, as a busy multiplayer server sees it.

Each configuration gets a fresh world with the same seed. Once the server is up, horizon-bots
(crates/horizon-bots, built with `cargo build --release -p horizon-bots`) brings the same bots with the same
moves in every run. The numbers are the chunks the bots received, the server's CPU time per chunk sent (world
generation, lighting, saving, packing and compressing chunk packets) and its tick time (`/tick query`).

Configurations are '+'-joined mod names from the folder of jars (--mods), as in accel_pregen.py; `-zlib`
turns horizon-accel's zlib off (`accel-zlib`), for comparing it on and off with the rest the same.

usage: accel_net.py --mods DIR [--configs c2me+scalablelux+accel-zlib,c2me+scalablelux+accel] [--bots 20]
       [--seconds 90] [--behaviour mixed] [--seed 12345] [--repeat 1] [--heap 6G] [--jfr] [--json OUT]
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
    from .accel_pregen import jar, cpu_seconds
except ImportError:
    import lab
    from accel_pregen import jar, cpu_seconds

ROOT = lab.ROOT
RUNS = lab.LOCAL / "accel-net"
BOTS = ROOT / "target/release" / ("horizon-bots.exe" if os.name == "nt" else "horizon-bots")
MSPT = re.compile(r"Average time per tick: ([\d.,]+)ms")


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
        if mod.startswith("accel"):
            shutil.copy(sorted((ROOT / "dist").glob("horizon-accel-*.jar"))[-1], world / "mods")
            library = next(p for p in (ROOT / "target/release").glob("*horizon_accel.*")
                           if p.suffix in (".so", ".dylib", ".dll"))
            flags += [f"-Dhorizon.accel.library={library}", f"-Dhorizon.accel.resources={lab.RESOURCES}"]
            if mod == "accel-zlib":
                flags.append("-Dhorizon.accel.zlib=false")
        else:
            shutil.copy(jar(options.mods, mod), world / "mods")
    (world / "eula.txt").write_text("eula=true\n")
    (world / "server.properties").write_text(
        f"online-mode=false\nlevel-seed={options.seed}\nview-distance=10\nsimulation-distance=6\n"
        "spawn-protection=0\nsync-chunk-writes=false\ngamemode=creative\nforce-gamemode=true\nwhite-list=false\nenforce-whitelist=false\n"
        f"max-players={options.bots + 10}\nmax-tick-time=-1\nnetwork-compression-threshold=256\n")
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
            if "horizon-accel:" in line and "zlib" in line:
                print("   ", line.strip().split("]: ", 1)[-1], flush=True)
            match = re.search(pattern, line)
            if match:
                return match
        raise SystemExit(f"{config}: timed out waiting for {pattern}")

    wait(r"Done \(", 600)
    report = world / "bots.json"
    cpu_began = cpu_seconds(server.pid)
    began = time.time()
    subprocess.run([str(BOTS), "--server", "127.0.0.1:25565", "--bots", str(options.bots), "--behaviour",
                    options.behaviour, "--seconds", str(options.seconds), "--seed", str(options.seed),
                    "--report", str(report)], check=False, stdout=open(world / "bots.log", "w"),
                   stderr=subprocess.STDOUT)
    elapsed = time.time() - began
    cpu = cpu_seconds(server.pid) - cpu_began
    send("tick query")
    mspt = wait(MSPT.pattern, 60).group(1)
    send("stop")
    server.wait(timeout=300)
    log.close()
    bots = json.loads(report.read_text())
    chunks = bots.get("chunks") or bots.get("chunks_received") or 0
    result = {"config": config, "bots": options.bots, "seconds": elapsed, "cpu_s": cpu, "report": bots,
              "mspt": float(mspt.replace(",", "")), "chunks": chunks,
              "cpu_ms_per_chunk": cpu * 1000 / chunks if chunks else None}
    print(f"{config}: {chunks} chunks to {options.bots} bots in {elapsed:.0f} s, "
          f"{result['cpu_ms_per_chunk'] or 0:.2f} CPU ms/chunk, {result['mspt']:.1f} ms/tick", flush=True)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--mods", type=Path, required=True)
    parser.add_argument("--configs", default="c2me+scalablelux+accel-zlib,c2me+scalablelux+accel")
    parser.add_argument("--bots", type=int, default=20)
    parser.add_argument("--seconds", type=int, default=90)
    parser.add_argument("--behaviour", default="mixed")
    parser.add_argument("--seed", type=int, default=12345)
    parser.add_argument("--repeat", type=int, default=1)
    parser.add_argument("--heap", default="6G")
    parser.add_argument("--json", type=Path)
    parser.add_argument("--java-arg", action="append", default=[], help="an extra JVM argument for every server")
    parser.add_argument("--jfr", action="store_true", help="record each run with Java Flight Recorder (profile.jfr)")
    options = parser.parse_args()
    if not BOTS.is_file():
        raise SystemExit(f"no {BOTS}: cargo build --release -p horizon-bots")
    results = []
    for _ in range(options.repeat):
        for config in options.configs.split(","):
            results.append(run(config, options))
    if options.json:
        options.json.write_text(json.dumps(results, indent=2) + "\n")


if __name__ == "__main__":
    sys.exit(main())
