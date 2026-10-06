#!/usr/bin/env python3
"""Verify a downloaded Inspirum Terminal release archive on its native runner.

This intentionally operates on the archive produced and re-downloaded through
GitHub Actions. It does not inspect the build directory.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import shutil
import struct
import subprocess
import tarfile
import tempfile
import zipfile


TARGET_ARCH = {
    "x86_64-unknown-linux-gnu": ("elf", "x86_64"),
    "x86_64-pc-windows-msvc": ("pe", "x86_64"),
    "aarch64-apple-darwin": ("macho", "arm64"),
}


def fail(message: str) -> "NoReturn":
    raise SystemExit(f"error: {message}")


def safe_relative(name: str) -> Path:
    normalized = name.replace("\\", "/")
    path = Path(normalized)
    if path.is_absolute() or ".." in path.parts or not path.parts:
        fail(f"unsafe archive member path: {name!r}")
    return path


def extract_archive(archive: Path, destination: Path) -> None:
    if archive.name.endswith(".tar.gz"):
        with tarfile.open(archive, "r:gz") as bundle:
            members = bundle.getmembers()
            for member in members:
                relative = safe_relative(member.name)
                if member.issym() or member.islnk():
                    fail(f"release archive contains a link: {member.name!r}")
                target = (destination / relative).resolve()
                try:
                    target.relative_to(destination.resolve())
                except ValueError:
                    fail(f"release archive member escapes extraction root: {member.name!r}")
            bundle.extractall(destination, members=members, filter="data")
        return

    if archive.suffix.lower() == ".zip":
        with zipfile.ZipFile(archive) as bundle:
            for info in bundle.infolist():
                relative = safe_relative(info.filename)
                target = (destination / relative).resolve()
                try:
                    target.relative_to(destination.resolve())
                except ValueError:
                    fail(f"release archive member escapes extraction root: {info.filename!r}")
            bundle.extractall(destination)
        return

    fail(f"unsupported release archive format: {archive.name}")


def architecture(binary: Path) -> tuple[str, str]:
    data = binary.read_bytes()
    if len(data) < 64:
        fail(f"release executable is too short to identify: {binary}")

    if data[:4] == b"\x7fELF":
        if data[4] != 2:
            fail("ELF executable is not 64-bit")
        if data[5] != 1:
            fail("ELF executable is not little-endian")
        machine = struct.unpack_from("<H", data, 18)[0]
        if machine != 62:
            fail(f"unexpected ELF machine value: {machine}")
        return ("elf", "x86_64")

    if data[:2] == b"MZ":
        pe_offset = struct.unpack_from("<I", data, 0x3C)[0]
        if pe_offset + 6 > len(data) or data[pe_offset:pe_offset + 4] != b"PE\x00\x00":
            fail("invalid PE executable header")
        machine = struct.unpack_from("<H", data, pe_offset + 4)[0]
        if machine != 0x8664:
            fail(f"unexpected PE machine value: 0x{machine:04x}")
        return ("pe", "x86_64")

    # Thin little-endian Mach-O 64-bit. Reject universal/fat files here: the
    # Apple Silicon release contract is a native arm64 binary, not a target
    # name inferred from CI.
    if data[:4] == bytes.fromhex("cffaedfe"):
        cpu_type = struct.unpack_from("<I", data, 4)[0]
        if cpu_type != 0x0100000C:
            fail(f"unexpected Mach-O CPU type: 0x{cpu_type:08x}")
        return ("macho", "arm64")

    fail("unrecognized executable format")


def read_metadata(root: Path) -> dict:
    path = root / "RELEASE-METADATA.json"
    try:
        metadata = json.loads(path.read_text(encoding="utf-8"))
    except OSError as error:
        fail(f"release metadata is missing: {error}")
    except json.JSONDecodeError as error:
        fail(f"release metadata is invalid JSON: {error}")
    return metadata


def verify_archive(archive: Path, target: str, version: str, tag: str) -> None:
    expected_arch = TARGET_ARCH.get(target)
    if expected_arch is None:
        fail(f"unsupported release target: {target}")

    expected_root = f"inspirum-terminal-{tag}-{target}"
    with tempfile.TemporaryDirectory(prefix="inspirum-release-verify-") as tmp:
        extraction = Path(tmp)
        extract_archive(archive, extraction)
        entries = [entry for entry in extraction.iterdir()]
        if [entry.name for entry in entries] != [expected_root] or not entries[0].is_dir():
            fail(
                f"archive must contain exactly one top-level directory {expected_root!r}; "
                f"found {[entry.name for entry in entries]!r}"
            )
        root = entries[0]
        executable = root / (
            "inspirum-terminal.exe" if target == "x86_64-pc-windows-msvc"
            else "inspirum-terminal"
        )
        if not executable.is_file():
            fail(f"release executable is missing: {executable}")

        for required in ["LICENSE", "README.md", "THIRD_PARTY_NOTICES"]:
            path = root / required
            if not path.exists():
                fail(f"required release content is missing: {required}")

        metadata = read_metadata(root)
        expected_metadata = {
            "package": "inspirum-terminal",
            "tag": tag,
            "target": target,
            "version": version,
            "signed": False,
            "signing_status": "unsigned",
        }
        for key, value in expected_metadata.items():
            if metadata.get(key) != value:
                fail(
                    f"release metadata mismatch for {key}: "
                    f"expected {value!r}, got {metadata.get(key)!r}"
                )

        actual_arch = architecture(executable)
        if actual_arch != expected_arch:
            fail(
                f"release architecture mismatch: expected {expected_arch}, got {actual_arch}"
            )

        result = subprocess.run(
            [str(executable), "--version"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=20,
            check=False,
        )
        expected_version = f"inspirum-terminal {version}"
        if result.returncode != 0:
            fail(
                f"downloaded executable --version failed with {result.returncode}: "
                f"{result.stderr.strip()}"
            )
        if result.stdout.strip() != expected_version:
            fail(
                f"downloaded executable version mismatch: "
                f"expected {expected_version!r}, got {result.stdout.strip()!r}"
            )

        print(
            "PASS downloaded native release: "
            f"target={target} format={actual_arch[0]} arch={actual_arch[1]} "
            f"version={version} signing=unsigned"
        )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--archive", required=True, type=Path)
    parser.add_argument("--target", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--tag", required=True)
    args = parser.parse_args(argv)

    if not args.archive.is_file():
        fail(f"release archive does not exist: {args.archive}")
    verify_archive(args.archive.resolve(), args.target, args.version, args.tag)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
