# 远程访问部署（hawk-server）

> 单用户自托管部署：一台有公网的服务器 + 一个域名（可选但推荐），即可让各设备的 hawk 互联，并通过浏览器经网页版只读访问素材库。启动参数、接口、网页版操作与运维排查见 [hawk-server 使用指南](hawk-server.md)；架构见 [远程访问设计](remote-access.md)，协议见 [remote-protocol](remote-protocol.md)。

## 组成

| 部件 | 说明 |
| ---- | ---- |
| hawk-server 二进制 | 仓库 `hawk-server/` 构建（`cargo build --release`），单文件无依赖 |
| 数据目录 | 设备注册表、网页版身份、浏览器令牌（JSON 文件，备份即迁移，见[数据目录](hawk-server.md#数据目录)） |
| 域名 + TLS 反代 | Caddy（自动签发续期，推荐）或 nginx；控制面 WSS 与网页版都要求 HTTPS/WSS |
| web 查看器产物（网页版需要） | `hawk-app/web/dist`（`cd hawk-app && npm run build` 产出），整目录上传到服务器 |

## Docker（推荐）

仓库根目录提供一体化 `Dockerfile`（镜像内同时构建 hawk-server 与 web 查看器产物）与 `docker-compose.yml`。最省事的方式是一键脚本（自动生成/注入密钥、构建启动、打印接入信息，幂等重跑即升级）：

```bash
tools/deploy-hawk-server.sh
```

或手动：编辑 compose 文件中的 `HAWK_SERVER_ACCESS_KEY`（`openssl rand -hex 32` 生成）后 `docker compose up -d --build`。

数据在命名卷 `hawk-server-data`（即容器内 `/data`）；明文端口只绑环回（127.0.0.1:27370），公网入口交给宿主机 nginx。不使用 compose 时等价命令：

```bash
docker build -t hawk-server .
docker run -d --name hawk-server --restart unless-stopped \
  -e HAWK_SERVER_ACCESS_KEY=<密钥> \
  -v hawk-server-data:/data \
  -p 127.0.0.1:27370:27370 \
  hawk-server
```

## 手动运行（二进制）

```bash
openssl rand -hex 32                  # 生成访问密钥（客户端与网页登录都用它）

hawk-server \
  --access-key <上一步的密钥> \
  --port 27370 \
  --data-dir /var/lib/hawk-server \
  --web-dist /opt/hawk-web-dist       # 可选：网页版查看器产物目录
```

参数与环境变量对照表见[配置](hawk-server.md#配置)。systemd 单元示例：

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

## 域名与 TLS（nginx 反代）

宿主机 nginx 终结 TLS（主域名证书已有），反代到环回的 27370。控制面 WSS 与中继 WS 需要显式的升级头，查看器事件流（SSE）需要关闭响应缓冲：

```nginx
server {
    listen 443 ssl;
    server_name hawk.example.com;
    # ssl_certificate / ssl_certificate_key 沿用主域名已有配置

    location / {
        proxy_pass http://127.0.0.1:27370;
        proxy_http_version 1.1;

        # WebSocket（控制面 WSS / 中继 WS）
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";

        # 转发头（hawk-server 据 X-Real-IP / X-Forwarded-For 还原设备公网 IP，直连候选依赖它）
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # SSE（查看器事件流）：关闭缓冲，否则事件被延迟聚合
        proxy_buffering off;

        # 素材大文件流式响应
        proxy_read_timeout 3600s;
        proxy_send_timeout 3600s;
    }
}
```

防火墙只放行 443/80；**27370 不要对公网开放**（明文 HTTP，密钥虽兜底但 TLS 是底线，且 X-Real-IP 头可信的前提是流量必经反代）。用 Caddy 则只需 `reverse_proxy 127.0.0.1:27370`（WSS 透传无需额外配置，同样建议补 `X-Real-IP` 头）。

## 部署后验证

1. `curl -s -H "Authorization: Bearer <密钥>" https://hawk.example.com/api/v1/devices` 返回 `{"status":"success",...}`
2. 桌面端 hawk → 设置 → 远程填服务端地址与密钥，设备出现在列表中
3. 浏览器打开 `https://hawk.example.com`，登录后能选到在线设备并进入查看器

客户端接入步骤、网页版操作、数据备份与常见故障见 [hawk-server 使用指南](hawk-server.md)。
