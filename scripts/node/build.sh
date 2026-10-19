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
# The optional helper that tells speakers apart (a separate program, see compute-node/speakers/src/main.rs). A failure here
# does not stop the node: without the helper the node just does not offer it and the phone does the work itself.
if cargo build --release -p marginalia-node-speakers; then
  echo "built: $root/target/release/marginalia-node-speakers"
else
  echo "The speaker helper did not build, the node works without it." >&2
fi
