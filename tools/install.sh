#!/usr/bin/env bash
# 本机安装：构建 hawk 桌面应用并安装到本机（编译复用 build-app.sh，不重复实现）。
# macOS 安装到 /Applications/hawk.app；Linux 归置 AppImage 到仓库根目录的 out/（已赋予执行权限）。
#
# 用法: ./tools/install.sh（仓库根目录或任意位置执行均可）
# 前置: 最新 Node.js 与 Rust 工具链（https://rustup.rs/）
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_DIR="$REPO_ROOT/hawk-app"
BUILD="$REPO_ROOT/tools/build-app.sh"

case "$(uname -s)" in
  Darwin)
    # --unpacked 跳过 zip 压缩，安装只需要 .app 目录
    "$BUILD" --unpacked
    APP="$(ls -d "$APP_DIR"/dist/mac*/hawk.app 2>/dev/null | head -n1)"
    [ -n "$APP" ] || { echo "打包产物不存在: dist/mac*/hawk.app（electron-builder 未产出）"; exit 1; }
    rm -rf "/Applications/hawk.app"
    cp -R "$APP" "/Applications/hawk.app"
    echo "完成：应用已安装到 /Applications/hawk.app。"
    ;;
  Linux)
    # Linux 的安装产物 = AppImage 归置到 out/，与 build-app.sh 的归置一致，直接复用完整构建
    "$BUILD"
    echo "完成：应用已归置到 $REPO_ROOT/out/（已赋予执行权限）。"
    ;;
  *)
    echo "不支持的平台: $(uname -s)"; exit 1
    ;;
esac
