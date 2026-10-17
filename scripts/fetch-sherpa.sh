#!/usr/bin/env bash
# Fetches the sherpa-onnx Android library (speaker separation). Apache-2.0, about 50 MB, not committed.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$root/app/libs"
v=1.13.8
f="$root/app/libs/sherpa-onnx-$v.aar"
[[ -f "$f" ]] || curl -fL "https://github.com/k2-fsa/sherpa-onnx/releases/download/v$v/sherpa-onnx-$v.aar" -o "$f"
echo "ok: $f"
