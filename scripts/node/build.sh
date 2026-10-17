#!/usr/bin/env bash
# Builds the node. Pass --cpu-only for a build without CUDA (also what a Raspberry Pi needs).
#   scripts/node/build.sh            CUDA build for an NVIDIA card (CUDAARCHS defaults to the RTX 40 series, 89)
#   CUDAARCHS=86 scripts/node/build.sh
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
features=(--features cuda)
[[ "${1:-}" == "--cpu-only" ]] && features=()
export CUDAARCHS="${CUDAARCHS:-89}" CMAKE_BUILD_PARALLEL_LEVEL="${CMAKE_BUILD_PARALLEL_LEVEL:-$(nproc)}"
cargo build --release -p marginalia-node "${features[@]}"
echo "built: $root/target/release/marginalia-node"
