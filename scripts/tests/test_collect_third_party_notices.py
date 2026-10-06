#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

SCRIPT = Path(__file__).resolve().parents[1] / "collect-third-party-notices.py"
SPEC = importlib.util.spec_from_file_location("collect_third_party_notices", SCRIPT)
assert SPEC and SPEC.loader
collector = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(collector)


class CollectorTests(unittest.TestCase):
    def metadata(self, root: Path, *, with_notice: bool = True) -> dict:
        root.mkdir()
        workspace = root / "workspace"
        dependency = root / "dependency"
        workspace.mkdir()
        dependency.mkdir()
        (workspace / "Cargo.toml").write_text("[package]\nname='app'\nversion='1.0.0'\n")
        (dependency / "Cargo.toml").write_text("[package]\nname='dep'\nversion='2.0.0'\n")
        if with_notice:
            (dependency / "LICENSE-MIT").write_text("published license text\n")
        workspace_id = "path+file:///workspace#app@1.0.0"
        dependency_id = "registry+https://example.invalid/index#dep@2.0.0"
        return {
            "workspace_members": [workspace_id],
            "packages": [
                {
                    "id": workspace_id,
                    "name": "app",
                    "version": "1.0.0",
                    "manifest_path": str(workspace / "Cargo.toml"),
                    "license": "Apache-2.0",
                    "license_file": None,
                    "repository": None,
                },
                {
                    "id": dependency_id,
                    "name": "dep",
                    "version": "2.0.0",
                    "manifest_path": str(dependency / "Cargo.toml"),
                    "license": "MIT",
                    "license_file": None,
                    "repository": "https://example.invalid/dep",
                },
            ],
            "resolve": {
                "nodes": [
                    {"id": workspace_id, "deps": [{"pkg": dependency_id}]},
                    {"id": dependency_id, "deps": []},
                ]
            },
        }

    def run_main(self, metadata: dict, output: Path, supplements: Path | None = None) -> int:
        encoded = json.dumps(metadata)
        tree = "app v1.0.0\ndep v2.0.0\n"
        arguments = ["--target", "x86_64-unknown-linux-gnu", "--output", str(output)]
        if supplements is not None:
            arguments.extend(["--supplements", str(supplements)])
        with mock.patch.object(collector.subprocess, "check_output", side_effect=[encoded, tree]):
            return collector.main(arguments)

    def write_crate(
        self, root: Path, name: str, *, version: str = "1.0.0", proc_macro: bool = False
    ) -> Path:
        crate = root / name
        (crate / "src").mkdir(parents=True)
        lib = "[lib]\nproc-macro = true\n" if proc_macro else ""
        (crate / "Cargo.toml").write_text(
            f"[package]\nname = {name!r}\nversion = {version!r}\nedition = '2021'\n"
            "license = 'MIT'\n" + lib
        )
        (crate / "src" / "lib.rs").write_text("")
        (crate / "LICENSE-MIT").write_text(f"license for {name}\n")
        return crate

    def cargo_fixture(self, root: Path) -> Path:
        project = root / "project"
        crates = root / "crates"
        (project / "src").mkdir(parents=True)
        (project / "src" / "main.rs").write_text("fn main() {}\n")
        names = [
            "always", "enabled-optional", "disabled-optional", "linux-target",
            "windows-target", "build-dep", "fixture-proc-macro",
        ]
        for name in names:
            self.write_crate(crates, name, proc_macro=name == "fixture-proc-macro")

        (project / "Cargo.toml").write_text(
            "[package]\nname = 'fixture-app'\nversion = '0.1.0'\nedition = '2021'\n"
            "license = 'Apache-2.0'\n\n"
            "[features]\ndefault = ['dep:enabled-optional']\n\n"
            "[dependencies]\n"
            f"always = {{ path = {os.fspath(crates / 'always')!r} }}\n"
            f"enabled-optional = {{ path = {os.fspath(crates / 'enabled-optional')!r}, optional = true }}\n"
            f"disabled-optional = {{ path = {os.fspath(crates / 'disabled-optional')!r}, optional = true }}\n"
            f"fixture-proc-macro = {{ path = {os.fspath(crates / 'fixture-proc-macro')!r} }}\n"
            + "\n[target.'cfg(target_os = \"linux\")'.dependencies]\n"
            f"linux-target = {{ path = {os.fspath(crates / 'linux-target')!r} }}\n"
            + "\n[target.'cfg(target_os = \"windows\")'.dependencies]\n"
            f"windows-target = {{ path = {os.fspath(crates / 'windows-target')!r} }}\n"
            + "\n[build-dependencies]\n"
            f"build-dep = {{ path = {os.fspath(crates / 'build-dep')!r} }}\n"
        )
        subprocess.run(
            ["cargo", "generate-lockfile", "--offline"], cwd=project, check=True,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        )
        return project

    def supplement_manifest(self, root: Path, *, version: str = "2.0.0", valid_hash: bool = True) -> Path:
        revision = "a" * 40
        license_path = root / "files" / "dep" / "LICENSE"
        license_path.parent.mkdir(parents=True)
        data = b"pinned upstream license text\n"
        license_path.write_bytes(data)
        digest = hashlib.sha256(data).hexdigest() if valid_hash else "0" * 64
        manifest = {
            "format": 1,
            "files": [{
                "id": "dep/LICENSE",
                "path": "files/dep/LICENSE",
                "repository": "https://example.invalid/dep",
                "revision": revision,
                "source_url": f"https://example.invalid/dep/raw/{revision}/LICENSE",
                "sha256": digest,
            }],
            "packages": [{
                "name": "dep",
                "version": version,
                "metadata_repository": "https://example.invalid/dep",
                "revision": revision,
                "revision_provenance": "registry .cargo_vcs_info.json git.sha1",
                "files": ["dep/LICENSE"],
            }],
            "unresolved": [],
        }
        path = root / "manifest.json"
        path.write_text(json.dumps(manifest))
        return path

    def canonical_checksum_supplement_manifest(
        self, root: Path, *, checksum: str = "b" * 64
    ) -> Path:
        standard_path = root / "files" / "spdx" / "MIT.txt"
        standard_path.parent.mkdir(parents=True)
        standard_data = b"canonical MIT terms\n"
        standard_path.write_bytes(standard_data)

        declaration_path = root / "evidence" / "dep-Cargo.toml"
        declaration_path.parent.mkdir(parents=True)
        declaration_data = (
            b"[package]\nname='dep'\nversion='2.0.0'\n"
            b"repository='https://example.invalid/dep'\nlicense='MIT'\n"
        )
        declaration_path.write_bytes(declaration_data)
        revision = "c" * 40
        manifest = {
            "format": 1,
            "files": [{
                "id": "spdx/MIT.txt",
                "origin": "canonical_standard_text",
                "path": "files/spdx/MIT.txt",
                "publisher": "SPDX",
                "sha256": hashlib.sha256(standard_data).hexdigest(),
                "source_url": "https://spdx.org/licenses/MIT.txt",
                "standard": "MIT",
            }],
            "packages": [{
                "name": "dep",
                "version": "2.0.0",
                "metadata_repository": "https://example.invalid/dep",
                "revision": revision,
                "revision_provenance": "registry checksum plus pinned declaration",
                "registry_checksum": checksum,
                "declared_license": "MIT",
                "declaration_path": "evidence/dep-Cargo.toml",
                "declaration_sha256": hashlib.sha256(declaration_data).hexdigest(),
                "declaration_url": f"https://example.invalid/dep/raw/{revision}/Cargo.toml",
                "files": ["spdx/MIT.txt"],
            }],
            "unresolved": [],
        }
        path = root / "manifest.json"
        path.write_text(json.dumps(manifest))
        return path

    def test_confinement_rejects_symlink_escape(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            package = root / "package"
            package.mkdir()
            outside = root / "outside-LICENSE"
            outside.write_text("outside\n")
            link = package / "LICENSE"
            link.symlink_to(outside)
            self.assertFalse(collector.confined_file(package, link))

    def test_existing_output_is_never_replaced(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            output = root / "notices"
            output.mkdir()
            marker = output / "keep"
            marker.write_text("untouched\n")
            metadata = self.metadata(root / "fixture")
            with self.assertRaisesRegex(SystemExit, "output already exists"):
                self.run_main(metadata, output)
            self.assertEqual(marker.read_text(), "untouched\n")

    def test_publish_refuses_concurrently_created_empty_output(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            staging = root / "staging"
            staging.mkdir()
            (staging / "manifest.json").write_text("{}\n")
            output = root / "notices"
            output.mkdir()  # Simulates creation after main's initial existence check.
            with self.assertRaisesRegex(SystemExit, "output already exists"):
                collector.publish_directory(staging, output)
            self.assertEqual(list(output.iterdir()), [])
            self.assertEqual((staging / "manifest.json").read_text(), "{}\n")

    def test_missing_per_package_text_blocks_publication(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            metadata = self.metadata(root / "fixture", with_notice=False)
            output = root / "notices"
            with self.assertRaisesRegex(SystemExit, r"publication is blocked:[\s\S]*dep 2.0.0"):
                self.run_main(metadata, output)
            self.assertFalse(output.exists())

    def test_supplemental_wrong_hash_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            supplements = self.supplement_manifest(root / "supplements", valid_hash=False)
            metadata = self.metadata(root / "fixture")
            with self.assertRaisesRegex(SystemExit, "supplemental license hash mismatch"):
                self.run_main(metadata, root / "notices", supplements)

    def test_supplemental_wrong_version_does_not_satisfy_package(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            supplements = self.supplement_manifest(root / "supplements", version="9.9.9")
            metadata = self.metadata(root / "fixture", with_notice=False)
            dependency = Path(metadata["packages"][1]["manifest_path"]).parent
            (dependency / ".cargo_vcs_info.json").write_text(
                json.dumps({"git": {"sha1": "a" * 40}})
            )
            with self.assertRaisesRegex(SystemExit, r"publication is blocked:[\s\S]*dep 2.0.0"):
                self.run_main(metadata, root / "notices", supplements)

    def test_checksum_pinned_canonical_license_supplement_is_accepted(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            metadata = self.metadata(root / "fixture", with_notice=False)
            dependency = Path(metadata["packages"][1]["manifest_path"]).parent
            checksum = "b" * 64
            (dependency / ".cargo-checksum.json").write_text(
                json.dumps({"package": checksum, "files": {}})
            )
            supplements = self.canonical_checksum_supplement_manifest(
                root / "supplements", checksum=checksum
            )
            output = root / "notices"
            self.assertEqual(self.run_main(metadata, output, supplements), 0)
            generated = json.loads((output / "manifest.json").read_text())
            files = generated["packages"][0]["files"]
            self.assertEqual(files[0]["standard"], "MIT")
            self.assertEqual(files[0]["origin"], "canonical_standard_text")

    def test_checksum_pinned_supplement_rejects_registry_mismatch(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            metadata = self.metadata(root / "fixture", with_notice=False)
            dependency = Path(metadata["packages"][1]["manifest_path"]).parent
            (dependency / ".cargo-checksum.json").write_text(
                json.dumps({"package": "d" * 64, "files": {}})
            )
            supplements = self.canonical_checksum_supplement_manifest(
                root / "supplements", checksum="b" * 64
            )
            with self.assertRaisesRegex(SystemExit, "supplemental registry checksum mismatch"):
                self.run_main(metadata, root / "notices", supplements)

    def test_deterministic_metadata_and_provenance_collection(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            metadata = self.metadata(root / "fixture")
            dependency = Path(metadata["packages"][1]["manifest_path"]).parent
            (dependency / "UPSTREAM.md").write_text("revision: abc123\n")
            first, second = root / "first", root / "second"
            self.assertEqual(self.run_main(metadata, first), 0)
            self.assertEqual(self.run_main(metadata, second), 0)

            def snapshot(directory: Path) -> dict[str, bytes]:
                return {
                    path.relative_to(directory).as_posix(): path.read_bytes()
                    for path in sorted(directory.rglob("*"))
                    if path.is_file()
                }

            self.assertEqual(snapshot(first), snapshot(second))
            manifest = json.loads((first / "manifest.json").read_text())
            paths = [entry["path"] for entry in manifest["packages"][0]["files"]]
            self.assertTrue(any(path.endswith("/UPSTREAM.md") for path in paths))
            self.assertTrue(any(path.endswith("/LICENSE-MIT") for path in paths))

    def test_real_cargo_fixture_selects_default_normal_build_and_target_closure(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            project = self.cargo_fixture(root)
            output = root / "notices"
            old_cwd = Path.cwd()
            try:
                os.chdir(project)
                with mock.patch.dict(os.environ, {"CARGO_NET_OFFLINE": "true"}):
                    self.assertEqual(
                        collector.main([
                            "--target", "x86_64-unknown-linux-gnu",
                            "--output", str(output),
                        ]),
                        0,
                    )
            finally:
                os.chdir(old_cwd)
            manifest = json.loads((output / "manifest.json").read_text())
            names = {package["name"] for package in manifest["packages"]}
            self.assertEqual(
                names,
                {"always", "enabled-optional", "linux-target", "build-dep", "fixture-proc-macro"},
            )

    def test_real_cargo_fixture_ambiguous_identity_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            project = self.cargo_fixture(root)
            metadata = json.loads(subprocess.check_output(
                [
                    "cargo", "metadata", "--locked", "--offline", "--format-version", "1",
                    "--filter-platform", "x86_64-unknown-linux-gnu",
                ],
                cwd=project, text=True,
            ))
            original = next(package for package in metadata["packages"] if package["name"] == "always")
            duplicate = dict(original)
            duplicate["id"] = original["id"] + "?alternate-source"
            duplicate["source"] = "registry+https://example.invalid/index"
            metadata["packages"].append(duplicate)
            with self.assertRaisesRegex(SystemExit, "maps ambiguously in metadata"):
                collector.active_package_ids(metadata, "always v1.0.0\n")

    def test_missing_tree_identity_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            metadata = self.metadata(Path(tmp) / "fixture")
            with self.assertRaisesRegex(SystemExit, "missing from metadata"):
                collector.active_package_ids(metadata, "absent v9.9.9\n")

    def test_cargo_tree_subprocess_failure_blocks_publication(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            metadata = self.metadata(root / "fixture")
            failure = subprocess.CalledProcessError(101, ["cargo", "tree"], stderr="tree failed")
            with mock.patch.object(
                collector.subprocess, "check_output", side_effect=[json.dumps(metadata), failure]
            ):
                with self.assertRaisesRegex(SystemExit, "cargo tree failed with exit status 101"):
                    collector.main([
                        "--target", "x86_64-unknown-linux-gnu",
                        "--output", str(root / "notices"),
                    ])


if __name__ == "__main__":
    unittest.main()
