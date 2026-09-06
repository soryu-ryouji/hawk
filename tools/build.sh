#!/usr/bin/env bash
# 统一发包入口：--platform 选择目标，转发到对应构建脚本（编译实现只在 build-app / build-extension 一处）。
# 用法: ./tools/build.sh --platform <app|extension> [--path <输出目录>]
#   ./tools/build.sh --platform app               # 桌面应用（mac → hawk-mac-<arch>.zip；Linux → AppImage）
#   ./tools/build.sh --platform extension         # 浏览器插件 → out/hawk-extension-chrome|firefox/
#   ./tools/build.sh --platform app --path /tmp/pub   # 指定输出目录（--path= 写法亦可）
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

PLATFORM=""
OUT_PATH=""
while [ $# -gt 0 ]; do
  case "$1" in
    --platform) [ $# -ge 2 ] || { echo "--platform 需要参数（app|extension）"; exit 1; }; PLATFORM="$2"; shift 2 ;;
    --platform=*) PLATFORM="${1#*=}"; shift ;;
    --path) [ $# -ge 2 ] || { echo "--path 需要目录参数"; exit 1; }; OUT_PATH="$2"; shift 2 ;;
    --path=*) OUT_PATH="${1#*=}"; shift ;;
    *) echo "未知参数: $1（用法: ./tools/build.sh --platform <app|extension> [--path <输出目录>]）"; exit 1 ;;
  esac
done

[ -n "$PLATFORM" ] || { echo "缺少 --platform（用法: ./tools/build.sh --platform <app|extension> [--path <输出目录>]）"; exit 1; }
case "$PLATFORM" in
  app|extension) ;;
  *) echo "未知 --platform: $PLATFORM（支持 app|extension）"; exit 1 ;;
esac

# 转发到对应构建脚本（产物目录参数原样透传）
if [ -n "$OUT_PATH" ]; then
  exec "$REPO_ROOT/tools/build-$PLATFORM.sh" --path "$OUT_PATH"
else
  exec "$REPO_ROOT/tools/build-$PLATFORM.sh"
fi
