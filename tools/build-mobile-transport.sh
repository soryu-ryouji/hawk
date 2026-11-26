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
  # 与 Xcode target 对齐，避免 C/汇编依赖按当前 SDK 的 27.x 最低版本生成。
  export IPHONEOS_DEPLOYMENT_TARGET=26.0
  targets=("${TARGET:-aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios}")
  device_libraries=()
  simulator_libraries=()
  for target in $targets; do
    case "$target" in
      aarch64-apple-ios|aarch64-apple-ios-sim|x86_64-apple-ios) ;;
      *) echo "未知 iOS target: $target" >&2; exit 2 ;;
    esac
    # blake3 的 build.rs 未监听部署版本环境变化，旧对象必须主动清理。
    for artifacts in \
      "$ROOT/hawk-transport/target/$target/release/build/blake3-"* \
      "$ROOT/hawk-transport/target/$target/release/.fingerprint/blake3-"*; do
      [[ -e "$artifacts" ]] || continue
      case "$artifacts" in
        "$ROOT/hawk-transport/target/$target/release/"*) rm -rf -- "$artifacts" ;;
        *) echo "blake3 清理路径异常: $artifacts" >&2; exit 2 ;;
      esac
    done
    cargo build --manifest-path "$ROOT/hawk-transport/Cargo.toml" --release --target "$target"
    library="$ROOT/hawk-transport/target/$target/release/libhawk_transport.a"
    if [[ "$target" == "aarch64-apple-ios" ]]; then
      device_libraries+=("$library")
    else
      simulator_libraries+=("$library")
    fi
  done
  out="$ROOT/hawk-ios/Frameworks"
  framework="$out/HawkTransport.xcframework"
  mkdir -p "$out"
  libraries=()
  if ((${#device_libraries[@]})); then
    libraries+=(-library "${device_libraries[0]}")
  fi
  if ((${#simulator_libraries[@]} == 1)); then
    libraries+=(-library "${simulator_libraries[0]}")
  elif ((${#simulator_libraries[@]} > 1)); then
    simulator="$out/libhawk_transport-simulator.a"
    lipo -create "${simulator_libraries[@]}" -output "$simulator"
    libraries+=(-library "$simulator")
  fi
  if [[ "$framework" != "$ROOT/hawk-ios/Frameworks/HawkTransport.xcframework" ]]; then
    echo "XCFramework 输出路径异常" >&2
    exit 2
  fi
  rm -rf "$framework"
  # 静态 XCFramework 同时供主 App 与 Share Extension 链接，无需嵌入动态库。
  xcodebuild -create-xcframework "${libraries[@]}" -output "$framework"
else
  echo "用法: tools/build-mobile-transport.sh android|ios [target]" >&2
  exit 2
fi
