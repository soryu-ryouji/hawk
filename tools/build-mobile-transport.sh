#!/usr/bin/env bash
set -euo pipefail

# 构建 hawk-transport 原生库并复制到移动端打包目录。
# 用法：tools/build-mobile-transport.sh android|ios [target]
# 需先 rustup target add 对应目标，并安装 Android NDK 或 Xcode 工具链。

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PLATFORM="${1:?android|ios}"
TARGET="${2:-}"

if [[ "$PLATFORM" == "android" ]]; then
  targets=("${TARGET:-aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android}")
  for target in $targets; do
    case "$target" in
      aarch64-linux-android) abi="arm64-v8a" ;;
      armv7-linux-androideabi) abi="armeabi-v7a" ;;
      x86_64-linux-android) abi="x86_64" ;;
      i686-linux-android) abi="x86" ;;
      *) echo "未知 Android target: $target" >&2; exit 2 ;;
    esac
    cargo build --manifest-path "$ROOT/hawk-transport/Cargo.toml" --release --target "$target"
    out="$ROOT/hawk-android/app/src/main/jniLibs/$abi"
    mkdir -p "$out"
    cp "$ROOT/hawk-transport/target/$target/release/hawk_transport.dll" "$out/libhawk_transport.so" 2>/dev/null || \
      cp "$ROOT/hawk-transport/target/$target/release/libhawk_transport.so" "$out/libhawk_transport.so"
  done
elif [[ "$PLATFORM" == "ios" ]]; then
  targets=("${TARGET:-aarch64-apple-ios aarch64-apple-ios-sim}")
  for target in $targets; do
    cargo build --manifest-path "$ROOT/hawk-transport/Cargo.toml" --release --target "$target"
    out="$ROOT/hawk-ios/Frameworks"
    mkdir -p "$out"
    cp "$ROOT/hawk-transport/target/$target/release/libhawk_transport.dylib" "$out/libhawk_transport_$target.dylib"
  done
else
  echo "用法: tools/build-mobile-transport.sh android|ios [target]" >&2
  exit 2
fi
