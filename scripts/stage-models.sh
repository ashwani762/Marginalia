#!/usr/bin/env bash
# Links the models into models-stage/models so Gradle packs them into the APK as assets.
#
#   scripts/stage-models.sh
#
# Environment:
#   MODELS   folder with the Whisper file       (default: ~/.local/share/marginalia/models)
#   LITERT   Gemma LiteRT-LM file               (default: ~/.litert-lm/models/gemma-4-E2B-it.litertlm)
#   EMBED    EmbeddingGemma LiteRT-LM file      (default: ~/.litert-lm/models/embeddinggemma-2-text-270m.litertlm)
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
src="${MODELS:-$HOME/.local/share/marginalia/models}"
litert="${LITERT:-$HOME/.litert-lm/models/gemma-4-E2B-it.litertlm}"
embed="${EMBED:-$HOME/.litert-lm/models/embeddinggemma-2-text-270m.litertlm}"
files=(ggml-small-q8_0.bin)

mkdir -p "$root/models-stage/models"
[[ -f "$litert" ]] || { echo "missing $litert (download gemma-4-E2B-it.litertlm from litert-community/gemma-4-E2B-it-litert-lm)" >&2; exit 1; }
[[ -f "$embed" ]] || { echo "missing $embed (download embeddinggemma-2-text-270m.litertlm from litert-community/embeddinggemma-2-litert-lm)" >&2; exit 1; }
ln -sf "$litert" "$root/models-stage/models/gemma-4-E2B-it.litertlm"
ln -sf "$embed" "$root/models-stage/models/embeddinggemma-2-text-270m.litertlm"
for f in "${files[@]}"; do
  [[ -f "$src/$f" ]] || { echo "missing $src/$f" >&2; exit 1; }
  ln -sf "$src/$f" "$root/models-stage/models/$f"
done
du -shL "$root"/models-stage/models/* | sed 's#.*/models-stage/#  #'
echo "staged. Build the embedded APK with: ./gradlew assembleRelease"
