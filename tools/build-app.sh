#!/usr/bin/env bash
# 发包（桌面应用）：构建 hawk 桌面应用的分发包并归置到输出目录（默认仓库根目录的 out/）。
# macOS 产物为 hawk-mac-<arch>.zip（.app 目录压缩包），Linux 产物为 hawk-linux-x64.AppImage。
# 浏览器插件是独立命令 tools/build-extension.sh；统一入口 tools/build.sh --platform app。
#
# 用法: ./tools/build-app.sh [--path <输出目录>] [--unpacked]
#   --unpacked：只产出未打包目录（mac 为 dist/mac*/hawk.app，Linux 为 dist/linux-unpacked），
#               跳过 zip/AppImage 压缩——install.sh 的复用入口
# 前置: 最新 Node.js 与 Rust 工具链（https://rustup.rs/）
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_DIR="$REPO_ROOT/hawk-app"

OUT_DIR=""
UNPACKED=0
while [ $# -gt 0 ]; do
  case "$1" in
    --path) [ $# -ge 2 ] || { echo "--path 需要目录参数"; exit 1; }; OUT_DIR="$2"; shift 2 ;;
    --path=*) OUT_DIR="${1#*=}"; shift ;;
    --unpacked) UNPACKED=1; shift ;;
    *) echo "未知参数: $1（用法: ./tools/build-app.sh [--path <输出目录>] [--unpacked]）"; exit 1 ;;
  esac
done
OUT_DIR="${OUT_DIR:-$REPO_ROOT/out}"

for tool in node npm cargo; do
  command -v "$tool" >/dev/null 2>&1 || { echo "未找到 $tool，请先安装最新的 Node.js 与 Rust 工具链（https://rustup.rs/）"; exit 1; }
done

# Electron 包（npm install）与 electron-builder（pack）的二进制下载默认走 npmmirror（国内网络；
# 用户已设置同名环境变量时尊重用户配置）
export ELECTRON_MIRROR="${ELECTRON_MIRROR:-https://npmmirror.com/mirrors/electron/}"
export ELECTRON_BUILDER_BINARIES_MIRROR="${ELECTRON_BUILDER_BINARIES_MIRROR:-https://npmmirror.com/mirrors/electron-builder-binaries/}"

cd "$APP_DIR"
[ -d node_modules ] || npm install
if [ "$UNPACKED" -eq 1 ]; then
  npm run pack:dir # install 只需要未打包目录，跳过 zip/AppImage 压缩
  echo "未打包产物: $APP_DIR/dist/"
  exit 0
fi
npm run pack

mkdir -p "$OUT_DIR"
case "$(uname -s)" in
  Darwin)
    APP="$(ls -d dist/mac*/hawk.app 2>/dev/null | head -n1)"
    [ -n "$APP" ] || { echo "打包产物不存在: dist/mac*/hawk.app（electron-builder 未产出）"; exit 1; }
    case "$(uname -m)" in
      arm64) ARCH=arm64 ;;
      x86_64) ARCH=x64 ;;
      *) ARCH="$(uname -m)" ;;
    esac
    # -y 保留符号链接（.app 内 Framework 链接必需）
    (cd "$(dirname "$APP")" && zip -ry "$OUT_DIR/hawk-mac-$ARCH.zip" hawk.app)
    echo "应用分发包: $OUT_DIR/hawk-mac-$ARCH.zip"
    ;;
  Linux)
    APPIMAGE="$(ls dist/*.AppImage 2>/dev/null | head -n1)"
    [ -n "$APPIMAGE" ] || { echo "打包产物不存在: dist/*.AppImage（electron-builder 未产出）"; exit 1; }
    cp "$APPIMAGE" "$OUT_DIR/"
    chmod +x "$OUT_DIR/$(basename "$APPIMAGE")"
    echo "应用分发包: $OUT_DIR/$(basename "$APPIMAGE")"
    ;;
  *)
    echo "不支持的平台: $(uname -s)"; exit 1
    ;;
esac

echo "完成：产物已归置到 $OUT_DIR。"
