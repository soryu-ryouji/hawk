#!/usr/bin/env bash
# 构建（Android）：构建 hawk-android 的 APK 并归置到输出目录（默认仓库根目录的 out/）。
# 统一入口 tools/build.sh --platform android。
#
# 用法: ./tools/build-android.sh [--path <输出目录>] [--release]
#   默认产出 debug APK（debug 签名，可直接 adb install）
#   --release：产出 R8 压缩的 release APK（当前工程没有签名配置，产物未签名，不能直接安装）
# 前置: Android SDK Platform 37；JDK 21+（未设置 JAVA_HOME 时自动找 Android Studio 自带的 JBR）
#   SDK 位置：读取 hawk-android/local.properties，缺省时用 ANDROID_HOME / ANDROID_SDK_ROOT /
#   macOS 的 ~/Library/Android/sdk、Linux 的 ~/Android/Sdk 并生成 local.properties
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ANDROID_DIR="$REPO_ROOT/hawk-android"

OUT_DIR=""
RELEASE=0
while [ $# -gt 0 ]; do
  case "$1" in
    --path) [ $# -ge 2 ] || { echo "--path 需要目录参数"; exit 1; }; OUT_DIR="$2"; shift 2 ;;
    --path=*) OUT_DIR="${1#*=}"; shift ;;
    --release) RELEASE=1; shift ;;
    *) echo "未知参数: $1（用法: ./tools/build-android.sh [--path <输出目录>] [--release]）"; exit 1 ;;
  esac
done
OUT_DIR="${OUT_DIR:-$REPO_ROOT/out}"
# 相对路径按调用时的当前目录解析；构建前先建目录，避免 cd 到 hawk-android 后落到错误位置。
if command -v cygpath >/dev/null 2>&1; then
  case "$OUT_DIR" in
    [A-Za-z]:/*) OUT_DIR="$(cygpath -u "$OUT_DIR")" ;;
    [A-Za-z]:\\*) OUT_DIR="$(cygpath -u "$OUT_DIR")" ;;
  esac
fi
case "$OUT_DIR" in
  /*) ;;
  *) OUT_DIR="$PWD/$OUT_DIR" ;;
esac
mkdir -p "$OUT_DIR"

[ -f "$ANDROID_DIR/gradlew" ] || { echo "未找到 hawk-android/gradlew，仓库目录不完整"; exit 1; }

# ---- JDK：优先 JAVA_HOME，否则找 Android Studio 自带 JBR ----
if [ -z "${JAVA_HOME:-}" ] || [ ! -x "$JAVA_HOME/bin/java" ]; then
  for candidate in \
    "${ANDROID_STUDIO_JDK:-}" \
    "/c/Program Files/Android/Android Studio/jbr" \
    "$HOME/AppData/Local/Programs/Android Studio/jbr" \
    "$HOME/AppData/Local/Android/Android Studio/jbr" \
    "/Applications/Android Studio.app/Contents/jbr/Contents/Home" \
    "/opt/android-studio/jbr" \
    "$HOME/android-studio/jbr" \
    "$HOME/.local/share/JetBrains/Toolbox/apps/AndroidStudio/ch-0/jbr"; do
    if [ -n "$candidate" ] && [ -x "$candidate/bin/java" ]; then
      export JAVA_HOME="$candidate"
      echo "使用 JDK: $candidate"
      break
    fi
  done
  if { [ -z "${JAVA_HOME:-}" ] || [ ! -x "$JAVA_HOME/bin/java" ]; } && ! command -v java >/dev/null 2>&1; then
    echo "未找到 JDK，请安装 Android Studio，或设置 JAVA_HOME 指向 JDK 21+"
    exit 1
  fi
fi

# ---- Android SDK：local.properties 缺省时按环境变量/默认目录补齐 ----
if [ ! -f "$ANDROID_DIR/local.properties" ]; then
  sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
  if [ -z "$sdk" ]; then
    for candidate in \
      "${LOCALAPPDATA:-}/Android/Sdk" \
      "$HOME/Library/Android/sdk" \
      "$HOME/Android/Sdk"; do
      if [ -n "$candidate" ] && [ -d "$candidate" ]; then sdk="$candidate"; break; fi
    done
  fi
  if [ -z "$sdk" ] || [ ! -d "$sdk" ]; then
    echo "未找到 Android SDK，请安装 Android Studio，或设置 ANDROID_HOME/ANDROID_SDK_ROOT"
    exit 1
  fi
  sdk="$(cd "$sdk" && pwd)"
  printf 'sdk.dir=%s\n' "${sdk//\\//}" > "$ANDROID_DIR/local.properties"
  echo "已生成 local.properties: sdk.dir=$sdk"
fi

cd "$ANDROID_DIR"
if [ "$RELEASE" -eq 1 ]; then
  ./gradlew :app:assembleRelease
else
  ./gradlew :app:assembleDebug
fi

if [ "$RELEASE" -eq 1 ]; then
  SOURCE="$ANDROID_DIR/app/build/outputs/apk/release/app-release-unsigned.apk"
  NAME="hawk-android-release-unsigned.apk"
else
  SOURCE="$ANDROID_DIR/app/build/outputs/apk/debug/app-debug.apk"
  NAME="hawk-android-debug.apk"
fi
[ -f "$SOURCE" ] || { echo "构建产物不存在: $SOURCE"; exit 1; }

cp "$SOURCE" "$OUT_DIR/$NAME"
echo ""
echo "APK: $OUT_DIR/$NAME"
if [ "$RELEASE" -eq 1 ]; then
  echo "release 产物未签名，不能直接安装；调试请用默认的 debug 构建。"
fi
echo "完成：产物已归置到 $OUT_DIR。"
