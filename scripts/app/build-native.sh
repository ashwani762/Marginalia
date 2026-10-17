#!/usr/bin/env bash
# Builds the Rust core (whisper.cpp + RAG + meetings) for Android and drops the
# shared libraries into app/src/main/jniLibs.
#
#   scripts/app/build-native.sh            all variants
#   scripts/app/build-native.sh arm64      phone builds only (baseline + i8mm)
#   scripts/app/build-native.sh x86_64     emulator build only
#
# arm64 is built twice: a baseline for any Armv8.2 phone (dotprod) and an i8mm build
# for Armv8.6+/Armv9 phones. The app picks one at launch from /proc/cpuinfo.
#
# Needs cargo-ndk and the Android NDK. Environment:
#   ANDROID_NDK_HOME   NDK to use (default: the newest one under ~/Android/Sdk/ndk)
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
core="$root/core"
jni="$root/app/src/main/jniLibs"
: "${ANDROID_NDK_HOME:=$(ls -d "$HOME"/Android/Sdk/ndk/* | sort -V | tail -1)}"
export ANDROID_NDK_HOME
api=28
sysroot="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib"

want="${1:-all}"

build() { # <abi> <rust-triple> <ndk-triple> <target-dir> <out-name> [env...]
  local abi="$1" triple="$2" ndk_triple="$3" tdir="$4" name="$5"
  shift 5
  echo "==> $abi → $name"
  (cd "$core" && env "$@" CARGO_TARGET_DIR="$core/$tdir" \
     cargo ndk -t "$abi" --platform "$api" build --release)
  mkdir -p "$jni/$abi"
  cp "$core/$tdir/$triple/release/libmarginalia_core.so" "$jni/$abi/$name"
  cp "$sysroot/$ndk_triple/libc++_shared.so" "$jni/$abi/"
}

if [[ "$want" == all || "$want" == arm64 ]]; then
  build arm64-v8a aarch64-linux-android aarch64-linux-android target libmarginalia_core.so \
    GGML_CPU_ARM_ARCH=armv8.2-a+dotprod+fp16
  build arm64-v8a aarch64-linux-android aarch64-linux-android target-i8mm libmarginalia_core_i8mm.so \
    GGML_CPU_ARM_ARCH=armv8.6-a+i8mm+dotprod+fp16
fi
if [[ "$want" == all || "$want" == x86_64 ]]; then
  build x86_64 x86_64-linux-android x86_64-linux-android target-x86 libmarginalia_core.so \
    RUSTFLAGS="-C target-feature=+avx2,+fma,+f16c,+bmi2"
fi
echo "done: $(ls "$jni"/*)"
