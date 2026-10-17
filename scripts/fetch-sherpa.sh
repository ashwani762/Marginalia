#!/usr/bin/env bash
# Fetches the sherpa-onnx Android library (speaker separation) into app/libs.
# Apache-2.0, about 50 MB, not committed.
#
#   scripts/fetch-sherpa.sh
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
version=1.13.8
file="$root/app/libs/sherpa-onnx-$version.aar"

mkdir -p "$root/app/libs"
[[ -f "$file" ]] || curl -fL "https://github.com/k2-fsa/sherpa-onnx/releases/download/v$version/sherpa-onnx-$version.aar" -o "$file"
echo "ok: $file"
