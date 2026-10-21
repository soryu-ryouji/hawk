# 远程访问部署（hawk-server）

> 单用户自托管部署：一台有公网的服务器 + 一个域名（可选但推荐），即可让各设备的 hawk 互联，并通过浏览器经网页版只读访问素材库。本文讲编译产物与反代部署；启动参数、接口、网页版操作与运维排查见 [hawk-server 使用指南](hawk-server.md)，架构见 [远程访问设计](remote-access.md)，协议见 [remote-protocol](remote-protocol.md)。

## 组成

| 部件 | 说明 |
| ---- | ---- |
| hawk-server 二进制 | 仓库 `hawk-server/` 编译（`tools/deploy-hawk-server.sh`），单文件 |
| 数据目录 | 设备注册表、网页版身份、浏览器令牌（JSON 文件，备份即迁移，见[数据目录](hawk-server.md#数据目录)） |
| 域名 + TLS 反代 | Caddy（自动签发续期，推荐）或 nginx；控制面 WSS 与网页版都要求 HTTPS/WSS |

网页版查看器的前端产物不落在服务器上：`/app`、`/assets/*` 等静态请求由服务端经隧道转发给正在浏览的那台设备的 daemon，由它托管自带的 `web/dist`（桌面端启动时传入）。因此服务端部署只有二进制 + 数据目录，前端版本永远与 daemon 一致。

## 编译

```bash
tools/deploy-hawk-server.sh                              # 默认输出仓库根 hawk-server.bin
tools/deploy-hawk-server.sh --path /usr/local/bin/hawk-server   # 指定输出文件
```

- 脚本只做 `cargo build --release --locked` 并把二进制复制到输出路径，不安装、不启动
- 仓库根已有同名目录 `hawk-server/`，默认文件名用 `hawk-server.bin`；`--path` 传目录（已存在或以 `/` 结尾）时写入该目录下的 `hawk-server`
- 脚本按本机平台构建：给 Linux 服务器部署时在服务器上执行，或自行交叉编译
- cargo 依赖默认走 rsproxy 国内镜像；`CARGO_MIRROR=` 回退官方 crates.io，也可换成其他 sparse 索引（地址以 `/` 结尾）

## 运行

```bash
openssl rand -hex 32                  # 生成访问密钥（客户端与网页登录都用它）

./hawk-server.bin \
  --access-key <上一步的密钥> \
  --data-dir /var/lib/hawk-server
```

参数与环境变量对照表见[配置](hawk-server.md#配置)。systemd 单元示例：

```ini
[Unit]
Description=hawk-server
After=network.target

[Service]
# 密钥走环境变量，不落命令行（ps 可见）
Environment=HAWK_SERVER_ACCESS_KEY=<密钥>
ExecStart=/usr/local/bin/hawk-server --data-dir /var/lib/hawk-server
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

二进制的更新方式：重新运行 `tools/deploy-hawk-server.sh --path /usr/local/bin/hawk-server` 覆盖，再重启服务。

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

防火墙只放行 443/80；**27370 不要对公网开放**（明文 HTTP，密钥虽兜底但 TLS 是底线，且 X-Real-IP 头可信的前提是流量必经反代）。`X-Forwarded-Proto` 别删：查看器的 `hawk_bt` cookie 据此判断要不要加 `Secure`。用 Caddy 则只需 `reverse_proxy 127.0.0.1:27370`（WSS 透传与 `X-Forwarded-Proto` 都无需额外配置，同样建议补 `X-Real-IP` 头）。

## 部署后验证

1. `curl -s -H "Authorization: Bearer <密钥>" https://hawk.example.com/api/v1/devices` 返回 `{"status":"success",...}`
2. 桌面端 hawk → 设置 → 远程填服务端地址与密钥，设备出现在列表中
3. 浏览器打开 `https://hawk.example.com`，登录后能选到在线设备并进入查看器（查看器页面由目标设备的 daemon 经隧道提供，该设备需是带前端产物的桌面端）

客户端接入步骤、网页版操作、数据备份与常见故障见 [hawk-server 使用指南](hawk-server.md)。
