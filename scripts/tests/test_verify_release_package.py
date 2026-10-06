#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
from pathlib import Path
import struct
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "verify-release-package.py"
SPEC = importlib.util.spec_from_file_location("verify_release_package", SCRIPT)
assert SPEC and SPEC.loader
verifier = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(verifier)


class ReleaseVerifierTests(unittest.TestCase):
    def write(self, root: Path, name: str, data: bytes) -> Path:
        path = root / name
        path.write_bytes(data)
        return path

    def test_identifies_native_release_architectures_from_binary_headers(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)

            elf = bytearray(64)
            elf[:4] = b"\x7fELF"
            elf[4] = 2
            elf[5] = 1
            struct.pack_into("<H", elf, 18, 62)
            self.assertEqual(
                verifier.architecture(self.write(root, "linux", bytes(elf))),
                ("elf", "x86_64"),
            )

            pe = bytearray(128)
            pe[:2] = b"MZ"
            struct.pack_into("<I", pe, 0x3C, 64)
            pe[64:68] = b"PE\x00\x00"
            struct.pack_into("<H", pe, 68, 0x8664)
            self.assertEqual(
                verifier.architecture(self.write(root, "windows.exe", bytes(pe))),
                ("pe", "x86_64"),
            )

            macho = bytearray(64)
            macho[:4] = bytes.fromhex("cffaedfe")
            struct.pack_into("<I", macho, 4, 0x0100000C)
            self.assertEqual(
                verifier.architecture(self.write(root, "macos", bytes(macho))),
                ("macho", "arm64"),
            )

    def test_wrong_architecture_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            elf = bytearray(64)
            elf[:4] = b"\x7fELF"
            elf[4] = 2
            elf[5] = 1
            struct.pack_into("<H", elf, 18, 183)  # AArch64, not Linux x64 release contract.
            path = self.write(root, "wrong", bytes(elf))
            with self.assertRaisesRegex(SystemExit, "unexpected ELF machine"):
                verifier.architecture(path)

    def test_archive_paths_reject_traversal_and_absolute_members(self) -> None:
        with self.assertRaisesRegex(SystemExit, "unsafe archive member"):
            verifier.safe_relative("../escape")
        with self.assertRaisesRegex(SystemExit, "unsafe archive member"):
            verifier.safe_relative("/absolute")
        self.assertEqual(
            verifier.safe_relative("inspirum-terminal-v0.1.0-target/LICENSE"),
            Path("inspirum-terminal-v0.1.0-target/LICENSE"),
        )


if __name__ == "__main__":
    unittest.main()
