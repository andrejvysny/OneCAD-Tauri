#!/usr/bin/env bash
# Build the OneCAD C++ sidecar worker and stage it for Tauri bundling.
#
# Usage: scripts/build-worker.sh [Debug|Release]   (default: Release)
#
# Produces src-tauri/binaries/onecad-worker-<rust-host-triple>, the name Tauri's
# bundle.externalBin expects. Run from anywhere; paths resolve to the repo root.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

BUILD_TYPE="${1:-Release}"

# Rust host triple names the sidecar binary; fall back to Apple Silicon if
# rustc is not on PATH.
if command -v rustc >/dev/null 2>&1; then
    TRIPLE="$(rustc -Vv | sed -n 's/^host: //p')"
fi
if [ -z "${TRIPLE:-}" ]; then
    TRIPLE="aarch64-apple-darwin"
fi

echo "==> Building onecad-worker (${BUILD_TYPE}) for triple ${TRIPLE}"

cmake -S "${ROOT_DIR}/worker" -B "${ROOT_DIR}/worker/build" \
    -DCMAKE_BUILD_TYPE="${BUILD_TYPE}"
cmake --build "${ROOT_DIR}/worker/build" -j

DEST_DIR="${ROOT_DIR}/src-tauri/binaries"
mkdir -p "${DEST_DIR}"
DEST="${DEST_DIR}/onecad-worker-${TRIPLE}"
cp "${ROOT_DIR}/worker/build/onecad-worker" "${DEST}"
chmod +x "${DEST}"

echo "==> Staged sidecar: ${DEST}"
