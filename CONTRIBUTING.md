# Contributing

Inspirum Terminal is an early SSH-first Rust application. Contributions should be small enough to review and verify independently.

Current limits are in `README.md`. Do not claim full WindTerm parity, a macOS app bundle, signed or notarized artifacts, or Windows/macOS native execution that has not been run. CI pins Rust 1.95.0. The `dispatch 0.2.0` notice blocks macOS distribution, not compilation.

## Scope and review

- Pick a bounded task with explicit acceptance criteria.
- Avoid combining refactors, dependency upgrades, UI changes, and protocol features in one pull request.
- Update user-facing and architecture documentation in the same pull request as behavior changes.
- Add or update tests for changed behavior. Do not describe an untested platform or feature as supported.
- Astra performs final review and decides when changes are merged, tagged, pushed, or released.

For large or security-sensitive work, open an issue first and describe the intended boundary, threats, test strategy, and rollback.

## Development checks

Run before requesting review:

```text
cargo fmt --all -- --check
cargo check --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

Use Rust 1.95.0 so local checks match the pinned CI toolchain.

On Linux, contributors changing notice collection should run `python3 -B -m unittest discover -s scripts/tests -p 'test_*.py'`. Contributors changing terminal or SSH lifecycle behavior should also run:

```text
scripts/test-ssh-integration.sh
```

Document anything that could not be exercised. Windows x64 and macOS Apple Silicon changes need native validation; successful compilation alone is not runtime validation.

## Design rules

- Keep the product SSH-first. `docs/PARITY.md` is a roadmap and is not a WindTerm parity claim.
- Use the system OpenSSH client and preserve its configuration, agent, identity, and host-key semantics.
- Never construct a shell command from profile fields; pass arguments separately.
- Do not persist passwords, private keys, or authentication responses.
- Treat terminal output as untrusted. Do not wire remote escape sequences to host clipboard, file, URL, or notification APIs without a reviewed policy and tests.
- Keep release artifacts reproducible from the committed lockfile.
- Keep platform-specific behavior explicit and covered on its native CI runner.

## Pull requests

A pull request should include:

1. the user-visible problem and bounded solution;
2. tests run, with host platform and results;
3. documentation changed in the same pull request;
4. security or compatibility implications;
5. remaining limitations.

Do not commit generated build output, caches, credentials, local profiles, or private SSH material.

## License

By contributing, you agree that your contribution is licensed under Apache-2.0, the repository's license.
