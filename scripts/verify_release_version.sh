#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: scripts/verify_release_version.sh vX.Y.Z" >&2
  exit 2
fi

TAG="$1"
VERSION="${TAG#v}"
if [[ "$TAG" != v* || "$VERSION" == "$TAG" || -z "$VERSION" ]]; then
  echo "release tag must look like vX.Y.Z, got: $TAG" >&2
  exit 1
fi

PACKAGE_VERSION="$(node -p "require('./package.json').version")"
CARGO_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"

if [[ "$PACKAGE_VERSION" != "$VERSION" ]]; then
  echo "package.json version $PACKAGE_VERSION does not match tag $TAG" >&2
  exit 1
fi

if [[ "$CARGO_VERSION" != "$VERSION" ]]; then
  echo "Cargo.toml version $CARGO_VERSION does not match tag $TAG" >&2
  exit 1
fi

echo "release version ok: $VERSION"
