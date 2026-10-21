#!/usr/bin/env python3
"""Build the horizon-accel Fabric mod (mods/horizon-accel) and its native library.

Compiles only original dev/horizonlab sources against the pinned 26.3 server jar, Fabric Loader
and Mixin, and never bundles game classes. Output: dist/horizon-accel-<version>.jar, plus the
release build of crates/horizon-accel (target/release/libhorizon_accel.*), which the mod loads
from -Dhorizon.accel.library.

usage: build_accel_mod.py
"""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import urllib.request
import zipfile

try:
    from . import lab, test_rust_parity as base
except ImportError:
    import lab
    import test_rust_parity as base

ROOT = lab.ROOT
SOURCE = ROOT / "mods/horizon-accel/src"
BUILD = lab.LOCAL / "accel-build"
MAVEN = "https://maven.fabricmc.net/"
# Compile-time only: the Fabric Loader and Mixin a 26.3 Fabric install runs.
COMPILE = ["net.fabricmc:fabric-loader:0.20.1", "net.fabricmc:sponge-mixin:0.17.5+mixin.0.8.7"]


def maven(coordinate):
    group, artifact, version = coordinate.split(":")
    path = f"{group.replace('.', '/')}/{artifact}/{version}/{artifact}-{version}.jar"
    destination = BUILD / "libraries" / Path(path).name
    if not destination.exists():
        destination.parent.mkdir(parents=True, exist_ok=True)
        data = urllib.request.urlopen(MAVEN + path, timeout=120).read()
        expected = urllib.request.urlopen(MAVEN + path + ".sha1", timeout=60).read().decode().split()[0]
        if hashlib.sha1(data).hexdigest() != expected:
            raise ValueError(f"{coordinate}: checksum differs from the repository's")
        destination.write_bytes(data)
    return destination


def build():
    _, jars, libraries = base.verify_artifacts()
    classpath = [jars["server"], *libraries, *map(maven, COMPILE)]
    classes = BUILD / "classes"
    if classes.exists():
        for path in sorted(classes.rglob("*"), reverse=True):
            path.unlink() if path.is_file() else path.rmdir()
    classes.mkdir(parents=True, exist_ok=True)
    sources = sorted([*(SOURCE / "main/java").rglob("*.java"), *(SOURCE / "mc/java").rglob("*.java")])
    subprocess.run(["javac", "--release", "25", "-proc:none", "-encoding", "UTF-8", "-Xlint:all,-restricted,-processing,-classfile",
                    "-Werror", "-d", str(classes), "-classpath", os.pathsep.join(map(str, classpath)),
                    *map(str, sources)], check=True)
    entries = {path.relative_to(classes).as_posix(): path.read_bytes() for path in classes.rglob("*.class")}
    if any(not name.startswith("dev/horizonlab/") for name in entries):
        raise ValueError("Only original Horizon classes may be packaged")
    resources = SOURCE / "mc/resources"
    entries.update({path.relative_to(resources).as_posix(): path.read_bytes() for path in resources.rglob("*") if path.is_file()})
    entries["LICENSE"] = (ROOT / "LICENSE").read_bytes()
    version = json.loads((resources / "fabric.mod.json").read_text())["version"]
    output = ROOT / "dist" / f"horizon-accel-{version}.jar"
    output.parent.mkdir(exist_ok=True)
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for name in sorted(entries):
            info = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o100644 << 16
            archive.writestr(info, entries[name])
    subprocess.run(["cargo", "build", "--release", "-q", "-p", "horizon-accel"], check=True, cwd=ROOT)
    print(f"Built {output} ({output.stat().st_size} bytes, sha256 {lab.digest(output, 'sha256')})")
    return output


if __name__ == "__main__":
    build()
