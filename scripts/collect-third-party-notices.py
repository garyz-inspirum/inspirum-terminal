#!/usr/bin/env python3
"""Collect deterministic third-party license material for one Rust target."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
from typing import NoReturn

SAFE_COMPONENT = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.+~-]*$")
TREE_PACKAGE = re.compile(r"^(?P<name>.+) v(?P<version>[^ ]+?)(?: \((?P<qualifier>.+)\))?$")
NOTICE_PREFIXES = ("license", "licence", "copying", "notice", "copyright", "unlicense")
PROVENANCE_FILES = {"inspirum_patches.md", "upstream.md"}
DEFAULT_SUPPLEMENTS = Path(__file__).resolve().parents[1] / "third-party-licenses" / "manifest.json"
CANONICAL_STANDARD_SOURCES = {
    "CC0-1.0": "https://creativecommons.org/publicdomain/zero/1.0/legalcode.txt",
    "MIT": "https://opensource.org/license/mit",
}


def fail(message: str) -> NoReturn:
    raise SystemExit(f"error: {message}")


def is_notice_file(relative: Path) -> bool:
    lower_name = relative.name.lower()
    if lower_name.startswith(NOTICE_PREFIXES) or lower_name in PROVENANCE_FILES:
        return True
    # epaint_default_fonts ships Hack, OFL, and Ubuntu font terms as .txt files.
    return relative.suffix.lower() == ".txt" and any(
        part.lower() in {"font", "fonts"} for part in relative.parts[:-1]
    )


def confined_file(package_dir: Path, candidate: Path) -> bool:
    try:
        candidate.resolve(strict=True).relative_to(package_dir.resolve(strict=True))
    except (OSError, ValueError):
        return False
    return candidate.is_file()


def cargo_output(command: list[str], description: str) -> str:
    try:
        return subprocess.check_output(command, text=True, stderr=subprocess.PIPE)
    except subprocess.CalledProcessError as error:
        detail = (error.stderr or "").strip()
        suffix = f": {detail}" if detail else ""
        fail(f"{description} failed with exit status {error.returncode}{suffix}")
    except OSError as error:
        fail(f"cannot run {description}: {error}")


def active_package_ids(metadata: dict, tree_output: str) -> set[str]:
    """Map Cargo's active normal/build tree back to exact metadata package IDs."""
    packages = metadata.get("packages")
    if not isinstance(packages, list):
        fail("cargo metadata did not return a package list")

    by_name_version: dict[tuple[str, str], list[dict]] = {}
    for package in packages:
        try:
            key = (package["name"], package["version"])
        except (KeyError, TypeError):
            fail("cargo metadata returned a package without a name or version")
        by_name_version.setdefault(key, []).append(package)

    active: set[str] = set()
    for rendered in tree_output.splitlines():
        rendered = rendered.strip()
        if not rendered:
            continue
        if rendered.endswith(" (proc-macro)"):
            rendered = rendered.removesuffix(" (proc-macro)")
        match = TREE_PACKAGE.fullmatch(rendered)
        if match is None:
            fail(f"cannot parse cargo tree package identity: {rendered!r}")
        key = (match.group("name"), match.group("version"))
        candidates = by_name_version.get(key, [])
        qualifier = match.group("qualifier")

        # Cargo renders path packages with their canonical package directory.
        # Use that qualifier when present. Other source kinds remain fail-closed
        # if name and version do not identify exactly one metadata package.
        if qualifier and Path(qualifier).is_absolute():
            qualified = [
                package for package in candidates
                if package.get("source") is None
                and Path(package["manifest_path"]).parent.resolve() == Path(qualifier).resolve()
            ]
            candidates = qualified
        if not candidates:
            fail(f"cargo tree package is missing from metadata: {rendered}")
        if len(candidates) != 1:
            ids = ", ".join(sorted(str(package.get("id")) for package in candidates))
            fail(f"cargo tree package maps ambiguously in metadata: {rendered}: {ids}")
        package_id = candidates[0].get("id")
        if not package_id:
            fail(f"cargo metadata package has no id: {rendered}")
        active.add(package_id)
    if not active:
        fail("cargo tree returned no active normal/build packages")
    return active


def package_component(name: str, version: str) -> str:
    component = f"{name}-{version}"
    if not SAFE_COMPONENT.fullmatch(component) or component in {".", ".."}:
        fail(f"unsafe package name/version cannot be used as a path: {name} {version}")
    return component


def load_supplements(manifest_path: Path) -> tuple[dict[tuple[str, str], dict], dict[str, dict]]:
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except OSError as error:
        fail(f"cannot read supplemental license manifest {manifest_path}: {error}")
    except json.JSONDecodeError as error:
        fail(f"invalid supplemental license manifest {manifest_path}: {error}")
    if manifest.get("format") != 1:
        fail(f"unsupported supplemental license manifest format in {manifest_path}")

    root = manifest_path.parent.resolve(strict=True)
    files: dict[str, dict] = {}
    for entry in manifest.get("files", []):
        file_id = entry.get("id")
        relative = Path(entry.get("path", ""))
        revision = entry.get("revision", "")
        source_url = entry.get("source_url", "")
        origin = entry.get("origin", "upstream_revision")
        if not file_id or file_id in files:
            fail(f"duplicate or empty supplemental file id: {file_id!r}")
        if relative.is_absolute() or ".." in relative.parts or not relative.parts:
            fail(f"unsafe supplemental license path for {file_id}: {relative}")
        source = root / relative
        if not confined_file(root, source):
            fail(f"supplemental license file is missing or escapes its directory: {relative}")
        data = source.read_bytes()
        actual_hash = hashlib.sha256(data).hexdigest()
        if actual_hash != entry.get("sha256"):
            fail(f"supplemental license hash mismatch for {file_id}: expected {entry.get('sha256')}, got {actual_hash}")
        if origin == "upstream_revision":
            if not revision or revision not in source_url:
                fail(f"supplemental source URL is not pinned to revision {revision!r} for {file_id}")
        elif origin == "canonical_standard_text":
            standard = entry.get("standard")
            if source_url != CANONICAL_STANDARD_SOURCES.get(standard):
                fail(f"unrecognized canonical standard source for {file_id}: {standard!r} {source_url!r}")
            if revision:
                fail(f"canonical standard text must not be labeled as an upstream revision for {file_id}")
        else:
            fail(f"unrecognized supplemental source origin for {file_id}: {origin!r}")
        files[file_id] = entry

    packages: dict[tuple[str, str], dict] = {}
    for entry in manifest.get("packages", []):
        key = (entry.get("name", ""), entry.get("version", ""))
        if not all(key) or key in packages:
            fail(f"duplicate or incomplete supplemental package entry: {key}")
        refs = entry.get("files", [])
        if not refs:
            fail(f"supplemental package has no source text: {key[0]} {key[1]}")
        for file_id in refs:
            source = files.get(file_id)
            if source is None:
                fail(f"supplemental package {key[0]} {key[1]} references unknown file {file_id}")
            if source.get("origin", "upstream_revision") == "upstream_revision" and source["revision"] != entry.get("revision"):
                fail(f"supplemental revision mismatch for {key[0]} {key[1]} and {file_id}")
            if source.get("origin") == "canonical_standard_text" and source.get("standard") != entry.get("declared_license"):
                fail(f"canonical standard does not match declared license for {key[0]} {key[1]}")
        if entry.get("declared_license"):
            relative = Path(entry.get("declaration_path", ""))
            if relative.is_absolute() or ".." in relative.parts or not relative.parts:
                fail(f"unsafe declaration evidence path for {key[0]} {key[1]}")
            declaration = root / relative
            if not confined_file(root, declaration):
                fail(f"declaration evidence is missing or escapes its directory for {key[0]} {key[1]}")
            actual_hash = hashlib.sha256(declaration.read_bytes()).hexdigest()
            if actual_hash != entry.get("declaration_sha256"):
                fail(f"declaration evidence hash mismatch for {key[0]} {key[1]}")
            if entry.get("revision", "") not in entry.get("declaration_url", ""):
                fail(f"declaration evidence URL is not revision-pinned for {key[0]} {key[1]}")
        packages[key] = entry
    return packages, files


def package_vcs_revision(package_dir: Path) -> str | None:
    vcs_path = package_dir / ".cargo_vcs_info.json"
    if not vcs_path.is_file():
        return None
    try:
        return json.loads(vcs_path.read_text(encoding="utf-8"))["git"]["sha1"]
    except (OSError, KeyError, TypeError, json.JSONDecodeError) as error:
        fail(f"invalid registry provenance file {vcs_path}: {error}")


def publish_directory(staging: Path, output: Path) -> None:
    """Publish without ever replacing an existing output directory."""
    try:
        output.mkdir()
    except FileExistsError:
        fail(f"output already exists (refusing to replace it): {output}")
    try:
        for child in sorted(staging.iterdir(), key=lambda path: path.name):
            os.rename(child, output / child.name)
        staging.rmdir()
    except BaseException:
        shutil.rmtree(output, ignore_errors=True)
        raise


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True, help="Rust target triple")
    parser.add_argument("--output", required=True, type=Path, help="new generated directory")
    parser.add_argument("--supplements", type=Path, default=DEFAULT_SUPPLEMENTS)
    args = parser.parse_args(argv)

    if not SAFE_COMPONENT.fullmatch(args.target):
        fail(f"unsafe target triple: {args.target!r}")
    output = args.output.absolute()
    if output.exists() or output.is_symlink():
        fail(f"output already exists (refusing to replace it): {output}")
    output.parent.mkdir(parents=True, exist_ok=True)
    supplements, supplemental_files = load_supplements(args.supplements.absolute())

    metadata_command = [
        "cargo", "metadata", "--locked", "--format-version", "1",
        "--filter-platform", args.target,
    ]
    try:
        metadata = json.loads(cargo_output(metadata_command, "cargo metadata"))
    except json.JSONDecodeError as error:
        fail(f"cargo metadata returned invalid JSON: {error}")

    tree_command = [
        "cargo", "tree", "--locked", "--target", args.target,
        "--edges", "normal,build", "--no-dedupe", "--prefix", "none",
        "--format", "{p}",
    ]
    active = active_package_ids(metadata, cargo_output(tree_command, "cargo tree"))
    workspace = set(metadata["workspace_members"])
    packages = [p for p in metadata["packages"] if p["id"] in active and p["id"] not in workspace]
    packages.sort(key=lambda p: (p["name"], p["version"], p["id"]))
    if not packages:
        fail("resolved graph contains no third-party packages")

    temporary = Path(tempfile.mkdtemp(prefix=f".{output.name}.", dir=output.parent))
    completed = False
    entries: list[dict] = []
    components: set[str] = set()
    try:
        licenses_root = temporary / "licenses"
        licenses_root.mkdir()
        copied_count = 0
        packages_without_files: list[str] = []

        for package in packages:
            name, version = package["name"], package["version"]
            component = package_component(name, version)
            if component in components:
                fail(f"duplicate package output path: {component}")
            components.add(component)

            license_expression = package.get("license")
            license_file = package.get("license_file")
            if not license_expression and not license_file:
                fail(f"{name} {version} has neither license metadata nor a license-file")

            package_dir = Path(package["manifest_path"]).parent.resolve(strict=True)
            candidates: list[Path] = []
            for candidate in package_dir.rglob("*"):
                try:
                    relative = candidate.relative_to(package_dir)
                except ValueError:
                    fail(f"path escaped package directory for {name} {version}: {candidate}")
                if is_notice_file(relative) and confined_file(package_dir, candidate):
                    candidates.append(relative)

            if license_file:
                explicit = Path(license_file)
                if explicit.is_absolute() or ".." in explicit.parts:
                    fail(f"unsafe license-file for {name} {version}: {license_file}")
                candidate = package_dir / explicit
                if not confined_file(package_dir, candidate):
                    fail(f"license-file is missing or escapes package directory for {name} {version}: {license_file}")
                candidates.append(explicit)

            candidates = sorted(set(candidates), key=lambda path: path.as_posix())
            files = []
            for relative in candidates:
                source = package_dir / relative
                destination = licenses_root / component / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                data = source.read_bytes()
                destination.write_bytes(data)
                copied_count += 1
                files.append({
                    "path": f"licenses/{component}/{relative.as_posix()}",
                    "sha256": hashlib.sha256(data).hexdigest(),
                })

            if not candidates:
                supplement = supplements.get((name, version))
                if supplement is None:
                    packages_without_files.append(f"{name} {version}")
                else:
                    if supplement.get("metadata_repository") != package.get("repository"):
                        fail(f"supplemental repository mismatch for {name} {version}")
                    if supplement.get("declared_license") and supplement["declared_license"] != license_expression:
                        fail(f"supplemental declared-license mismatch for {name} {version}")
                    actual_revision = package_vcs_revision(package_dir)
                    expected_revision = supplement.get("revision")
                    if actual_revision != expected_revision:
                        fail(
                            f"supplemental revision mismatch for {name} {version}: "
                            f"expected {expected_revision}, registry package has {actual_revision}"
                        )
                    for file_id in supplement["files"]:
                        source_entry = supplemental_files[file_id]
                        source = args.supplements.absolute().parent / source_entry["path"]
                        relative = Path("supplemental") / source_entry["path"]
                        destination = licenses_root / component / relative
                        destination.parent.mkdir(parents=True, exist_ok=True)
                        data = source.read_bytes()
                        destination.write_bytes(data)
                        copied_count += 1
                        files.append({
                            "path": f"licenses/{component}/{relative.as_posix()}",
                            "sha256": source_entry["sha256"],
                            "source_url": source_entry["source_url"],
                            "origin": source_entry.get("origin", "upstream_revision"),
                            **({"revision": source_entry["revision"]} if source_entry.get("revision") else {}),
                            **({"standard": source_entry["standard"]} if source_entry.get("standard") else {}),
                        })

            entries.append({
                "name": name,
                "version": version,
                "license": license_expression or f"license-file: {license_file}",
                "repository": package.get("repository") or "<not provided>",
                "files": files,
            })

        if packages_without_files:
            missing = "\n  - ".join(packages_without_files)
            fail(
                "resolved packages have neither package-local nor hash-verified supplemental "
                "license/notice source text; publication is blocked:\n  - " + missing
            )

        index = [
            "THIRD-PARTY DEPENDENCY NOTICES",
            f"Target: {args.target}",
            "Generated from Cargo.lock, the target-specific active normal/build cargo tree,",
            "and target-filtered cargo metadata for exact package identities and source lookup.",
            "The project Apache-2.0 license does not relicense these dependencies.",
            "",
        ]
        for entry in entries:
            index.extend([
                f"Package: {entry['name']} {entry['version']}",
                f"License: {entry['license']}",
                f"Repository: {entry['repository']}",
            ])
            for file in entry["files"]:
                source = f" (SHA-256 {file['sha256']})"
                if "source_url" in file:
                    if file.get("origin") == "canonical_standard_text":
                        source += f" [canonical {file['standard']} legal text; not upstream-shipped; {file['source_url']}]"
                    else:
                        source += f" [upstream {file['source_url']}; revision {file['revision']}]"
                index.append(f"Source text: {file['path']}{source}")
            index.append("")

        (temporary / "THIRD_PARTY_NOTICES.txt").write_text("\n".join(index), encoding="utf-8")
        manifest = {"format": 1, "target": args.target, "packages": entries}
        (temporary / "manifest.json").write_text(
            json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        publish_directory(temporary, output)
        completed = True
        print(
            f"collected {copied_count} source files for {len(entries)} packages into {output}",
            file=sys.stderr,
        )
    finally:
        if not completed:
            shutil.rmtree(temporary, ignore_errors=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
