import importlib.util
import struct
import tempfile
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "verify_release_package", ROOT / "scripts" / "verify-release-package.py"
)
VERIFY = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(VERIFY)


class ReleaseArchitectureTests(unittest.TestCase):
    def test_elf_x86_64_header(self):
        data = bytearray(64)
        data[:4] = b"\x7fELF"
        data[4] = 2
        data[5] = 1
        struct.pack_into("<H", data, 18, 62)
        self.assertEqual(VERIFY.inspect_elf(bytes(data)), "x86_64-elf")

    def test_pe_x86_64_pe32_plus_header(self):
        data = bytearray(256)
        data[:2] = b"MZ"
        struct.pack_into("<I", data, 0x3C, 0x80)
        data[0x80:0x84] = b"PE\0\0"
        struct.pack_into("<H", data, 0x84, 0x8664)
        struct.pack_into("<H", data, 0x98, 0x20B)
        self.assertEqual(VERIFY.inspect_pe(bytes(data)), "x86_64-pe")

    def test_macho_arm64_header(self):
        data = bytearray(32)
        data[:4] = b"\xcf\xfa\xed\xfe"
        struct.pack_into("<I", data, 4, 0x0100000C)
        self.assertEqual(VERIFY.inspect_macho(bytes(data)), "arm64-macho")

    def test_macho_fat_binary_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "fat/universal"):
            VERIFY.inspect_macho(b"\xca\xfe\xba\xbe" + b"\0" * 28)

    def test_metadata_requires_unsigned_state(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "RELEASE_METADATA.json"
            path.write_text(
                """{
  "format": 1,
  "name": "inspirum-terminal",
  "version": "0.1.0",
  "tag": "v0.1.0",
  "target": "x86_64-unknown-linux-gnu",
  "commit": "0123456789012345678901234567890123456789",
  "release_channel": "unsigned-prerelease",
  "developer_signed": true,
  "notarized": false
}
""",
                encoding="utf-8",
            )
            with self.assertRaisesRegex(SystemExit, "developer_signed"):
                VERIFY.verify_metadata(
                    path,
                    target="x86_64-unknown-linux-gnu",
                    tag="v0.1.0",
                    commit="0123456789012345678901234567890123456789",
                    version="0.1.0",
                )


if __name__ == "__main__":
    unittest.main()
