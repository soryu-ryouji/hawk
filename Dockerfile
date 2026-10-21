# hawk-server 一体化部署镜像：web 查看器产物 + hawk-server 二进制 + 极简运行时。
# 构建（仓库根目录）：docker build -t hawk-server .
# 运行：docker run -d -e HAWK_SERVER_ACCESS_KEY=<密钥> -v hawk-server-data:/data -p 127.0.0.1:27370:27370 hawk-server
# 域名/TLS 见 docker-compose.yml 与 docs/backend/remote-deployment.md
#
# 基础镜像可经构建参数替换（国内拉不动 docker.io 时传镜像代理，如：
#   NODE_IMAGE=docker.m.daocloud.io/library/node:26-slim docker compose up -d --build）
ARG NODE_IMAGE=node:26-slim
ARG RUST_IMAGE=rust:1-slim-bookworm
ARG DEBIAN_IMAGE=debian:bookworm-slim

# ---- 阶段 1：web 查看器产物（--web-dist 的来源） ----
FROM ${NODE_IMAGE} AS web-builder
WORKDIR /build
COPY hawk-app ./hawk-app
RUN cd hawk-app && npm ci && npm run build

# ---- 阶段 2：hawk-server 二进制（独立 crate，无需仓库其余部分） ----
FROM ${RUST_IMAGE} AS rust-builder
# ring 的汇编编译需要 C 工具链
RUN apt-get update && apt-get install -y --no-install-recommends gcc libc6-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY hawk-server ./hawk-server
RUN cd hawk-server && cargo build --release --locked

# ---- 阶段 3：运行时（无 OpenSSL 依赖，TLS 全程 rustls） ----
FROM ${DEBIAN_IMAGE}
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -r -u 10001 -m hawk \
    && mkdir -p /data && chown hawk:hawk /data
COPY --from=rust-builder /build/hawk-server/target/release/hawk-server /usr/local/bin/hawk-server
COPY --from=web-builder /build/hawk-app/web/dist /opt/hawk-web-dist

USER hawk
ENV HAWK_SERVER_PORT=27370 \
    HAWK_SERVER_DATA_DIR=/data \
    HAWK_SERVER_WEB_DIST=/opt/hawk-web-dist
# HAWK_SERVER_ACCESS_KEY 由运行时注入（必填，不烘进镜像）
VOLUME /data
EXPOSE 27370
ENTRYPOINT ["hawk-server"]
