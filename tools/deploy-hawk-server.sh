#!/usr/bin/env bash
# hawk-server 服务器一键 Docker 部署：注入访问密钥 → 构建并启动容器 → 打印接入信息。
#
# 用法（服务器上，仓库任意目录执行均可）：
#   tools/deploy-hawk-server.sh                        # 首次：生成密钥并启动；重跑：重建升级
#   HAWK_SERVER_ACCESS_KEY=xxx tools/deploy-hawk-server.sh   # 自带密钥首次部署
#
# 幂等：密钥只注入一次（替换 compose 占位符；已有密钥则保留），重跑不重新生成。
#
# 国内拉不动 docker.io 基础镜像时，先配 registry mirror（daemon.json）；仍失败可传代理前缀：
#   export RUST_IMAGE=docker.m.daocloud.io/library/rust:1-slim-bookworm
#   export NODE_IMAGE=docker.m.daocloud.io/library/node:26-slim
#   bash tools/deploy-hawk-server.sh

# 被 sh/dash 误调时转交 bash 重跑（脚本使用 bash 专有语法）
if [ -z "${BASH_VERSION:-}" ]; then exec bash "$0" "$@"; fi
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE_FILE="$REPO_ROOT/docker-compose.yml"
PLACEHOLDER='在此填入访问密钥'

log() { printf '\033[1;32m[deploy]\033[0m %s\n' "$*"; }
die() { printf '\033[1;31m[deploy] %s\033[0m\n' "$*" >&2; exit 1; }

# ---- 依赖检查 ----
command -v docker >/dev/null 2>&1 || die "未安装 docker。Ubuntu/Debian 可：curl -fsSL https://get.docker.com | sh"
docker compose version >/dev/null 2>&1 || die "docker compose 插件不可用（docker compose version 失败）"
[ -f "$COMPOSE_FILE" ] || die "未找到 $COMPOSE_FILE（脚本应位于仓库 tools/ 目录下）"

# ---- 密钥注入（幂等） ----
if grep -q "$PLACEHOLDER" "$COMPOSE_FILE"; then
  KEY="${HAWK_SERVER_ACCESS_KEY:-}"
  if [ -z "$KEY" ]; then
    if command -v openssl >/dev/null 2>&1; then
      KEY="$(openssl rand -hex 32)"
    else
      KEY="$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')"
    fi
    log "已生成访问密钥：$KEY"
  else
    log "使用环境变量提供的访问密钥"
  fi
  sed -i.bak "s|$PLACEHOLDER|$KEY|" "$COMPOSE_FILE"
  rm -f "$COMPOSE_FILE.bak"
  log "密钥已写入 docker-compose.yml"
else
  KEY="$(sed -n "s/.*HAWK_SERVER_ACCESS_KEY: *'\([^']*\)'.*/\1/p" "$COMPOSE_FILE" | head -1)"
  log "使用 docker-compose.yml 中已有密钥"
fi

# ---- 构建并启动 ----
log "构建并启动 hawk-server（首次构建约 3-8 分钟）…"
docker compose -f "$COMPOSE_FILE" up -d --build

# ---- 接入信息 ----
log "部署完成："
docker compose -f "$COMPOSE_FILE" ps
cat <<EOF

下一步：
  1. nginx 反代到 127.0.0.1:27370（示例见 docs/backend/remote-deployment.md，
     必带 X-Real-IP、X-Forwarded-For 与 WebSocket 升级头，SSE 关缓冲）
  2. 桌面端 hawk → 设置 → 远程：服务端地址 https://<你的域名>，访问密钥：$KEY
  3. 网页版：https://<你的域名> 输入同一密钥

重跑本脚本 = 重建镜像并重启（密钥不变）；卸载：docker compose down -v
EOF
