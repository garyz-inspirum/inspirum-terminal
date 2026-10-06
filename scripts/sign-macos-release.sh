#!/usr/bin/env bash
# Credential-dependent signing hook. The release workflow intentionally does not
# call this until repository signing credentials and signed-artifact metadata are enabled.
set -euo pipefail

if [[ "${1:-}" == "--help" || $# -eq 0 ]]; then
  cat <<'EOF'
Usage: MACOS_SIGNING_IDENTITY='Developer ID Application: ...' sign-macos-release.sh BINARY [NOTARY_ZIP]

Signs BINARY with hardened runtime and a trusted timestamp, then verifies the
signature. If NOTARY_ZIP is supplied, MACOS_NOTARY_PROFILE must name a
notarytool keychain profile and the ZIP is submitted with --wait.

This hook is not invoked by the unsigned release workflow.
EOF
  exit 0
fi

binary="$1"
test -f "$binary" || { echo "binary not found: $binary" >&2; exit 1; }
: "${MACOS_SIGNING_IDENTITY:?MACOS_SIGNING_IDENTITY is required}"

codesign --force --options runtime --timestamp --sign "$MACOS_SIGNING_IDENTITY" "$binary"
codesign --verify --strict --verbose=2 "$binary"

if [[ $# -ge 2 ]]; then
  archive="$2"
  test -f "$archive" || { echo "notary archive not found: $archive" >&2; exit 1; }
  : "${MACOS_NOTARY_PROFILE:?MACOS_NOTARY_PROFILE is required for notarization}"
  xcrun notarytool submit "$archive" --keychain-profile "$MACOS_NOTARY_PROFILE" --wait
fi
