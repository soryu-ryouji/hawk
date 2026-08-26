#!/usr/bin/env bash
# 发布当前平台的 hawk-server 自包含单文件到 resources/hawk-server/（electron-builder 的 extraResources 来源）。
# 用法：scripts/build-server.sh [RID]   例：scripts/build-server.sh osx-arm64
set -euo pipefail

cd "$(dirname "$0")/.."
RID="${1:-}"
if [[ -z "$RID" ]]; then
  case "$(uname -s)-$(uname -m)" in
    Darwin-arm64) RID=osx-arm64 ;;
    Darwin-x86_64) RID=osx-x64 ;;
    Linux-x86_64) RID=linux-x64 ;;
    *) echo "未知平台，请显式传入 RID"; exit 1 ;;
  esac
fi

OUT="resources/hawk-server"
rm -rf "$OUT"
dotnet publish ../hawk-server/hawk-server.csproj -c Release -r "$RID" --self-contained \
  -p:PublishSingleFile=true -o "$OUT"
echo "已发布 $RID → $OUT"
