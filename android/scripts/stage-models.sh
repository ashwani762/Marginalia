#!/usr/bin/env bash
# Links the models into models-stage/models so Gradle packs them into the APK as assets.
# Source: the desktop app's model folder (override with MODELS=/path).
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
src="${MODELS:-$HOME/.local/share/marginalia/models}"
# Default engine: LiteRT-LM (one file holds Gemma with vision) + search + speech. Set LLAMA=1 to embed
# the llama.cpp Gemma files as well (the APK then exceeds the 4 GiB limit; use only for experiments).
LITERT="${LITERT:-$HOME/.litert-lm/models/gemma-4-E2B-it.litertlm}"
files=(ggml-small-q8_0.bin)
EMBED="${EMBED:-$HOME/.litert-lm/models/embeddinggemma-2-text-270m.litertlm}"
mkdir -p "$root/models-stage/models"
[[ -f "$LITERT" ]] || { echo "missing $LITERT (download gemma-4-E2B-it.litertlm from litert-community/gemma-4-E2B-it-litert-lm)" >&2; exit 1; }
[[ -f "$EMBED" ]] || { echo "missing $EMBED (download embeddinggemma-2-text-270m.litertlm from litert-community/embeddinggemma-2-litert-lm)" >&2; exit 1; }
ln -sf "$LITERT" "$root/models-stage/models/gemma-4-E2B-it.litertlm"
ln -sf "$EMBED" "$root/models-stage/models/embeddinggemma-2-text-270m.litertlm"
if [[ "${LLAMA:-0}" == 1 ]]; then files+=(gemma-4-E2B-it-Q4_0.gguf mmproj-gemma-4-E2B-it-Q8_0.gguf); fi
for f in "${files[@]}"; do
  [[ -f "$src/$f" ]] || { echo "missing $src/$f" >&2; exit 1; }
  ln -sf "$src/$f" "$root/models-stage/models/$f"
done
du -shL "$root"/models-stage/models/* | sed 's#.*/models-stage/#  #'
echo "staged. Build the embedded APK with: ./gradlew assembleRelease"
