"""tools/macho_align.py: the string pool moves to an 8-byte boundary and the ad-hoc signature still matches."""

import hashlib
from pathlib import Path
import struct
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
import macho_align  # noqa: E402

PAGE_SHIFT = 12


def library(string_offset, pool=b"\0_horizon_accel_abi\0_horizon_accel_zlib\0", signature_offset=12288):
    """A minimal thin Mach-O: LC_SYMTAB and LC_CODE_SIGNATURE, a string pool, one ad-hoc CodeDirectory."""
    data = bytearray(signature_offset)
    struct.pack_into("<IiiIIII", data, 0, macho_align.MH_MAGIC_64, 0x0100000C, 0, 6, 2, 24 + 16, 0)
    struct.pack_into("<IIIIII", data, 32, macho_align.LC_SYMTAB, 24, 4096, 0, string_offset, len(pool))
    data[string_offset:string_offset + len(pool)] = pool
    slots = (signature_offset + (1 << PAGE_SHIFT) - 1) >> PAGE_SHIFT
    ident = b"libhorizon_accel.dylib\0"
    hash_offset = 44 + len(ident)
    directory = bytearray(hash_offset + 32 * slots)
    struct.pack_into(">IIIIIIIIIBBBB", directory, 0, macho_align.CSMAGIC_CODEDIRECTORY, len(directory), 0x20400,
                     0x20002, hash_offset, 44, 0, slots, signature_offset, 32, 2, 0, PAGE_SHIFT)
    directory[44:44 + len(ident)] = ident
    blob = bytearray(struct.pack(">IIIII", macho_align.CSMAGIC_EMBEDDED_SIGNATURE, 20 + len(directory), 1, 0, 20))
    blob += directory
    struct.pack_into("<IIII", data, 56, macho_align.LC_CODE_SIGNATURE, 16, signature_offset, len(blob))
    data += blob
    macho_align.resign(data)
    return bytes(data)


def hashes_match(data):
    copy = bytearray(data)
    macho_align.resign(copy)
    return bytes(copy) == data


class MachoAlignTests(unittest.TestCase):
    def test_moves_a_misaligned_pool_and_resigns(self):
        original = library(8196)
        self.assertTrue(macho_align.misaligned(original))
        fixed = macho_align.align_string_pool(original)
        self.assertFalse(macho_align.misaligned(fixed))
        self.assertEqual(len(fixed), len(original))
        offset, size = struct.unpack_from("<II", fixed, 32 + 16)
        self.assertEqual(offset, 8200)
        self.assertEqual(fixed[offset:offset + size], original[8196:8196 + size])
        self.assertTrue(hashes_match(fixed))
        self.assertNotEqual(hashlib.sha256(fixed[8192:12288]).digest(),
                            hashlib.sha256(original[8192:12288]).digest())

    def test_leaves_an_aligned_library_alone(self):
        original = library(8200)
        self.assertEqual(macho_align.align_string_pool(original), original)

    def test_refuses_when_there_is_no_room_before_the_signature(self):
        with self.assertRaises(ValueError):
            macho_align.align_string_pool(library(12288 - 36, signature_offset=12288))


if __name__ == "__main__":
    unittest.main()
