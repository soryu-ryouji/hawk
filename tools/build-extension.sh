#!/usr/bin/env bash
# 发包（浏览器插件）：构建 Chrome / Firefox 插件并归置到输出目录
# （默认仓库根目录的 out/，即 out/hawk-extension-chrome|firefox/，浏览器「加载已解压的扩展程序」直接用）。
# 桌面应用是独立命令 tools/build-app.sh；统一入口 tools/build.sh --platform extension。
#
# 用法: ./tools/build-extension.sh [--path <输出目录>]
# 前置: 最新 Node.js（https://nodejs.org/）
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXT_DIR="$REPO_ROOT/hawk-browser-extension"

OUT_DIR=""
while [ $# -gt 0 ]; do
  case "$1" in
    --path) [ $# -ge 2 ] || { echo "--path 需要目录参数"; exit 1; }; OUT_DIR="$2"; shift 2 ;;
    --path=*) OUT_DIR="${1#*=}"; shift ;;
    *) echo "未知参数: $1（用法: ./tools/build-extension.sh [--path <输出目录>]）"; exit 1 ;;
  esac
done
OUT_DIR="${OUT_DIR:-$REPO_ROOT/out}"

for tool in node npm; do
  command -v "$tool" >/dev/null 2>&1 || { echo "未找到 $tool，请先安装最新的 Node.js（https://nodejs.org/）"; exit 1; }
done

cd "$EXT_DIR"
[ -d node_modules ] || npm install
npm run build
npm run build:firefox

CHROME_OUT="$(ls -d .output/chrome-mv3 2>/dev/null | head -n1)"
FIREFOX_OUT="$(ls -d .output/firefox-mv2 2>/dev/null | head -n1)"
[ -n "$CHROME_OUT" ] && [ -n "$FIREFOX_OUT" ] || { echo "插件构建产物不存在: .output/chrome-mv3 或 .output/firefox-mv2"; exit 1; }
# 插件目录独立，先删后拷避免旧版本残留
mkdir -p "$OUT_DIR"
rm -rf "$OUT_DIR/hawk-extension-chrome" "$OUT_DIR/hawk-extension-firefox"
cp -R "$CHROME_OUT" "$OUT_DIR/hawk-extension-chrome"
cp -R "$FIREFOX_OUT" "$OUT_DIR/hawk-extension-firefox"
echo "浏览器插件: $OUT_DIR/hawk-extension-chrome、hawk-extension-firefox（浏览器「加载已解压的扩展程序」直接用）"
echo "完成：产物已归置到 $OUT_DIR。"
