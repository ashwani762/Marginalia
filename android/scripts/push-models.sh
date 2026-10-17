#!/usr/bin/env bash
# Development shortcut: copy the models to a connected device or emulator instead of
# embedding them in the APK (build that APK with -Pmarginalia.embedModels=false).
set -euo pipefail
src="${MODELS:-$HOME/.local/share/marginalia/models}"
adb="${ADB:-adb}"
dest="/sdcard/Android/data/app.marginalia/files/models"
files=(embeddinggemma-2-Q8_0.gguf ggml-small-q8_0.bin)
litert="${LITERT:-$HOME/.litert-lm/models/gemma-4-E2B-it.litertlm}"
[[ -f "$litert" ]] && { echo "pushing gemma-4-E2B-it.litertlm"; $adb push "$litert" "$dest/gemma-4-E2B-it.litertlm"; }
# llama.cpp fallback files (optional): LLAMA=1
if [[ "${LLAMA:-0}" == 1 ]]; then files+=(gemma-4-E2B-it-Q4_0.gguf mmproj-gemma-4-E2B-it-Q8_0.gguf); fi
$adb shell mkdir -p "$dest"
for f in "${files[@]}"; do
  echo "pushing $f"
  $adb push "$src/$f" "$dest/$f"
done
