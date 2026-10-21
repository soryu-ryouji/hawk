#!/usr/bin/env bash
# 编译 hawk-server 单文件二进制（不做安装/启动/进程管理）。
#
# 用法: tools/deploy-hawk-server.sh [--path <输出文件>]
#   默认输出: <仓库根>/hawk-server.bin（仓库根已有同名目录 hawk-server/，二进制用 .bin 后缀）
#   --path /usr/local/bin/hawk-server    指定输出文件；传目录（已存在或以 / 结尾）则写入其中的 hawk-server
#
# cargo 依赖默认走 rsproxy 国内镜像；回退官方源或换镜像：
#   CARGO_MIRROR= bash tools/deploy-hawk-server.sh
#   CARGO_MIRROR=https://mirrors.ustc.edu.cn/crates.io-index/ bash tools/deploy-hawk-server.sh
#
# 脚本按本机平台构建。给 Linux 服务器部署时，在服务器上执行（或自行交叉编译）。

# 被 sh/dash 误调时转交 bash 重跑（脚本使用 bash 专有语法）
if [ -z "${BASH_VERSION:-}" ]; then exec bash "$0" "$@"; fi
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$REPO_ROOT/hawk-server.bin"

usage() {
  echo "用法: $0 [--path <输出文件>]"
  echo "  默认输出: $REPO_ROOT/hawk-server.bin"
  echo "  CARGO_MIRROR=<sparse 索引地址> 换 cargo 源；CARGO_MIRROR= 回退官方 crates.io"
}

while [ $# -gt 0 ]; do
  case "$1" in
    --path)
      [ $# -ge 2 ] || { echo "--path 需要参数"; exit 1; }
      OUT="$2"
      shift 2
      ;;
    --path=*)
      OUT="${1#*=}"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "未知参数: $1"
      usage
      exit 1
      ;;
  esac
done

# 目录语义：显式传入目录时落到目录内的 hawk-server
case "$OUT" in
  */) OUT="${OUT%/}/hawk-server" ;;
esac
if [ -d "$OUT" ]; then OUT="$OUT/hawk-server"; fi

MIRROR="${CARGO_MIRROR-https://rsproxy.cn/index/}"
cargo_args=(build --release --locked --manifest-path "$REPO_ROOT/hawk-server/Cargo.toml")
if [ -n "$MIRROR" ]; then
  echo "[deploy] 构建 hawk-server（依赖源 ${MIRROR}）…"
  cargo_args+=(--config 'source.crates-io.replace-with="mirror"')
  cargo_args+=(--config "source.mirror.registry=\"sparse+${MIRROR}\"")
else
  echo "[deploy] 构建 hawk-server…"
fi
cargo "${cargo_args[@]}"

BUILT="$REPO_ROOT/hawk-server/target/release/hawk-server"
[ -f "$BUILT" ] || { echo "构建产物不存在: $BUILT"; exit 1; }

mkdir -p "$(dirname "$OUT")"
install -m 755 "$BUILT" "$OUT"
echo "[deploy] 已输出: $OUT"
