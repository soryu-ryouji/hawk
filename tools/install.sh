#!/usr/bin/env bash
# 本机安装：构建 hawk 桌面应用并安装到本机。
# macOS 直接安装到 /Applications/hawk.app；Linux 产物为 hawk-linux-x64.AppImage（已赋予执行权限），归置到仓库根目录的 out/。
#
# 用法: ./tools/install.sh（仓库根目录或任意位置执行均可）
# 前置: 最新 Node.js 与 Rust 工具链（https://rustup.rs/）
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_DIR="$REPO_ROOT/hawk-app"
OUT_DIR="$REPO_ROOT/out"

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
    rm -rf "/Applications/hawk.app"
    cp -R "$APP" "/Applications/hawk.app"
    echo "完成：应用已安装到 /Applications/hawk.app。"
    ;;
  Linux)
    APPIMAGE="$(ls dist/*.AppImage 2>/dev/null | head -n1)"
    [ -n "$APPIMAGE" ] || { echo "打包产物不存在: dist/*.AppImage（electron-builder 未产出）"; exit 1; }
    cp "$APPIMAGE" "$OUT_DIR/"
    chmod +x "$OUT_DIR/$(basename "$APPIMAGE")"
    echo "完成：应用已归置到 $OUT_DIR/$(basename "$APPIMAGE")（已赋予执行权限）。"
    ;;
  *)
    echo "不支持的平台: $(uname -s)"; exit 1
    ;;
esac
