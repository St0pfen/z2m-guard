#!/usr/bin/env bash
# Builds release artifacts locally without root: static musl binary, checksum
# file and CycloneDX SBOM. Everything runs in the artifacts image; the host only
# provides the source and receives dist/.
#
#   ./deploy/build-release.sh [version]
set -euo pipefail

VERSION="${1:-$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="z2m-guard-artifacts:local"
TARGET="x86_64-unknown-linux-musl"
BIN="z2m-guard-${VERSION}-${TARGET}"

cd "$ROOT"
if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
  echo "== building $IMAGE (one-off)"
  docker build -f deploy/Dockerfile.artifacts -t "$IMAGE" .
fi

mkdir -p dist
# Run as the calling user so dist/ stays writable for them; cargo home lives in
# a scratch dir inside the container.
docker run --rm \
  -u "$(id -u):$(id -g)" \
  -e CARGO_HOME=/tmp/cargo-home \
  -e HOME=/tmp \
  -v "$ROOT":/src \
  -w /src \
  "$IMAGE" \
  bash -euo pipefail -c "
    cargo build --release --target $TARGET -p z2m-guard
    cp target/$TARGET/release/z2m-guard dist/$BIN
    # cargo-cyclonedx has no --output-cdx flag in this version: it writes
    # <package>.cdx.json next to the manifest.
    cargo cyclonedx --manifest-path crates/z2m-guard/Cargo.toml \
      --format json --spec-version 1.5
    mv crates/z2m-guard/z2m-guard.cdx.json "dist/z2m-guard-${VERSION}.cdx.json"
  "

cd dist
# Notices travel with the binary: MIT/BSD/ISC/Apache terms require the licence
# text and attribution to be redistributed alongside a compiled artifact.
cp ../LICENSE-MIT ../LICENSE-APACHE .
( cd .. && python3 scripts/license-notices.py )
sha256sum "$BIN" "z2m-guard-${VERSION}.cdx.json" \
  LICENSE-MIT LICENSE-APACHE THIRD-PARTY-LICENSES.txt > SHA256SUMS
sha256sum -c SHA256SUMS
echo "== artifacts in $(pwd)"
ls -l
file "$BIN" || true
