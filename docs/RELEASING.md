# Releasing

Releases are intentionally unsigned prereleases until signing and notarization credentials exist.

## Preconditions

1. Astra completes final review.
2. `cargo fmt --all -- --check`, `cargo check --locked`, `cargo clippy --all-targets -- -D warnings`, and `cargo test --locked` pass.
3. `scripts/test-native-ssh-smoke.py` passes through Inspirum -> system OpenSSH on Linux x64, Windows x64, and macOS Apple Silicon, covering authentication, terminal I/O, PTY resize, reconnect, changed-host-key rejection before authentication, and cleanup.
4. `scripts/test-ssh-integration.sh` passes its deeper isolated Linux authentication/forwarding/file-transfer fixture.
5. The release-package CI gate has downloaded, clean-extracted, architecture-inspected, and executed each native package.
6. `Cargo.toml` contains the intended `MAJOR.MINOR.PATCH` version.
7. Documentation and known limitations are current.

## Tag contract

Create an annotated tag named exactly `vMAJOR.MINOR.PATCH` at the reviewed commit. The part after `v` must equal `package.version` in `Cargo.toml`. Push only after Astra approval.

A pushed `v*` tag starts the release workflow. Manual dispatch accepts an existing tag and applies the same format, version, and checked-out-commit validation; it does not create a tag.

## Produced assets

- `inspirum-terminal-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz`
- `inspirum-terminal-vX.Y.Z-x86_64-pc-windows-msvc.zip`
- `inspirum-terminal-vX.Y.Z-aarch64-apple-darwin.tar.gz`
- `SHA256SUMS`

Each platform archive contains the executable, `LICENSE`, `README.md`, `THIRD_PARTY_NOTICES/`, and `RELEASE_METADATA.json`. The metadata records the package version, tag, exact Git commit, native target, release channel, developer-signing state, and notarization state. Current archives are deliberately marked `unsigned-prerelease`, `developer_signed: false`, and `notarized: false`. The notice directory is regenerated for that target from `cargo tree --locked --target TARGET --edges normal,build --no-dedupe --prefix none --format '{p}'`, using the same default feature selection as the release build. This selects the active target-specific production normal/build closure, conservatively including build dependencies and procedural macros while excluding dev dependencies and disabled optional dependencies. Target-filtered `cargo metadata --locked` remains the source of exact package identities, source directories, and package metadata; a missing or ambiguous tree-to-metadata identity, either Cargo subprocess failing, or malformed output blocks publication. There are no package-name exclusions. Its deterministic index records every selected dependency's name, version, declared license, repository, copied source-text paths, and hashes; only license/notice material is copied from dependency package directories, including embedded-font terms and vendored `INSPIRUM_PATCHES.md` or `UPSTREAM.md` provenance files when present. Generation is fail-closed per package. When a registry package omits a monorepo-root license, the collector may use only a checked-in `third-party-licenses/manifest.json` entry matching the exact package name, version, Cargo repository metadata, and registry `.cargo_vcs_info.json` revision. Supplemental upstream source URLs are revision-pinned, every checked-in byte is SHA-256 verified, and shared upstream files are referenced by multiple packages rather than duplicated. The sole standardized-text exception is a manifest-labeled canonical CC0-1.0 legal code from Creative Commons, accepted only when exact-version declaration evidence and registry revision provenance are separately hash-verified; it is explicitly not represented as upstream-shipped text. `third-party-licenses/EVIDENCE.md` records source URLs, declaration evidence, closure findings, and unresolved attribution gaps. Unsafe paths, missing license metadata, missing explicit license files, revision/version/repository/hash mismatches, or a package with neither package-local nor verified supplemental source text block publication rather than supplying guessed text. The manifest's `unresolved` entries document exact revisions lacking sufficient attributable license text and block publication only if those exact packages become active in the selected release graph. The former macOS blocker, crates.io `dispatch 0.2.0`, is no longer distributed: `[patch.crates-io]` resolves that compatibility edge to the project-owned Apache-2.0 `vendor/dispatch-compat` crate, which contains a package-local license and only the `Queue::main().exec_sync(...)` surface required by `objc2-foundation 0.2.2`. Metadata-only `block 0.1.6` and `malloc_buf 0.0.6` remain inactive. Normal CI now regenerates the Apple Silicon release notices so this closure fails closed if the old unresolved dependency returns. The output path is reserved with no-overwrite directory creation and is consumed only after the command succeeds; concurrent attempts targeting the same path fail instead of replacing it.

CI asserts the native compiler host triple before building, runs Clippy with warnings denied on every matrix target, runs the cross-platform native authenticated SSH fixture on all three native targets, and runs the deeper isolated SSH integration fixture on Linux. GitHub Actions use reviewed, commit-pinned actions.

## Downloaded-artifact gate

Normal CI and the tag-release workflow both package the release binary, upload the archive as a workflow artifact, and verify it in a separate native job after downloading it again. `scripts/verify-release-package.py` extracts into a new temporary directory and rejects path traversal, symlinks, device entries, incorrect metadata, and unexpected archive layouts.

Architecture is checked from the packaged executable bytes rather than from the build target name: Linux must be 64-bit ELF with `EM_X86_64`, Windows must be PE32+ with COFF machine `0x8664`, and macOS must be a thin 64-bit Mach-O with ARM64 CPU type `0x0100000c`. The extracted executable is then run from the clean package directory with `--version`; its output must match `Cargo.toml`. The tag-release `publish` job depends on all three native verification jobs, so a package which cannot be downloaded, extracted, identified, or executed is never published.

The release remains a prerelease and is explicitly described as unsigned. Never promote it to a stable/signed release until credential-backed signing policy and verification are enabled.

## Signing and notarization readiness

The unsigned workflow intentionally does **not** invoke signing credentials. Two credential-dependent hooks are checked in for the future signing stage:

- `scripts/sign-macos-release.sh` signs a built executable with a configured Developer ID identity, hardened runtime, and trusted timestamp, verifies the resulting code signature, and can submit a supplied ZIP to `notarytool --wait` using a keychain profile.
- `scripts/sign-windows-release.ps1` signs an executable by certificate SHA-1 thumbprint with SHA-256 plus an RFC3161 timestamp and requires `Get-AuthenticodeSignature` to report `Valid`.

These hooks belong **before** `scripts/package-release.py` if signing is enabled later. Enabling either hook also requires changing `RELEASE_METADATA.json` generation and the package verifier to assert the actual signature/notarization result. Merely possessing a certificate, running a signing command, or setting metadata is not enough to claim a signed release. macOS stable distribution may additionally move to an app bundle/DMG so notarization tickets can be stapled in the normal Apple distribution model.

## Windows reputation guidance

Current Windows prereleases are not Authenticode-signed and may therefore show Microsoft Defender SmartScreen or browser reputation warnings such as an unknown publisher. Users should verify the published SHA-256 manifest and follow their organisation's security policy; release documentation must not instruct users to bypass managed endpoint controls. A future stable signing setup should use a protected, consistent code-signing identity and timestamp every release so publisher identity and reputation can accumulate.

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

After extracting an archive, inspect `RELEASE_METADATA.json`; its `version`, `tag`, `target`, and `commit` must match the release being installed, and the current release channel must state that developer signing/notarization is false. Running the extracted executable with `--version` provides the same clean-install executable check used by CI.

Checksum matching detects changed bytes relative to the manifest; while releases are unsigned, it does not independently authenticate the publisher.
