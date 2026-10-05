# Supplemental notice evidence

This note records provenance research for supplemental release notices. It is not a legal audit or legal advice.

## Closure check

`cargo tree --locked --target TARGET -i PACKAGE` was run with the release lockfile and default features. `hexf-parse 0.2.1`, `block 0.1.6`, and `malloc_buf 0.0.6` did not appear in the active default-feature tree. `dispatch 0.2.0` did appear on macOS through `objc2-foundation 0.2.2 -> eframe 0.31.1 -> inspirum-terminal`. Target-filtered `cargo metadata`, which intentionally provides the release notice gate's broader inventory, contains `hexf-parse` on all three targets and all three MIT packages on macOS. Coverage was not reduced based on this distinction.

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

## dispatch 0.2.0 — unresolved

- Registry provenance revision: `82d6c7a5b75dc0c71c3f46f87bb6c16a476f7748`.
- Exact Cargo declaration (`license = "MIT"`, author `Steven Sheldon`):
  https://github.com/SSheldon/rust-dispatch/blob/82d6c7a5b75dc0c71c3f46f87bb6c16a476f7748/Cargo.toml
- Exact README and source contain no copyright notice or MIT grant text:
  https://github.com/SSheldon/rust-dispatch/blob/82d6c7a5b75dc0c71c3f46f87bb6c16a476f7748/README.md
  https://github.com/SSheldon/rust-dispatch/blob/82d6c7a5b75dc0c71c3f46f87bb6c16a476f7748/src/lib.rs
- Repository history contains no license-file addition. A holder/year cannot be derived without invention, so no supplement was added.

## malloc_buf 0.0.6 — unresolved

- Exact version tag/revision: `0.0.6` / `a7811e5f4c6f9685dd57ad9bfe34e9bf6b0ba4f9`; the registry crate has no `.cargo_vcs_info.json`.
- Exact Cargo declaration (`license = "MIT"`, author `Steven Sheldon`):
  https://github.com/SSheldon/malloc_buf/blob/a7811e5f4c6f9685dd57ad9bfe34e9bf6b0ba4f9/Cargo.toml
- Exact source contains no copyright notice or MIT grant text:
  https://github.com/SSheldon/malloc_buf/blob/a7811e5f4c6f9685dd57ad9bfe34e9bf6b0ba4f9/src/lib.rs
- A later root `LICENSE` was added in commit `d9a3e539642bd90e07df458d226b19cdfa606863` by Fred Potter with the message “Adding an MIT license since crates.io indicates the intention was to license as MIT”:
  https://github.com/SSheldon/malloc_buf/commit/d9a3e539642bd90e07df458d226b19cdfa606863
- That commit is a descendant of version 0.0.6 and names `Copyright (c) 2020 Steven Sheldon`, but it was authored by a third party and does not expressly state retroactive scope for 0.0.6. It was therefore not silently applied to the older release.

## Astra decision options for unresolved MIT packages

Keep publication blocked; obtain a version-specific license/attribution clarification from the copyright holder; or separately review replacing/updating the dependency to a release carrying complete license text. Dependency replacement or updates are outside this notice-only change.
