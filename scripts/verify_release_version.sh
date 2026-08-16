#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: scripts/verify_release_version.sh vX.Y.Z" >&2
  exit 2
fi

TAG="$1"
VERSION="${TAG#v}"
if [[ ! "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "release tag must look like vX.Y.Z, got: $TAG" >&2
  exit 1
fi

PACKAGE_VERSION="$(node -p "require('./package.json').version")"
CARGO_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
LOCK_VERSION="$(awk '
  $0 == "name = \"related\"" { found = 1; next }
  found && /^version = "/ {
    value = $0
    sub(/^version = "/, "", value)
    sub(/"$/, "", value)
    print value
    exit
  }
' Cargo.lock)"
FUZZ_LOCK_VERSION="$(awk '
  $0 == "name = \"related\"" { found = 1; next }
  found && /^version = "/ {
    value = $0
    sub(/^version = "/, "", value)
    sub(/"$/, "", value)
    print value
    exit
  }
' fuzz/Cargo.lock)"
WORKFLOW_TAG_EXAMPLE="$(
  sed -n 's/.*for example \(v[0-9][0-9.]*\)".*/\1/p' .github/workflows/release.yml | head -1
)"

if [[ "$PACKAGE_VERSION" != "$VERSION" ]]; then
  echo "package.json version $PACKAGE_VERSION does not match tag $TAG" >&2
  exit 1
fi

if [[ "$CARGO_VERSION" != "$VERSION" ]]; then
  echo "Cargo.toml version $CARGO_VERSION does not match tag $TAG" >&2
  exit 1
fi

if [[ "$LOCK_VERSION" != "$VERSION" ]]; then
  echo "Cargo.lock version $LOCK_VERSION does not match tag $TAG" >&2
  exit 1
fi

if [[ "$FUZZ_LOCK_VERSION" != "$VERSION" ]]; then
  echo "fuzz/Cargo.lock version $FUZZ_LOCK_VERSION does not match tag $TAG" >&2
  exit 1
fi

if [[ "$WORKFLOW_TAG_EXAMPLE" != "$TAG" ]]; then
  echo "release workflow tag example $WORKFLOW_TAG_EXAMPLE does not match tag $TAG" >&2
  exit 1
fi

echo "release version ok: $VERSION"
