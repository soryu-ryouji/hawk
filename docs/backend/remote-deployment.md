# 远程访问部署（hawk-server）

> 多租户自托管部署：一台有公网的服务器 + 一个域名（可选但推荐），即可让各设备的 hawk 互联（每台一个池或共用一个池），并通过浏览器经网页版访问素材库；持有带写权限访问密钥的插件可向目标设备导入素材。本文讲编译产物与反代部署；启动参数、租户管理、接口、网页版操作与运维排查见 [hawk-server 使用指南](hawk-server.md)，架构见 [远程访问设计](remote-access.md)，协议见 [remote-protocol](remote-protocol.md)。

## 组成

| 部件 | 说明 |
| ---- | ---- |
| hawk-server 二进制 | 仓库 `hawk-server/` 编译（`tools/deploy-hawk-server.sh`），单文件 |
| 数据目录 | 设备注册表、租户与 key 注册表、pepper、网页版身份、浏览器令牌（JSON 文件，备份即迁移，见[数据目录](hawk-server.md#数据目录)） |
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

启动无需任何密钥参数；启动后用 `tenant` 子命令创建租户拿到 service-key（明文只显示一次，服务端只存 HMAC 哈希）：

```bash
./hawk-server.bin --data-dir /var/lib/hawk-server        # 启动服务

# 另开终端创建租户（运行中的服务立刻感知，无需重启）
./hawk-server.bin tenant add --label home --data-dir /var/lib/hawk-server
# → 输出租户 id 与 service-key（hs_...），立即保存
```

systemd 单元示例：

```ini
[Unit]
Description=hawk-server
After=network.target

[Service]
ExecStart=/usr/local/bin/hawk-server --data-dir /var/lib/hawk-server
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

租户管理同样在命令行完成（`tenant list` / `tenant revoke`），数据目录见[配置](hawk-server.md#配置)。

二进制的更新方式：重新运行 `tools/deploy-hawk-server.sh --path /usr/local/bin/hawk-server` 覆盖，再重启服务。

## 域名与 TLS（nginx 反代）

宿主机 nginx 终结 TLS（主域名证书已有），反代到环回的 9100。控制面 WSS 与中继 WS 需要显式的升级头，查看器事件流（SSE）需要关闭响应缓冲：

```nginx
server {
    listen 443 ssl;
    server_name hawk.example.com;
    # ssl_certificate / ssl_certificate_key 沿用主域名已有配置

    location / {
        proxy_pass http://127.0.0.1:9100;
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

        # 插件经域名保存图片（base64 请求体）；nginx 默认 1MB，会 413
        client_max_body_size 256m;
        proxy_request_buffering off;
    }
}
```

防火墙只放行 443/80 与 **UDP 9101**（打洞探测口，P3：NAT 打洞需要它公网直达，UDP 不经反代；云服务器还需在**安全组**（阿里云/腾讯云控制台）显式放行 UDP 9101，仅改宿主机防火墙不够。未开放时打洞编排探测超时，自动退化为「无直连候选即 unreachable」，其余功能不受影响）；**9100 不要对公网开放**（明文 HTTP，密钥虽兜底但 TLS 是底线，且 X-Real-IP 头可信与 IP 风控生效的前提是流量必经反代）。管理面端口 9110 只绑环回（租户创建/查询/吊销），**不要经反代暴露也不要放防火墙**；远程管理用 `ssh -L 9110:127.0.0.1:9110 <服务器>` 端口转发。`X-Forwarded-Proto` 别删：查看器的 `hawk_bt` cookie 据此判断要不要加 `Secure`。用 Caddy 则只需 `reverse_proxy 127.0.0.1:9100`（WSS 透传与 `X-Forwarded-Proto` 都无需额外配置，同样建议补 `X-Real-IP` 头）。

## 部署后验证

1. `curl -s -H "Authorization: Bearer <service-key>" https://hawk.example.com/api/v1/devices` 返回 `{"status":"success",...}`
1. `node tools/check-probe-port.mjs hawk.example.com` 返回「✓ 探测口可达」（打洞探测口 UDP 9101 连通性；无响应 = 安全组/防火墙未放行 UDP）
2. 桌面端 hawk → 设置 → 远程填服务端地址与服务密钥（service-key），设备出现在列表中
3. 桌面端设置 → 远程 →「访问密钥」签发一把只读 key；浏览器打开 `https://hawk.example.com`，先填 service-key 再填访问密钥，即可进入绑定素材库的查看器（查看器页面由目标设备的 daemon 经隧道提供，该设备需是带前端产物的桌面端）
4. 插件设置选「远程」→ 填域名 + service-key + 带写权限的访问密钥 → 验证并查看素材库 → 保存。目标设备需在设置 → 远程中开启「允许广域网保存素材」

客户端接入步骤、网页版操作、数据备份与常见故障见 [hawk-server 使用指南](hawk-server.md)。
