# 远程访问部署（hawk-server）

> 单用户自托管部署：一台有公网的服务器 + 一个域名（可选但推荐），即可让各设备的 hawk 互联，并通过浏览器经网页版只读访问素材库。架构见 [远程访问设计](remote-access.md)，协议见 [remote-protocol](remote-protocol.md)。

## 组成

| 部件 | 说明 |
| ---- | ---- |
| hawk-server 二进制 | 仓库 `hawk-server/` 构建（`cargo build --release`），单文件无依赖 |
| 数据目录 | 设备注册表、网页版身份、浏览器令牌（JSON 文件，备份即迁移） |
| 域名 + TLS 反代 | Caddy（自动签发续期，推荐）或 nginx；控制面 WSS 与网页版都要求 HTTPS/WSS |
| web 查看器产物（网页版需要） | `hawk-app/web/dist`（`cd hawk-app && npm run build` 产出），整目录上传到服务器 |

## Docker（推荐）

仓库根目录提供一体化 `Dockerfile`（镜像内同时构建 hawk-server 与 web 查看器产物）与 `docker-compose.yml`：

```bash
# 1. 准备密钥（写入 .env，不会烘进镜像）
echo "HAWK_SERVER_ACCESS_KEY=$(openssl rand -hex 32)" > .env

# 2. 仅 hawk-server（127.0.0.1:27370，自行反代）
docker compose up -d --build

# 3. 或连同 Caddy（编辑 Caddyfile 填入域名，自动 HTTPS/WSS）
docker compose --profile tls up -d --build
```

数据在命名卷 `hawk-server-data`（即容器内 `/data`）；不使用 compose 时等价命令：

```bash
docker build -t hawk-server .
docker run -d --name hawk-server --restart unless-stopped \
  -e HAWK_SERVER_ACCESS_KEY=<密钥> \
  -v hawk-server-data:/data \
  -p 127.0.0.1:27370:27370 \
  hawk-server
```

## 手动启动

```bash
# 生成一把访问密钥（保存好，客户端与网页登录都用它）
openssl rand -hex 32

hawk-server \
  --access-key <上一步的密钥> \
  --port 27370 \
  --data-dir /var/lib/hawk-server \
  --web-dist /opt/hawk-web-dist        # 可选：网页版查看器产物目录
```

参数均可走环境变量：`HAWK_SERVER_PORT` / `HAWK_SERVER_ACCESS_KEY` / `HAWK_SERVER_DATA_DIR` / `HAWK_SERVER_WEB_DIST`。

systemd 单元示例：

```ini
[Unit]
Description=hawk-server
After=network.target

[Service]
ExecStart=/usr/local/bin/hawk-server --access-key <密钥> --data-dir /var/lib/hawk-server --web-dist /opt/hawk-web-dist
Restart=on-failure
# 密钥不落命令行（ps 可见）的替代：Environment=HAWK_SERVER_ACCESS_KEY=<密钥>

[Install]
WantedBy=multi-user.target
```

## 域名与 TLS（反代）

```text
# Caddyfile：自动签发/续期证书，WSS 透传无需额外配置
hawk.example.com {
    reverse_proxy 127.0.0.1:27370
}
```

防火墙只放行 443/80；**27370 不要对公网开放**（明文 HTTP，密钥虽兜底但 TLS 是底线）。

nginx 等价配置需显式放行 WebSocket 升级头（`Upgrade`/`Connection`）。

## 客户端接入

桌面端 hawk → 设置 → 远程：

- 服务端地址：`https://hawk.example.com`
- 访问密钥：上面生成的密钥

保存即注册上线。其他设备同样接入后，设备列表互见，可发起直连（UPnP/IPv6）或经确认走中继。

## 网页版

浏览器打开 `https://hawk.example.com`：

1. 登录页输入访问密钥 → 换发浏览器令牌（bt，服务端存哈希，可随时在页面注销）
2. 设备列表选一台在线设备 → 服务端经中继建立到该设备的 QUIC 隧道（**素材明文过服务器**，自托管信任前提，见 remote-access.md 安全模型）
3. 进入完整 web 查看器（与局域网 web 查看同一构建产物），只读浏览

不配置 `--web-dist` 时登录/设备选择器仍可用，但无法进入查看器界面。

## 数据与迁移

`/var/lib/hawk-server` 全部状态即三个 JSON：`devices.json`（设备注册表）、`identity.json`（网页版设备身份与私钥，0600）、`viewer_tokens.json`（浏览器令牌哈希）。整目录备份即完成迁移；丢失 `devices.json` 后各客户端 daemon 会自动重注册（心跳 404 触发），网页版身份则重新生成（旧浏览器令牌失效需重新登录）。
