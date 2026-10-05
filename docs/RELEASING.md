# Releasing

Releases are intentionally unsigned prereleases until signing and notarization credentials exist.

## Preconditions

1. Astra completes final review.
2. `cargo fmt --all -- --check`, `cargo check --locked`, `cargo clippy --all-targets -- -D warnings`, and `cargo test --locked` pass.
3. `scripts/test-ssh-integration.sh` passes against its isolated loopback sshd fixture on Linux.
4. The package has been tested natively on every platform claimed in release notes.
5. `Cargo.toml` contains the intended `MAJOR.MINOR.PATCH` version.
6. Documentation and known limitations are current.

## Tag contract

Create an annotated tag named exactly `vMAJOR.MINOR.PATCH` at the reviewed commit. The part after `v` must equal `package.version` in `Cargo.toml`. Push only after Astra approval.

A pushed `v*` tag starts the release workflow. Manual dispatch accepts an existing tag and applies the same format, version, and checked-out-commit validation; it does not create a tag.

## Produced assets

- `inspirum-terminal-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz`
- `inspirum-terminal-vX.Y.Z-x86_64-pc-windows-msvc.zip`
- `inspirum-terminal-vX.Y.Z-aarch64-apple-darwin.tar.gz`
- `SHA256SUMS`

Each platform archive contains the executable, `LICENSE`, `README.md`, and `THIRD_PARTY_NOTICES/`. The notice directory is regenerated for that target from `cargo tree --locked --target TARGET --edges normal,build --no-dedupe --prefix none --format '{p}'`, using the same default feature selection as the release build. This selects the active target-specific production normal/build closure, conservatively including build dependencies and procedural macros while excluding dev dependencies and disabled optional dependencies. Target-filtered `cargo metadata --locked` remains the source of exact package identities, source directories, and package metadata; a missing or ambiguous tree-to-metadata identity, either Cargo subprocess failing, or malformed output blocks publication. There are no package-name exclusions. Its deterministic index records every selected dependency's name, version, declared license, repository, copied source-text paths, and hashes; only license/notice material is copied from dependency package directories, including embedded-font terms and vendored `INSPIRUM_PATCHES.md` or `UPSTREAM.md` provenance files when present. Generation is fail-closed per package. When a registry package omits a monorepo-root license, the collector may use only a checked-in `third-party-licenses/manifest.json` entry matching the exact package name, version, Cargo repository metadata, and registry `.cargo_vcs_info.json` revision. Supplemental upstream source URLs are revision-pinned, every checked-in byte is SHA-256 verified, and shared upstream files are referenced by multiple packages rather than duplicated. The sole standardized-text exception is a manifest-labeled canonical CC0-1.0 legal code from Creative Commons, accepted only when exact-version declaration evidence and registry revision provenance are separately hash-verified; it is explicitly not represented as upstream-shipped text. `third-party-licenses/EVIDENCE.md` records source URLs, declaration evidence, closure findings, and unresolved attribution gaps. Unsafe paths, missing license metadata, missing explicit license files, revision/version/repository/hash mismatches, or a package with neither package-local nor verified supplemental source text block publication rather than supplying guessed text. The manifest's `unresolved` entries document exact revisions lacking sufficient attributable license text and continue to block affected targets. With the production closure above, macOS remains blocked only by active `dispatch 0.2.0`; metadata-only `block 0.1.6` and `malloc_buf 0.0.6` are not selected. This records a notice-text gap, not an assertion that the package lacks an open-source license. The output path is reserved with no-overwrite directory creation and is consumed only after the command succeeds; concurrent attempts targeting the same path fail instead of replacing it.

CI asserts the native compiler host triple before building, runs Clippy with warnings denied on every matrix target, and runs the real isolated SSH integration fixture on Linux. GitHub Actions use reviewed, commit-pinned actions.

The release is marked as a prerelease and explicitly described as unsigned. Never promote it to a stable release until platform validation and signing policy are complete.

## Verification

Download all assets from the same GitHub Release and verify the manifest:

Linux:

```text
sha256sum --check SHA256SUMS
```

macOS:

```text
shasum -a 256 -c SHA256SUMS
```

On Windows, compare `Get-FileHash -Algorithm SHA256 <archive>` against the corresponding manifest line.

Checksum matching detects changed bytes relative to the manifest; while releases are unsigned, it does not independently authenticate the publisher.
