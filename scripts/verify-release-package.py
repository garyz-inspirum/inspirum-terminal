#!/usr/bin/env python3
"""Verify a downloaded Inspirum native package from clean extracted bytes."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tarfile
import tempfile
import tomllib
import zipfile


PROJECT = "inspirum-terminal"
LINUX_TARGET = "x86_64-unknown-linux-gnu"
WINDOWS_TARGET = "x86_64-pc-windows-msvc"
MACOS_TARGET = "aarch64-apple-darwin"


def expected_version(root: Path) -> str:
    with (root / "Cargo.toml").open("rb") as stream:
        return str(tomllib.load(stream)["package"]["version"])


def expected_binary_name(target: str) -> str:
    return "inspirum-terminal.exe" if target == WINDOWS_TARGET else "inspirum-terminal"


def archive_from_directory(directory: Path, target: str) -> Path:
    suffix = ".zip" if target == WINDOWS_TARGET else ".tar.gz"
    candidates = sorted(path for path in directory.iterdir() if path.name.endswith(suffix))
    if len(candidates) != 1:
        raise SystemExit(
            f"expected exactly one {suffix} archive in {directory}, found {len(candidates)}"
        )
    return candidates[0]


def safe_destination(root: Path, member: str) -> Path:
    candidate = (root / member).resolve()
    try:
        candidate.relative_to(root.resolve())
    except ValueError as error:
        raise SystemExit(f"archive member escapes extraction directory: {member}") from error
    return candidate


def extract_archive(archive: Path, destination: Path) -> None:
    if archive.name.endswith(".zip"):
        with zipfile.ZipFile(archive) as bundle:
            for info in bundle.infolist():
                safe_destination(destination, info.filename)
                mode = (info.external_attr >> 16) & 0o170000
                if mode == 0o120000:
                    raise SystemExit(f"archive contains symlink: {info.filename}")
            bundle.extractall(destination)
        return

    if archive.name.endswith(".tar.gz"):
        with tarfile.open(archive, "r:gz") as bundle:
            for member in bundle.getmembers():
                safe_destination(destination, member.name)
                if not (member.isdir() or member.isfile()):
                    raise SystemExit(f"archive contains unsupported link/device entry: {member.name}")
            bundle.extractall(destination)
        return

    raise SystemExit(f"unsupported release archive: {archive}")


def inspect_elf(data: bytes) -> str:
    if len(data) < 20 or data[:4] != b"\x7fELF":
        raise ValueError("not ELF")
    if data[4] != 2:
        raise ValueError("ELF binary is not 64-bit")
    endian = {1: "<", 2: ">"}.get(data[5])
    if endian is None:
        raise ValueError("ELF binary has invalid endianness")
    machine = struct.unpack_from(endian + "H", data, 18)[0]
    if machine != 62:
        raise ValueError(f"ELF machine is {machine:#x}, expected x86_64 (0x3e)")
    return "x86_64-elf"


def inspect_pe(data: bytes) -> str:
    if len(data) < 0x40 or data[:2] != b"MZ":
        raise ValueError("not PE")
    pe_offset = struct.unpack_from("<I", data, 0x3C)[0]
    if pe_offset + 26 > len(data) or data[pe_offset : pe_offset + 4] != b"PE\0\0":
        raise ValueError("invalid PE signature")
    machine = struct.unpack_from("<H", data, pe_offset + 4)[0]
    if machine != 0x8664:
        raise ValueError(f"PE machine is {machine:#x}, expected AMD64 (0x8664)")
    optional_magic = struct.unpack_from("<H", data, pe_offset + 24)[0]
    if optional_magic != 0x20B:
        raise ValueError(f"PE optional header is {optional_magic:#x}, expected PE32+ (0x20b)")
    return "x86_64-pe"


def inspect_macho(data: bytes) -> str:
    if len(data) < 12:
        raise ValueError("Mach-O binary is too short")
    magic_bytes = data[:4]
    if magic_bytes == b"\xcf\xfa\xed\xfe":
        endian = "<"
    elif magic_bytes == b"\xfe\xed\xfa\xcf":
        endian = ">"
    elif magic_bytes in {b"\xca\xfe\xba\xbe", b"\xbe\xba\xfe\xca"}:
        raise ValueError("fat/universal Mach-O is not a thin Apple Silicon build")
    else:
        raise ValueError("not 64-bit Mach-O")
    cpu_type = struct.unpack_from(endian + "I", data, 4)[0]
    if cpu_type != 0x0100000C:
        raise ValueError(
            f"Mach-O CPU type is {cpu_type:#x}, expected ARM64 (0x0100000c)"
        )
    return "arm64-macho"


def inspect_architecture(binary: Path, target: str) -> str:
    data = binary.read_bytes()
    try:
        if target == LINUX_TARGET:
            return inspect_elf(data)
        if target == WINDOWS_TARGET:
            return inspect_pe(data)
        if target == MACOS_TARGET:
            return inspect_macho(data)
    except ValueError as error:
        raise SystemExit(f"architecture verification failed for {binary}: {error}") from error
    raise SystemExit(f"unsupported target for architecture verification: {target}")


def verify_metadata(
    metadata_path: Path, *, target: str, tag: str, commit: str, version: str
) -> dict:
    try:
        metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise SystemExit(f"cannot read release metadata: {error}") from error

    expected = {
        "format": 1,
        "name": PROJECT,
        "version": version,
        "tag": tag,
        "target": target,
        "commit": commit.lower(),
        "release_channel": "unsigned-prerelease",
        "developer_signed": False,
        "notarized": False,
    }
    for key, value in expected.items():
        if metadata.get(key) != value:
            raise SystemExit(
                f"release metadata mismatch for {key}: {metadata.get(key)!r} != {value!r}"
            )
    return metadata


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser()
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--archive", type=Path)
    source.add_argument("--archive-dir", type=Path)
    parser.add_argument("--target", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--work-parent", type=Path, required=True)
    args = parser.parse_args()

    root = Path(__file__).resolve().parents[1]
    version = expected_version(root)
    archive = (
        args.archive.resolve()
        if args.archive
        else archive_from_directory(args.archive_dir.resolve(), args.target)
    )
    if not archive.is_file():
        raise SystemExit(f"release archive does not exist: {archive}")

    work_parent = args.work_parent.resolve()
    work_parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="clean-install-", dir=work_parent) as temporary:
        extraction = Path(temporary)
        extract_archive(archive, extraction)

        package_name = f"{PROJECT}-{args.tag}-{args.target}"
        package_root = extraction / package_name
        if not package_root.is_dir():
            raise SystemExit(f"expected package root is missing: {package_root}")

        binary = package_root / expected_binary_name(args.target)
        if not binary.is_file():
            raise SystemExit(f"packaged executable is missing: {binary}")
        for required in ("LICENSE", "README.md", "THIRD_PARTY_NOTICES", "RELEASE_METADATA.json"):
            if not (package_root / required).exists():
                raise SystemExit(f"packaged release component is missing: {required}")

        metadata = verify_metadata(
            package_root / "RELEASE_METADATA.json",
            target=args.target,
            tag=args.tag,
            commit=args.commit,
            version=version,
        )
        architecture = inspect_architecture(binary, args.target)

        completed = subprocess.run(
            [str(binary), "--version"],
            cwd=package_root,
            env=dict(os.environ),
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=20,
        )
        if completed.returncode != 0:
            raise SystemExit(
                f"downloaded executable failed with {completed.returncode}: {completed.stderr}"
            )
        expected_output = f"{PROJECT} {version}"
        if completed.stdout.strip() != expected_output:
            raise SystemExit(
                f"downloaded executable version mismatch: {completed.stdout.strip()!r}"
            )

        print(
            "PASS downloaded package "
            f"target={args.target} architecture={architecture} "
            f"version={version} commit={metadata['commit']} "
            f"developer_signed={metadata['developer_signed']} "
            f"notarized={metadata['notarized']} "
            f"sha256={sha256(archive)}"
        )

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
