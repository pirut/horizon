#!/usr/bin/env python3
"""Compare two saved worlds block for block: did horizon-accel change anything a player could see?

For every chunk both worlds saved as fully generated (status minecraft:full), compares each
section's blocks and biomes after decoding them (palette order and bit packing may differ while
the blocks are the same), and the block entities' ids and positions. Prints the first differences
and a summary; exits 1 if any chunk differs.

usage: accel_world_diff.py WORLD_A WORLD_B [--dimension dimensions/minecraft/overworld/region]
"""
import argparse
from pathlib import Path
import sys

try:
    from . import anvil_dump
except ImportError:
    import anvil_dump


def unpack(container, size):
    """A paletted container as `size` palette entries, in index order."""
    palette = container["palette"]
    if "data" not in container:
        return [palette[0]] * size
    words = container["data"][1]
    bits = max(1, (len(palette) - 1).bit_length())
    if size == 4096:
        bits = max(4, bits)
    per_word = 64 // bits
    mask = (1 << bits) - 1
    out = []
    for i in range(size):
        word = words[i // per_word] & 0xFFFFFFFFFFFFFFFF
        out.append(palette[(word >> (i % per_word * bits)) & mask])
    return out


def state_text(state):
    """A palette entry as text. 26.x writes a state without properties as its name, one with them as
    {id, properties}, and a list mixing both wraps the plain names as {"": name}."""
    if isinstance(state, dict) and set(state) == {""}:
        state = state[""]
    if isinstance(state, str):
        return state
    properties = state.get("properties", state.get("Properties", {}))
    name = state.get("id", state.get("Name"))
    return name + ("[" + ",".join(f"{k}={v}" for k, v in sorted(properties.items())) + "]" if properties else "")


def chunk_view(chunk):
    sections = {}
    for section in chunk.get("sections", []):
        y = section["Y"][1] if isinstance(section["Y"], tuple) else section["Y"]
        blocks = [state_text(s) for s in unpack(section["block_states"], 4096)] if "block_states" in section else None
        biomes = unpack(section["biomes"], 64) if "biomes" in section else None
        sections[y] = (blocks, biomes)
    entities = sorted((e.get("id"), e.get("x"), e.get("y"), e.get("z")) for e in chunk.get("block_entities", []))
    return sections, entities


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("a", type=Path)
    parser.add_argument("b", type=Path)
    parser.add_argument("--dimension", default="dimensions/minecraft/overworld/region")
    parser.add_argument("--show", type=int, default=10)
    options = parser.parse_args()
    compared = differ = 0
    shown = 0
    for region in sorted((options.a / options.dimension).glob("r.*.mca")):
        other = options.b / options.dimension / region.name
        if not other.exists():
            continue
        a, b = anvil_dump.region_chunks(region), anvil_dump.region_chunks(other)
        for key in sorted(a.keys() & b.keys()):
            if a[key].get("Status") != "minecraft:full" or b[key].get("Status") != "minecraft:full":
                continue
            compared += 1
            (sa, ea), (sb, eb) = chunk_view(a[key]), chunk_view(b[key])
            problems = []
            for y in sorted(sa.keys() | sb.keys()):
                ba, bb = sa.get(y, (None, None)), sb.get(y, (None, None))
                if ba[0] != bb[0]:
                    count = sum(x != y2 for x, y2 in zip(ba[0] or [], bb[0] or [])) if ba[0] and bb[0] else 4096
                    first = next((i for i, (x, y2) in enumerate(zip(ba[0] or [], bb[0] or [])) if x != y2), 0)
                    sample = f"{ba[0][first]} vs {bb[0][first]}" if ba[0] and bb[0] else "missing"
                    problems.append(f"section {y}: {count} blocks, first at index {first}: {sample}")
                if ba[1] != bb[1]:
                    problems.append(f"section {y}: biomes")
            if ea != eb:
                problems.append(f"block entities {len(ea)} vs {len(eb)}")
            if problems:
                differ += 1
                if shown < options.show:
                    shown += 1
                    x = a[key].get("xPos"), a[key].get("zPos")
                    print(f"chunk {x}: " + "; ".join(problems[:4]))
    print(f"{compared - differ}/{compared} full chunks identical")
    return 1 if differ or not compared else 0


if __name__ == "__main__":
    sys.exit(main())
