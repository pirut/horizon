#!/usr/bin/env python3
"""Build the horizon-accel Fabric mod (mods/horizon-accel) and its native library.

Compiles only original dev/horizonlab sources against the pinned 26.3 server jar, Fabric Loader
and Mixin, and never bundles game classes. Output: dist/horizon-accel-<version>.jar, plus the
release build of crates/horizon-accel (target/release/libhorizon_accel.*). The jar carries that library
under natives/<os>-<arch>/ and unpacks it at startup (-Dhorizon.accel.library overrides it); --targets also
cross-builds it for other platforms with cargo-zigbuild (`pip install ziglang`, `cargo install cargo-zigbuild`,
`rustup target add <target>`) and carries those too, so one jar runs on each.

usage: build_accel_mod.py [--targets aarch64-apple-darwin,x86_64-apple-darwin,...]
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
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
# Compile-time only: the Fabric Loader, Mixin and ASM a 26.3 Fabric install runs.
COMPILE = ["net.fabricmc:fabric-loader:0.20.1", "net.fabricmc:sponge-mixin:0.17.5+mixin.0.8.7",
           "org.ow2.asm:asm:9.10.1"]


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


# Rust target -> the jar's natives/ folder (Natives.platform()) and the library's file name there.
TARGETS = {
    "aarch64-apple-darwin": ("macos-aarch64", "libhorizon_accel.dylib"),
    "x86_64-apple-darwin": ("macos-x86_64", "libhorizon_accel.dylib"),
    "x86_64-unknown-linux-gnu": ("linux-x86_64", "libhorizon_accel.so"),
    "aarch64-unknown-linux-gnu": ("linux-aarch64", "libhorizon_accel.so"),
    "x86_64-pc-windows-gnu": ("windows-x86_64", "horizon_accel.dll"),
}


def host_native():
    """This machine's natives/ folder and the library's file name, as Natives.platform() names them."""
    system = {"Darwin": "macos", "Windows": "windows"}.get(platform.system(), "linux")
    machine = {"amd64": "x86_64", "arm64": "aarch64"}.get(platform.machine().lower(), platform.machine().lower())
    name = {"macos": "libhorizon_accel.dylib", "windows": "horizon_accel.dll"}.get(system, "libhorizon_accel.so")
    return f"{system}-{machine}", name


def natives(targets):
    """Builds the library for this machine and each cross target: {jar path: bytes}."""
    subprocess.run(["cargo", "build", "--release", "-q", "-p", "horizon-accel"], check=True, cwd=ROOT)
    folder, name = host_native()
    built = {f"natives/{folder}/{name}": (lab.cargo_target() / "release" / name).read_bytes()}
    for target in targets:
        folder, name = TARGETS[target]
        if f"natives/{folder}/{name}" in built:
            continue
        subprocess.run(["cargo", "zigbuild", "--release", "-q", "-p", "horizon-accel", "--target", target],
                       check=True, cwd=ROOT)
        built[f"natives/{folder}/{name}"] = (lab.cargo_target() / target / "release" / name).read_bytes()
    return built


def build(targets=()):
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
    entries.update(natives(targets))
    version = json.loads((resources / "fabric.mod.json").read_text())["version"]
    output = ROOT / "dist" / f"horizon-accel-{version}.jar"
    output.parent.mkdir(exist_ok=True)
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for name in sorted(entries):
            info = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o100644 << 16
            archive.writestr(info, entries[name])
    print(f"Built {output} ({output.stat().st_size} bytes, sha256 {lab.digest(output, 'sha256')})")
    return output


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--targets", default="", help="comma-separated Rust targets to cross-build: "
                        + ", ".join(TARGETS))
    options = parser.parse_args()
    targets = [t for t in options.targets.split(",") if t]
    unknown = [t for t in targets if t not in TARGETS]
    if unknown:
        parser.error(f"unknown targets: {', '.join(unknown)}")
    build(targets)
