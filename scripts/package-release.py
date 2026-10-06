#!/usr/bin/env python3
"""Create a native Inspirum release archive with explicit provenance metadata."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import shutil
import tarfile
import tempfile
import tomllib
import zipfile


PROJECT = "inspirum-terminal"
WINDOWS_TARGET = "x86_64-pc-windows-msvc"


def package_version(root: Path) -> str:
    with (root / "Cargo.toml").open("rb") as stream:
        return str(tomllib.load(stream)["package"]["version"])


def binary_name(target: str) -> str:
    return "inspirum-terminal.exe" if target == WINDOWS_TARGET else "inspirum-terminal"


def archive_suffix(target: str) -> str:
    return ".zip" if target == WINDOWS_TARGET else ".tar.gz"


def copy_tree(source: Path, destination: Path) -> None:
    if not source.is_dir():
        raise SystemExit(f"notice directory does not exist: {source}")
    shutil.copytree(source, destination)


def build_archive(
    *,
    root: Path,
    output_dir: Path,
    target: str,
    tag: str,
    commit: str,
    binary: Path,
    notices: Path,
) -> Path:
    version = package_version(root)
    expected_binary = binary_name(target)
    if not binary.is_file():
        raise SystemExit(f"release binary does not exist: {binary}")
    if len(commit) != 40 or any(ch not in "0123456789abcdefABCDEF" for ch in commit):
        raise SystemExit("commit must be a full 40-character hexadecimal Git SHA")

    package_name = f"{PROJECT}-{tag}-{target}"
    output_dir.mkdir(parents=True, exist_ok=True)
    archive = output_dir / f"{package_name}{archive_suffix(target)}"
    if archive.exists():
        raise SystemExit(f"refusing to overwrite existing archive: {archive}")

    with tempfile.TemporaryDirectory(prefix=".package-", dir=output_dir) as temporary:
        package_root = Path(temporary) / package_name
        package_root.mkdir()

        destination_binary = package_root / expected_binary
        shutil.copy2(binary, destination_binary)
        shutil.copy2(root / "LICENSE", package_root / "LICENSE")
        shutil.copy2(root / "README.md", package_root / "README.md")
        copy_tree(notices, package_root / "THIRD_PARTY_NOTICES")

        metadata = {
            "format": 1,
            "name": PROJECT,
            "version": version,
            "tag": tag,
            "target": target,
            "commit": commit.lower(),
            "release_channel": "unsigned-prerelease",
            "developer_signed": False,
            "notarized": False,
            "signature_policy": "No release signing identity or notarization credential was applied.",
        }
        (package_root / "RELEASE_METADATA.json").write_text(
            json.dumps(metadata, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )

        if target == WINDOWS_TARGET:
            with zipfile.ZipFile(archive, "x", compression=zipfile.ZIP_DEFLATED) as bundle:
                for path in sorted(package_root.rglob("*")):
                    bundle.write(path, path.relative_to(Path(temporary)))
        else:
            with tarfile.open(archive, "x:gz") as bundle:
                bundle.add(package_root, arcname=package_name, recursive=True)

    print(archive)
    return archive


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--notices", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()

    root = Path(__file__).resolve().parents[1]
    build_archive(
        root=root,
        output_dir=args.output_dir.resolve(),
        target=args.target,
        tag=args.tag,
        commit=args.commit,
        binary=args.binary.resolve(),
        notices=args.notices.resolve(),
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
