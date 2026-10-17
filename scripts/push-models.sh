#!/usr/bin/env bash
# Development shortcut: copy the models to a connected device or emulator instead of
# embedding them in the APK (build that APK with -Pmarginalia.embedModels=false).
#
#   scripts/push-models.sh
#
# Environment:
#   MODELS   folder with the Whisper file       (default: ~/.local/share/marginalia/models)
#   LITERT   Gemma LiteRT-LM file               (default: ~/.litert-lm/models/gemma-4-E2B-it.litertlm)
#   EMBED    EmbeddingGemma LiteRT-LM file      (default: ~/.litert-lm/models/embeddinggemma-2-text-270m.litertlm)
#   ADB      adb command to use                 (default: adb)
set -euo pipefail

src="${MODELS:-$HOME/.local/share/marginalia/models}"
adb="${ADB:-adb}"
litert="${LITERT:-$HOME/.litert-lm/models/gemma-4-E2B-it.litertlm}"
embed="${EMBED:-$HOME/.litert-lm/models/embeddinggemma-2-text-270m.litertlm}"
dest="/sdcard/Android/data/app.marginalia/files/models"
files=(ggml-small-q8_0.bin)

$adb shell mkdir -p "$dest"
for m in "$litert" "$embed"; do
  if [[ -f "$m" ]]; then
    echo "pushing $(basename "$m")"
    $adb push "$m" "$dest/$(basename "$m")"
  fi
done
for f in "${files[@]}"; do
  echo "pushing $f"
  $adb push "$src/$f" "$dest/$f"
done
