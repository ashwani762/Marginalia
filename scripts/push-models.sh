#!/usr/bin/env bash
# Development shortcut: copy the models to a connected device or emulator instead of
# embedding them in the APK (build that APK with -Pmarginalia.embedModels=false).
set -euo pipefail
src="${MODELS:-$HOME/.local/share/marginalia/models}"
adb="${ADB:-adb}"
dest="/sdcard/Android/data/app.marginalia/files/models"
files=(ggml-small-q8_0.bin)
litert="${LITERT:-$HOME/.litert-lm/models/gemma-4-E2B-it.litertlm}"
embed="${EMBED:-$HOME/.litert-lm/models/embeddinggemma-2-text-270m.litertlm}"
$adb shell mkdir -p "$dest"
for m in "$litert" "$embed"; do
  [[ -f "$m" ]] && { echo "pushing $(basename "$m")"; $adb push "$m" "$dest/$(basename "$m")"; }
done
$adb shell mkdir -p "$dest"
for f in "${files[@]}"; do
  echo "pushing $f"
  $adb push "$src/$f" "$dest/$f"
done
