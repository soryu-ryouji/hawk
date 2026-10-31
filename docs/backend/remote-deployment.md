# 远程访问部署（hawk-server）

> hawk-server 当前部署内容为单二进制控制面 + 可选密文中继，不包含公网网页版或前端静态产物。
> 配置与运维见 [hawk-server.md](hawk-server.md)，控制面接口见 [remote-protocol.md](remote-protocol.md)。

## 1. 组成

| 部件 | 说明 |
| --- | --- |
| hawk-server 二进制 | 仓库 `hawk-server/` 构建 |
| 数据目录 | tenant/device 注册表、pepper、relay token 和 server.toml |
| 域名 + TLS 反代 | Caddy 或 nginx，终结 HTTPS/WSS |
| 可选 relay | 内嵌 iroh-relay，仅处理端到端加密流量 |

不再部署或托管 `/app`、`/assets/*`、公网登录页和 viewer token。

## 2. 编译与运行

```bash
tools/deploy-hawk-server.sh --path /usr/local/bin/hawk-server

sudo useradd --system --home /var/lib/hawk-server --shell /usr/sbin/nologin hawk
sudo install -d -o hawk -g hawk /var/lib/hawk-server
sudo -u hawk /usr/local/bin/hawk-server tenant add --label home
```

保存输出的 tenant_key。systemd 示例：

```ini
[Unit]
Description=hawk-server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=hawk
Group=hawk
ExecStart=/usr/local/bin/hawk-server --data-dir /var/lib/hawk-server
Restart=on-failure
RestartSec=3
NoNewPrivileges=true
ProtectSystem=strict
ReadWritePaths=/var/lib/hawk-server
ProtectHome=true
PrivateTmp=true
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now hawk-server
journalctl -u hawk-server -f
```

## 3. nginx 反代

主控制面监听明文 HTTP，但生产环境只应通过 HTTPS/WSS 暴露。控制面和 WSS 共用主入口：

```nginx
server {
    listen 443 ssl;
    server_name hawk.example.com;
    # ssl_certificate / ssl_certificate_key 按部署填写

    location / {
        proxy_pass http://127.0.0.1:9100;
        proxy_http_version 1.1;

        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        proxy_read_timeout 3600s;
        proxy_send_timeout 3600s;
    }
}
```

- 防火墙仅开放 80/443；9100 不对公网开放。
- 管理面 9110 仅环回，远程管理使用 SSH 端口转发。
- 反代必须传递真实客户端地址；不要允许公网直接伪造 Forwarded 系列头。
- Caddy 可使用 `reverse_proxy 127.0.0.1:9100`，WSS 自动透传。

## 4. 可选密文中继

配置 relay 公网入口后启动内嵌 relay：

```bash
hawk-server --data-dir /var/lib/hawk-server \
  --relay-url https://hawk.example.com:9102 \
  --relay-port 9102
```

relay 同样只监听环回，由独立反代入口转发到其上游端口。防火墙/安全组放行的是对外 relay 端口，不是环回上游端口。

未配置 relay_url 时不提供 relay：只有直连地址可达的 Device 会话能够建立。relay 只承载 iroh 端到端加密数据，不能查看素材明文。

## 5. 部署后验证

1. `POST /api/v1/tenant/session` 能换取 TenantAdmin Session。
2. 桌面端保存服务器地址和 tenant_key 后，daemon 自动完成设备注册和签名认证。
3. 设备出现在同 tenant 列表，心跳和 WSS 保持在线。
4. 两台 Device 能创建并接受会话；需要 relay 时，连接信息包含 relay url/token。
5. 以下旧路径应返回 404：`/api/v1/viewer/*`、`/api/v1/devices/{id}/keys`、`/` 的公网登录页。

## 6. 备份与升级

备份整个数据目录，尤其 `tenants.json` 与 `pepper` 必须一起恢复。Session、nonce 和在线状态在内存，重启后由 daemon 自动恢复。

使用部署脚本覆盖二进制并重启即可。V2 不保留旧认证兼容路径，server 与 daemon 需同步升级。
