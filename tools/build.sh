#!/usr/bin/env bash
# 发包：构建 hawk 桌面应用的分发包并归置到仓库根目录的 out/。
# macOS 产物为 hawk-mac-<arch>.zip（.app 目录压缩包），Linux 产物为 hawk-linux-x64.AppImage。
# -e/--extensions：附带构建浏览器插件（out/hawk-extension-chrome|firefox/，加载已解压扩展即用）。
#
# 用法: ./tools/build.sh [-e|--extensions]
# 前置: 最新 Node.js 与 Rust 工具链（https://rustup.rs/）
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_DIR="$REPO_ROOT/hawk-app"
EXT_DIR="$REPO_ROOT/hawk-browser-extension"
OUT_DIR="$REPO_ROOT/out"
EXTENSIONS=0
for arg in "$@"; do
  case "$arg" in
    -e|--extensions) EXTENSIONS=1 ;;
    *) echo "未知参数: $arg（支持 -e / --extensions）"; exit 1 ;;
  esac
done

for tool in node npm cargo; do
  command -v "$tool" >/dev/null 2>&1 || { echo "未找到 $tool，请先安装最新的 Node.js 与 Rust 工具链（https://rustup.rs/）"; exit 1; }
done

cd "$APP_DIR"
[ -d node_modules ] || npm install
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

if [ "$EXTENSIONS" -eq 1 ]; then
  cd "$EXT_DIR"
  [ -d node_modules ] || npm install
  npm run build
  npm run build:firefox
  CHROME_OUT="$(ls -d .output/chrome-mv3 2>/dev/null | head -n1)"
  FIREFOX_OUT="$(ls -d .output/firefox-mv2 2>/dev/null | head -n1)"
  [ -n "$CHROME_OUT" ] && [ -n "$FIREFOX_OUT" ] || { echo "插件构建产物不存在: .output/chrome-mv3 或 .output/firefox-mv2"; exit 1; }
  # 插件目录独立，先删后拷避免旧版本残留
  rm -rf "$OUT_DIR/hawk-extension-chrome" "$OUT_DIR/hawk-extension-firefox"
  cp -R "$CHROME_OUT" "$OUT_DIR/hawk-extension-chrome"
  cp -R "$FIREFOX_OUT" "$OUT_DIR/hawk-extension-firefox"
  echo "浏览器插件: $OUT_DIR/hawk-extension-chrome、hawk-extension-firefox（浏览器「加载已解压的扩展程序」直接用）"
fi

echo "完成：全部产物已归置到 $OUT_DIR。"
