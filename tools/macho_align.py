#!/usr/bin/env python3
"""Move a thin 64-bit Mach-O's symbol string pool to an 8-byte boundary and refresh its ad-hoc code signature.

zig's Mach-O linker (cargo-zigbuild) can place the string pool right after the 4-byte indirect symbol table, at an
offset that is only 4-byte aligned. macOS 27's dyld refuses such a library ("mis-aligned LINKEDIT string pool"),
so build_accel_mod.py passes each cross-built dylib through align_string_pool() before packaging it. The pool moves
into the padding before the code signature (the signature itself does not move), and the signature's page hashes
are recomputed: zig signs ad hoc (linker-signed), so the hashes are all there is to it.

usage: macho_align.py LIBRARY [--check]
"""
import argparse
import hashlib
from pathlib import Path
import struct

MH_MAGIC_64 = 0xFEEDFACF
LC_SYMTAB = 0x2
LC_CODE_SIGNATURE = 0x1D
CSMAGIC_EMBEDDED_SIGNATURE = 0xFADE0CC0
CSMAGIC_CODEDIRECTORY = 0xFADE0C02
CS_HASHTYPE_SHA256 = 2


def _commands(data):
    """{command: offset of its load command} for the commands this module touches."""
    magic, _, _, _, ncmds = struct.unpack_from("<IiiII", data, 0)
    if magic != MH_MAGIC_64:
        raise ValueError("not a thin little-endian 64-bit Mach-O")
    found, offset = {}, 32
    for _ in range(ncmds):
        command, size = struct.unpack_from("<II", data, offset)
        found.setdefault(command, offset)
        offset += size
    return found


def misaligned(data):
    """Whether the string pool's file offset is not a multiple of 8."""
    symtab = _commands(data).get(LC_SYMTAB)
    return symtab is not None and struct.unpack_from("<I", data, symtab + 16)[0] % 8 != 0


def resign(data):
    """Recompute the SHA-256 page hashes of every ad-hoc CodeDirectory in the embedded signature, in place."""
    signature = _commands(data).get(LC_CODE_SIGNATURE)
    if signature is None:
        return
    start = struct.unpack_from("<I", data, signature + 8)[0]
    magic, _, count = struct.unpack_from(">III", data, start)
    if magic != CSMAGIC_EMBEDDED_SIGNATURE:
        raise ValueError("unknown code signature format")
    for index in range(count):
        _, blob = struct.unpack_from(">II", data, start + 12 + 8 * index)
        directory = start + blob
        if struct.unpack_from(">I", data, directory)[0] != CSMAGIC_CODEDIRECTORY:
            continue
        (hash_offset, _, _, code_slots, code_limit, hash_size, hash_type, _,
         page_shift) = struct.unpack_from(">IIIIIBBBB", data, directory + 16)
        if hash_type != CS_HASHTYPE_SHA256 or hash_size != 32:
            raise ValueError("only SHA-256 code directories are supported")
        page = 1 << page_shift
        for slot in range(code_slots):
            digest = hashlib.sha256(data[slot * page:min((slot + 1) * page, code_limit)]).digest()
            at = directory + hash_offset + slot * hash_size
            data[at:at + hash_size] = digest


def align_string_pool(library):
    """The library with its string pool on an 8-byte boundary (unchanged if it already is)."""
    data = bytearray(library)
    if not misaligned(data):
        return bytes(data)
    commands = _commands(data)
    symtab = commands[LC_SYMTAB]
    offset, size = struct.unpack_from("<II", data, symtab + 16)
    moved = (offset + 7) & ~7
    limit = len(data)
    if LC_CODE_SIGNATURE in commands:
        limit = struct.unpack_from("<I", data, commands[LC_CODE_SIGNATURE] + 8)[0]
    if moved + size > limit:
        raise ValueError("no padding before the code signature to align the string pool into")
    data[moved:moved + size] = bytes(data[offset:offset + size])
    data[offset:moved] = bytes(moved - offset)
    struct.pack_into("<I", data, symtab + 16, moved)
    resign(data)
    return bytes(data)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("library", type=Path)
    parser.add_argument("--check", action="store_true", help="only report whether the string pool is misaligned")
    args = parser.parse_args()
    data = args.library.read_bytes()
    if args.check:
        print("misaligned" if misaligned(data) else "aligned")
        return 1 if misaligned(data) else 0
    args.library.write_bytes(align_string_pool(data))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
