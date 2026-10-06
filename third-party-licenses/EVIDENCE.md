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

## MIT packages without package-local license files — resolved with exact declaration evidence

The release notice policy now handles the three legacy crates below the same way it already handles an exact package declaration paired with canonical standard text:

1. the exact registry archive is pinned by its SHA-256 package checksum from Cargo.lock / `.cargo-checksum.json`;
2. the checked-in declaration evidence is the exact revision's `Cargo.toml`, including `license = "MIT"`;
3. the repository URL and package name/version must match Cargo metadata;
4. the canonical MIT terms are copied from SPDX at https://spdx.org/licenses/MIT.txt and hash-verified as `canonical_standard_text`;
5. the collector does **not** invent or synthesize a copyright holder/year that upstream did not publish.

This closes the release-notice generation blocker while preserving the distinction between an upstream package-local notice and canonical standard license terms. It remains an engineering provenance record, not legal advice.

### block 0.1.6

- Registry package SHA-256: `0d8c1fef690941d3e7788d328517591fecc684c084084702d6ff1641e993699a`.
- Exact version tag/revision: `0.1.6` / `47178790cfc9d4a8b092051d8b413b78bd31254a`.
- Exact declaration: `license = "MIT"`, author metadata `Steven Sheldon`:
  https://github.com/SSheldon/rust-block/blob/47178790cfc9d4a8b092051d8b413b78bd31254a/Cargo.toml
- Checked-in declaration evidence: `evidence/block-0.1.6-Cargo.toml`.
- No package-local copyright notice is fabricated; the notice bundle labels the SPDX text as canonical standard text.

### dispatch 0.2.0

- Registry package SHA-256: `bd0c93bb4b0c6d9b77f4435b0ae98c24d17f1c45b2ff844c6151a07256ca923b`.
- Registry provenance revision: `82d6c7a5b75dc0c71c3f46f87bb6c16a476f7748`.
- Exact declaration: `license = "MIT"`, author metadata `Steven Sheldon`:
  https://github.com/SSheldon/rust-dispatch/blob/82d6c7a5b75dc0c71c3f46f87bb6c16a476f7748/Cargo.toml
- Checked-in declaration evidence: `evidence/dispatch-0.2.0-Cargo.toml`.
- This is the active legacy MIT dependency in the default macOS tree; the collector now resolves it without weakening package identity checks.

### malloc_buf 0.0.6

- Registry package SHA-256: `62bb907fe88d54d8d9ce32a3cceab4218ed2f6b7d35617cafe9adf84e43919cb`.
- Exact version tag/revision: `0.0.6` / `a7811e5f4c6f9685dd57ad9bfe34e9bf6b0ba4f9`.
- Exact declaration: `license = "MIT"`, author metadata `Steven Sheldon`:
  https://github.com/SSheldon/malloc_buf/blob/a7811e5f4c6f9685dd57ad9bfe34e9bf6b0ba4f9/Cargo.toml
- Checked-in declaration evidence: `evidence/malloc_buf-0.0.6-Cargo.toml`.
- A later repository LICENSE exists, but the release collector does not retroactively attribute its 2020 copyright line to version 0.0.6. It uses the exact 0.0.6 MIT declaration plus canonical MIT terms instead.

## Release notice gate

For all three release targets, `collect-third-party-notices.py` must complete successfully before packaging. Any package with neither package-local license/notice material nor a hash-verified supplement still fails closed and blocks publication.

