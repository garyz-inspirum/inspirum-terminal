# Supplemental notice evidence

This note records provenance research for supplemental release notices. It is not a legal audit or legal advice.

## Closure check

`cargo tree --locked --target TARGET -i PACKAGE` was run with the release lockfile and default features. `hexf-parse 0.2.1`, `block 0.1.6`, and `malloc_buf 0.0.6` do not appear in the active default-feature tree. The previously active crates.io `dispatch 0.2.0` edge on macOS has been replaced with the project-owned `vendor/dispatch-compat` package via `[patch.crates-io]`. The compatibility package implements only the `Queue::main().exec_sync(...)` surface required by `objc2-foundation 0.2.2`, is Apache-2.0 licensed, and carries its package-local `LICENSE`. The unresolved upstream `rust-dispatch 0.2.0` bytes are therefore no longer in the production dependency closure or release notices. Target-filtered `cargo metadata` remains the broader inventory used by the release notice gate.

## hexf-parse 0.2.1 — resolved with canonical CC0 text

- Registry provenance revision: `4225763d744183d720f575ae96d04161b4d08ea0` (`.cargo_vcs_info.json`).
- Exact-revision declaration: `license = "CC0-1.0"` in `parse/Cargo.toml`:
  https://github.com/lifthrasiir/hexf/blob/4225763d744183d720f575ae96d04161b4d08ea0/parse/Cargo.toml
- Checked-in declaration evidence: `evidence/hexf-parse-0.2.1-Cargo.toml`, byte-for-byte from:
  https://raw.githubusercontent.com/lifthrasiir/hexf/4225763d744183d720f575ae96d04161b4d08ea0/parse/Cargo.toml
- Canonical CC0 1.0 legal code publisher: Creative Commons:
  https://creativecommons.org/publicdomain/zero/1.0/legalcode.txt
- The canonical legal code is labeled `canonical_standard_text`; it is not represented as a file shipped by hexf-parse upstream. The collector separately verifies the exact package version, repository, registry revision, declared license, declaration evidence hash, and canonical-text hash/source.

## block 0.1.6 — unresolved

- Exact version tag/revision: `0.1.6` / `47178790cfc9d4a8b092051d8b413b78bd31254a`; the registry crate has no `.cargo_vcs_info.json`.
- Exact Cargo declaration (`license = "MIT"`, author `Steven Sheldon`):
  https://github.com/SSheldon/rust-block/blob/47178790cfc9d4a8b092051d8b413b78bd31254a/Cargo.toml
- Exact README and source contain no copyright notice or MIT grant text:
  https://github.com/SSheldon/rust-block/blob/47178790cfc9d4a8b092051d8b413b78bd31254a/README.md
  https://github.com/SSheldon/rust-block/blob/47178790cfc9d4a8b092051d8b413b78bd31254a/src/lib.rs
- Repository history contains no `LICENSE`, `LICENSE.md`, `LICENSE.txt`, or `COPYING` addition. A holder/year cannot be derived without invention, so no supplement was added.

## dispatch 0.2.0 — retired from the active release graph

- Registry provenance revision: `82d6c7a5b75dc0c71c3f46f87bb6c16a476f7748`.
- Exact Cargo declaration (`license = "MIT"`, author `Steven Sheldon`):
  https://github.com/SSheldon/rust-dispatch/blob/82d6c7a5b75dc0c71c3f46f87bb6c16a476f7748/Cargo.toml
- Exact README and source contain no copyright notice or MIT grant text:
  https://github.com/SSheldon/rust-dispatch/blob/82d6c7a5b75dc0c71c3f46f87bb6c16a476f7748/README.md
  https://github.com/SSheldon/rust-dispatch/blob/82d6c7a5b75dc0c71c3f46f87bb6c16a476f7748/src/lib.rs
- Repository history contains no license-file addition. A holder/year cannot be derived without invention, so no supplement was added.
- This exact registry package is retained here only as historical provenance. The release graph now resolves `dispatch 0.2.0` to the project-owned Apache-2.0 `vendor/dispatch-compat` package, so these unresolved upstream bytes are not distributed.

## malloc_buf 0.0.6 — unresolved

- Exact version tag/revision: `0.0.6` / `a7811e5f4c6f9685dd57ad9bfe34e9bf6b0ba4f9`; the registry crate has no `.cargo_vcs_info.json`.
- Exact Cargo declaration (`license = "MIT"`, author `Steven Sheldon`):
  https://github.com/SSheldon/malloc_buf/blob/a7811e5f4c6f9685dd57ad9bfe34e9bf6b0ba4f9/Cargo.toml
- Exact source contains no copyright notice or MIT grant text:
  https://github.com/SSheldon/malloc_buf/blob/a7811e5f4c6f9685dd57ad9bfe34e9bf6b0ba4f9/src/lib.rs
- A later root `LICENSE` was added in commit `d9a3e539642bd90e07df458d226b19cdfa606863` by Fred Potter with the message “Adding an MIT license since crates.io indicates the intention was to license as MIT”:
  https://github.com/SSheldon/malloc_buf/commit/d9a3e539642bd90e07df458d226b19cdfa606863
- That commit is a descendant of version 0.0.6 and names `Copyright (c) 2020 Steven Sheldon`, but it was authored by a third party and does not expressly state retroactive scope for 0.0.6. It was therefore not silently applied to the older release.

## Remaining historical unresolved MIT packages

`block 0.1.6` and `malloc_buf 0.0.6` remain documented for provenance but are not selected by the active production tree. If either becomes active in a future release graph, publication must fail closed until version-specific license/attribution evidence is resolved.

## Iced default release dependencies

Iced is enabled in the default release graph. The published Iced subcrates, Lyon crates, and svg_fmt omit package-local license text. Each new supplemental package entry is pinned to its published `.cargo_vcs_info.json` revision and repository. The checked-in license texts are fetched byte-for-byte from that revision of the upstream repository; `manifest.json` records the exact URL and SHA-256 for each file. No attribution text is synthesized. The strict collector validates these supplements against the locked package metadata for Linux, Windows, and macOS.

The macOS production dependency tree uses Iced tiny-skia. The WGPU Metal backend would reactivate the historically unresolved `block 0.1.6` and `malloc_buf 0.0.6` packages documented above, so WGPU is enabled only on Linux and Windows. `objc2-quartz-core 0.3.2` uses the existing hash-verified upstream license supplement at its registry revision.
